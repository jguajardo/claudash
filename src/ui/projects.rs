//! Projects: every project as a card in a grid; Enter opens its page, with a
//! menu of sections on the left and the section on the right.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        Cell, HighlightSpacing, List, ListItem, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, Wrap,
    },
};

use super::{
    ACCENT, HIGHLIGHT, card, dim, draw_ecosystem, draw_mcp, draw_menu, field, git_summary,
    mcp_summary, plural, project_lines,
};
use crate::{
    app::{App, Card, Section},
    audit::Severity,
    hooks::Activity,
    paths,
    sessions::{self, Session, human_tokens},
    specs::Stage,
};

/// Width a card wants; the grid fits as many per row as there's room for.
const CARD_WIDTH: u16 = 46;
const CARD_HEIGHT: u16 = 8;
const MENU_WIDTH: u16 = 32;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.project_page.is_some() {
        draw_page(frame, app, area);
    } else {
        draw_grid(frame, app, area);
    }
}

// ---- Cards ---------------------------------------------------------------------

/// What a card says about a project.
struct Summary {
    sessions: usize,
    open: usize,
    needs_you: usize,
    last: Option<std::time::SystemTime>,
    week_tokens: u64,
    week_cost: f64,
    secrets: usize,
    risky: usize,
}

fn summarize(app: &App, sessions: &[&Session]) -> Summary {
    let week = chrono::Local::now().date_naive() - chrono::Days::new(6);
    let mut s = Summary {
        sessions: sessions.len(),
        open: 0,
        needs_you: 0,
        last: None,
        week_tokens: 0,
        week_cost: 0.0,
        secrets: 0,
        risky: 0,
    };
    for session in sessions {
        match app.activity(&session.id) {
            Some(Activity::NeedsYou) => {
                s.open += 1;
                s.needs_you += 1;
            }
            Some(Activity::Working | Activity::Waiting) => s.open += 1,
            _ => {}
        }
        s.last = s.last.max(Some(session.modified));
        for (day, models) in &session.tokens.daily {
            if *day >= week {
                s.week_tokens += models.values().map(|u| u.processed()).sum::<u64>();
                s.week_cost += models.values().map(|u| u.cost).sum::<f64>();
            }
        }
        if let Some(a) = app.analysis(session) {
            s.secrets += a.secrets.len();
            s.risky += a
                .audit
                .iter()
                .filter(|e| e.severity == Severity::High)
                .count();
        }
    }
    s
}

fn card_lines(app: &App, c: &Card) -> Vec<Line<'static>> {
    let sessions = app.project_sessions(&c.dir);
    let s = summarize(app, &sessions);
    let mut lines = Vec::new();

    // Git.
    let main = c.repo.and_then(|r| app.projects.repos[r].checkouts.first());
    match main {
        Some(co) => {
            let mut spans = vec![
                Span::styled("⎇ ", Style::new().fg(Color::Cyan)),
                Span::styled(
                    co.branch.clone().unwrap_or_else(|| "detached".into()),
                    Style::new().bold(),
                ),
                Span::raw("  "),
            ];
            spans.extend(git_summary(co.status.as_ref()));
            let worktrees = app.projects.repos[c.repo.unwrap_or(0)].checkouts.len() - 1;
            if worktrees > 0 {
                spans.push(Span::styled(
                    format!("  +{}", plural(worktrees as u64, "worktree")),
                    dim(),
                ));
            }
            lines.push(Line::from(spans));
        }
        None => lines.push(Line::from(Span::styled("▪ not a git repository", dim()))),
    }

    // Sessions.
    let mut spans = vec![
        Span::styled("≡ ", Style::new().fg(ACCENT)),
        Span::raw(plural(s.sessions as u64, "session")),
    ];
    if s.needs_you > 0 {
        spans.push(Span::styled(
            format!("  ▲ {} needs you", s.needs_you),
            Style::new().fg(Color::Yellow).bold(),
        ));
    } else if s.open > 0 {
        spans.push(Span::styled(
            format!("  ● {} open", s.open),
            Style::new().fg(Color::Green),
        ));
    }
    lines.push(Line::from(spans));
    let last = s
        .last
        .map(sessions::relative_age)
        .unwrap_or_else(|| "never".into());
    let mut spans = vec![Span::styled(format!("◷ {last}"), dim())];
    if s.week_tokens > 0 {
        spans.push(Span::styled(
            format!(" · week: {} tokens", human_tokens(s.week_tokens)),
            dim(),
        ));
        spans.push(Span::styled(
            format!(" ≈{}", crate::pricing::format_usd(s.week_cost)),
            Style::new().fg(Color::Green),
        ));
    }
    lines.push(Line::from(spans));

    // Specs.
    if let Some(specs) = app.project_specs(&c.dir) {
        let open: Vec<_> = specs
            .changes
            .iter()
            .filter(|ch| ch.stage != Stage::Complete)
            .collect();
        let (done, total) = open
            .iter()
            .fold((0, 0), |(d, t), ch| (d + ch.done, t + ch.total));
        let names: Vec<&str> = specs.frameworks.iter().map(|f| f.title()).collect();
        let mut spans = vec![
            Span::styled("▤ ", Style::new().fg(Color::LightBlue)),
            Span::raw(format!(
                "{} · {}",
                names.join(", "),
                plural(open.len() as u64, "open change")
            )),
        ];
        if total > 0 {
            spans.push(Span::styled(
                format!("  {done}/{total} tasks"),
                Style::new().fg(Color::LightBlue),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Alerts.
    let mut alerts = Vec::new();
    if s.secrets > 0 {
        alerts.push(Span::styled(
            format!("🔑 {}  ", plural(s.secrets as u64, "secret")),
            Style::new().fg(Color::Red),
        ));
    }
    if s.risky > 0 {
        alerts.push(Span::styled(
            format!("⚠ {} risky  ", s.risky),
            Style::new().fg(Color::Red),
        ));
    }
    let mut mcp_line = None;
    if let Some(Ok(servers)) = app.mcp_cache.get(&c.dir).map(|m| &m.result) {
        let count = |pick: fn(&crate::mcp::McpStatus) -> bool| {
            servers.iter().filter(|s| pick(&s.status)).count()
        };
        let failed = count(|s| matches!(s, crate::mcp::McpStatus::Failed(_)));
        let sign_in = count(|s| matches!(s, crate::mcp::McpStatus::NeedsAuth));
        // A line of its own: it doesn't fit next to the security badges.
        let mut mcp = Vec::new();
        if failed > 0 {
            mcp.push(Span::styled(
                format!("✗ {failed} MCP failed  "),
                Style::new().fg(Color::Red),
            ));
        }
        if sign_in > 0 {
            mcp.push(Span::styled(
                format!("◐ {sign_in} MCP to sign in"),
                Style::new().fg(Color::Yellow),
            ));
        }
        if !mcp.is_empty() {
            mcp_line = Some(Line::from(mcp));
        }
    }
    if !alerts.is_empty() {
        lines.push(Line::from(alerts));
    }
    lines.extend(mcp_line);
    lines
}

fn draw_grid(frame: &mut Frame, app: &mut App, area: Rect) {
    let cards = app.cards();
    if cards.is_empty() {
        let text = if app.sessions.is_empty() {
            "No sessions yet: projects show up once Claude Code has run in them."
        } else {
            "Reading the git state of your session folders…"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(text, dim())).block(card("Projects", ACCENT, false)),
            area,
        );
        return;
    }
    let columns = (area.width / CARD_WIDTH).max(1) as usize;
    app.project_columns = columns;
    let cursor = app.project_cursor.min(cards.len() - 1);
    let rows_total = cards.len().div_ceil(columns);
    let rows_visible = (area.height / CARD_HEIGHT).max(1) as usize;
    // Scroll so the selected card is visible.
    let first_row = (cursor / columns).saturating_sub(rows_visible - 1);

    let row_areas =
        Layout::vertical(vec![Constraint::Length(CARD_HEIGHT); rows_visible]).split(area);
    for (r, row_area) in row_areas.iter().enumerate() {
        let row = first_row + r;
        if row >= rows_total {
            break;
        }
        let cells = Layout::horizontal(vec![Constraint::Ratio(1, columns as u32); columns])
            .spacing(1)
            .split(*row_area);
        for (c, cell) in cells.iter().enumerate() {
            let i = row * columns + c;
            let Some(project) = cards.get(i) else {
                break;
            };
            let selected = i == cursor;
            let block = card(&project.name, ACCENT, selected);
            frame.render_widget(Paragraph::new(card_lines(app, project)).block(block), *cell);
        }
    }
    if rows_total > rows_visible {
        let mut state = ScrollbarState::new(rows_total).position(cursor / columns);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut state,
        );
    }
}

// ---- A project's page ------------------------------------------------------------

fn draw_page(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(page) = &app.project_page else {
        return;
    };
    let dir = page.dir.clone();
    let section = page.section;
    let in_content = page.in_content;
    let sessions = app.project_sessions(&dir).len();
    let specs = app.project_specs(&dir).map_or(0, |s| s.changes.len());
    let checkouts = app.repo_at(&dir).map_or(0, |r| r.checkouts.len());
    let servers = app.project_servers().map_or(0, Vec::len);
    let count = |n: usize| {
        if n > 0 {
            format!(" ({n})")
        } else {
            String::new()
        }
    };
    let entries: Vec<(String, String)> = Section::ALL
        .iter()
        .map(|s| {
            let label = match s {
                Section::Sessions => format!("{}{}", s.title(), count(sessions)),
                Section::Specs => format!("{}{}", s.title(), count(specs)),
                Section::Worktrees => format!("{}{}", s.title(), count(checkouts)),
                Section::Snapshots if app.snapshots_for(&dir) => {
                    format!("{}{}", s.title(), count(app.snapshots.len()))
                }
                Section::Mcp => format!("{}{}", s.title(), count(servers)),
                _ => s.title().to_string(),
            };
            (s.icon().to_string(), label)
        })
        .collect();
    let selected = Section::ALL.iter().position(|s| *s == section).unwrap_or(0);

    let [menu, content] = Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
        .spacing(1)
        .areas(area);
    let name = dir.file_name().map_or_else(
        || paths::display(&dir),
        |n| n.to_string_lossy().into_owned(),
    );
    draw_menu(frame, menu, &name, &entries, selected, !in_content);

    match section {
        Section::Overview => draw_overview(frame, app, content, &dir),
        Section::Sessions => draw_sessions_table(frame, app, content, &dir, in_content),
        Section::Specs => draw_specs(frame, app, content, &dir, in_content),
        Section::Worktrees => draw_worktrees(frame, app, content, &dir, in_content),
        Section::Snapshots => draw_snapshots(frame, app, content, &dir, in_content),
        Section::Mcp => draw_mcp(frame, app, content),
        Section::Setup => draw_ecosystem(frame, app, content),
        Section::Security => draw_project_security(frame, app, content, &dir),
    }
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect, dir: &std::path::Path) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .spacing(1)
            .areas(area);

    // What Claude Code loads here, and git.
    let mut lines = vec![field(
        "Folder",
        10,
        vec![Span::styled(paths::display(dir), Style::new().bold())],
    )];
    lines.push(Line::default());
    lines.extend(project_lines(app));
    if let Some(line) = mcp_summary(app) {
        lines.push(Line::default());
        lines.push(line);
    }
    let sessions = app.project_sessions(dir);
    let [loads, recent] = Layout::vertical([
        Constraint::Length((lines.len() as u16 + 2).min(left.height / 2)),
        Constraint::Min(4),
    ])
    .spacing(1)
    .areas(left);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(card(
            "Claude Code here",
            ACCENT,
            false,
        )),
        loads,
    );
    let mut lines = Vec::new();
    for s in sessions
        .iter()
        .take((recent.height.saturating_sub(2) / 2) as usize)
    {
        let state = match app.activity(&s.id) {
            Some(Activity::NeedsYou) => Span::styled("▲ ", Style::new().fg(Color::Yellow)),
            Some(Activity::Working) => Span::styled("● ", Style::new().fg(Color::Green)),
            Some(Activity::Waiting) => Span::styled("● ", Style::new().fg(Color::Cyan)),
            _ => Span::raw("  "),
        };
        lines.push(Line::from(vec![
            state,
            Span::styled(s.title.clone(), Style::new().bold()),
        ]));
        let mut meta = format!("  {}", sessions::relative_age(s.modified));
        if let Some(branch) = &s.git_branch {
            meta.push_str(&format!(" · {branch}"));
        }
        lines.push(Line::from(Span::styled(meta, dim())));
    }
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled("No sessions yet.", dim())));
    }
    frame.render_widget(
        Paragraph::new(lines).block(card("Recent sessions", ACCENT, false)),
        recent,
    );

    let s = summarize(app, &sessions);
    let [activity, specs_area] = Layout::vertical([Constraint::Length(10), Constraint::Min(4)])
        .spacing(1)
        .areas(right);
    let last = s
        .last
        .map(sessions::relative_age)
        .unwrap_or_else(|| "never".into());
    let lines = vec![
        field(
            "Sessions",
            14,
            vec![Span::styled(s.sessions.to_string(), Style::new().bold())],
        ),
        field(
            "Open now",
            14,
            vec![Span::styled(
                if s.needs_you > 0 {
                    format!("{} · ▲ {} needs you", s.open, s.needs_you)
                } else {
                    s.open.to_string()
                },
                Style::new().fg(if s.needs_you > 0 {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            )],
        ),
        field("Last activity", 14, vec![Span::raw(last)]),
        field(
            "This week",
            14,
            vec![
                Span::raw(format!("{} tokens", human_tokens(s.week_tokens))),
                Span::styled(
                    format!(
                        " · ≈{} API-equivalent",
                        crate::pricing::format_usd(s.week_cost)
                    ),
                    Style::new().fg(Color::Green),
                ),
            ],
        ),
        Line::default(),
        field(
            "Security",
            14,
            if s.secrets + s.risky == 0 {
                vec![Span::styled(
                    "nothing to flag",
                    Style::new().fg(Color::Green),
                )]
            } else {
                vec![Span::styled(
                    format!(
                        "🔑 {} · ⚠ {} risky",
                        plural(s.secrets as u64, "secret"),
                        s.risky
                    ),
                    Style::new().fg(Color::Red),
                )]
            },
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(card("Activity", Color::Green, false)),
        activity,
    );

    let mut lines = Vec::new();
    match app.project_specs(dir) {
        Some(specs) => {
            let names: Vec<&str> = specs.frameworks.iter().map(|f| f.title()).collect();
            lines.push(Line::from(Span::styled(names.join(", "), Style::new().bold())));
            lines.push(Line::default());
            for c in specs.changes.iter().take(6) {
                lines.push(Line::from(vec![
                    Span::raw(format!("{:<22} ", c.id.chars().take(22).collect::<String>())),
                    Span::styled(progress(c.done, c.total), stage_style(c.stage)),
                ]));
            }
            if specs.changes.is_empty() {
                lines.push(Line::from(Span::styled("no open changes", dim())));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "No spec-driven framework (OpenSpec, spec-kit, Kiro, Task Master, GSD) in this project.",
            dim(),
        ))),
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(card(
            "Specs",
            Color::LightBlue,
            false,
        )),
        specs_area,
    );
}

fn progress(done: usize, total: usize) -> String {
    match (done * 10).checked_div(total) {
        Some(filled) => {
            let filled = filled.min(10);
            format!(
                "{}{} {done}/{total}",
                "█".repeat(filled),
                "░".repeat(10 - filled)
            )
        }
        None => "no tasks yet".into(),
    }
}

fn stage_style(stage: Stage) -> Style {
    Style::new().fg(match stage {
        Stage::Complete => Color::Green,
        Stage::Implementing => Color::Yellow,
        Stage::Planning => Color::Cyan,
    })
}

fn activity_cell(app: &App, s: &Session) -> Cell<'static> {
    match app.activity(&s.id) {
        Some(Activity::NeedsYou) => Cell::from("▲ needs you").style(Style::new().fg(Color::Yellow)),
        Some(Activity::Working) => Cell::from("● working").style(Style::new().fg(Color::Green)),
        Some(Activity::Waiting) => Cell::from("● waiting").style(Style::new().fg(Color::Cyan)),
        _ => Cell::from(""),
    }
}

fn draw_sessions_table(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    dir: &std::path::Path,
    focused: bool,
) {
    let sessions = app.project_sessions(dir);
    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| {
            Row::new(vec![
                activity_cell(app, s),
                Cell::from(s.title.clone()).style(Style::new().bold()),
                Cell::from(s.git_branch.clone().unwrap_or_default())
                    .style(Style::new().fg(Color::Cyan)),
                Cell::from(sessions::relative_age(s.modified)).style(dim()),
                Cell::from(human_tokens(s.tokens.total.processed())).style(dim()),
                Cell::from(crate::pricing::format_usd(
                    s.tokens.total.cost + s.tokens.subagent_total.cost,
                ))
                .style(Style::new().fg(Color::Green)),
            ])
            .height(1)
            .bottom_margin(1)
        })
        .collect();
    let empty = rows.is_empty();
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Fill(3),
            Constraint::Fill(1),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["", "Session", "Branch", "Last activity", "Tokens", "≈ USD"])
            .style(Style::new().fg(ACCENT).bold())
            .bottom_margin(1),
    )
    .column_spacing(2)
    .row_highlight_style(if focused {
        Style::new().fg(Color::White).bg(HIGHLIGHT)
    } else {
        Style::new()
    })
    .highlight_symbol(if focused { "▶ " } else { "  " })
    .highlight_spacing(HighlightSpacing::Always)
    .block(card("Sessions", ACCENT, focused));
    if empty {
        frame.render_widget(
            Paragraph::new(Span::styled("No sessions in this project.", dim()))
                .block(card("Sessions", ACCENT, focused)),
            area,
        );
        return;
    }
    if let Some(page) = &mut app.project_page {
        frame.render_stateful_widget(table, area, &mut page.sessions_state);
    }
}

fn draw_specs(frame: &mut Frame, app: &mut App, area: Rect, dir: &std::path::Path, focused: bool) {
    let Some(specs) = app.project_specs(dir).cloned() else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "This project doesn't use a spec-driven framework claudash knows.",
                    dim(),
                )),
                Line::default(),
                Line::from(Span::styled(
                    "Supported: OpenSpec, spec-kit, Kiro and cc-sdd, spec-workflow, Task Master, GSD.",
                    dim(),
                )),
            ])
            .wrap(Wrap { trim: false })
            .block(card("Specs", Color::LightBlue, focused)),
            area,
        );
        return;
    };
    let names: Vec<&str> = specs.frameworks.iter().map(|f| f.title()).collect();
    let block = card(
        &format!("Specs · {}", names.join(", ")),
        Color::LightBlue,
        focused,
    );
    if specs.changes.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No open changes or feature specs.", dim())).block(block),
            area,
        );
        return;
    }
    let width = area.width.saturating_sub(8) as usize;
    let items: Vec<ListItem> = specs
        .changes
        .iter()
        .map(|c| {
            let sessions = app.change_sessions(dir, c);
            let tokens: u64 = sessions.iter().map(|s| s.tokens.total.processed()).sum();
            let mut second = format!("{} · {}", c.framework.title(), c.stage.label());
            if !sessions.is_empty() {
                second.push_str(&format!(
                    " · {} · {} tokens",
                    plural(sessions.len() as u64, "session"),
                    human_tokens(tokens)
                ));
            }
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(format!("{:<28}", c.id), Style::new().bold()),
                    Span::styled(progress(c.done, c.total), stage_style(c.stage)),
                ]),
                Line::from(Span::styled(
                    second.chars().take(width).collect::<String>(),
                    dim(),
                )),
            ];
            if let (Some(next), Some(label)) = (&c.next, c.next_label) {
                lines.push(Line::from(vec![
                    Span::styled("next: ", dim()),
                    Span::raw(format!("{label} ")),
                    Span::styled(next.clone(), Style::new().fg(Color::Yellow)),
                ]));
            }
            lines.push(Line::default());
            ListItem::new(lines)
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().fg(Color::White).bg(HIGHLIGHT)
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▶ " } else { "  " })
        .highlight_spacing(HighlightSpacing::Always);
    frame.render_stateful_widget(list, area, &mut app.specs_state);
}

fn draw_worktrees(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    dir: &std::path::Path,
    focused: bool,
) {
    let Some(repo) = app.repo_at(dir) else {
        frame.render_widget(
            Paragraph::new(Span::styled("This folder isn't a git repository.", dim())).block(card(
                "Worktrees & branches",
                Color::Cyan,
                focused,
            )),
            area,
        );
        return;
    };
    let root = repo.checkouts[0].path.clone();
    let rows: Vec<Row> = repo
        .checkouts
        .iter()
        .map(|co| {
            let place = if co.main {
                "main checkout".to_string()
            } else {
                co.path
                    .strip_prefix(&root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| paths::display(&co.path))
            };
            let mut flags = Vec::new();
            if co.claude_created {
                flags.push("Claude Code");
            }
            if co.review {
                flags.push("review");
            }
            if co.locked {
                flags.push("locked");
            }
            if co.prunable {
                flags.push("missing");
            }
            let git = if co.prunable {
                Line::from(Span::styled("gone", dim()))
            } else {
                Line::from(git_summary(co.status.as_ref()))
            };
            Row::new(vec![
                Cell::from(co.branch.clone().unwrap_or_else(|| "detached".into()))
                    .style(Style::new().bold()),
                Cell::from(place).style(dim()),
                Cell::from(git),
                Cell::from(plural(app.sessions_in(&co.path).len() as u64, "session")).style(dim()),
                Cell::from(flags.join(" · ")).style(Style::new().fg(Color::Magenta)),
            ])
            .bottom_margin(1)
        })
        .collect();
    let [table_area, hint] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(area);
    let table = Table::new(
        rows,
        [
            Constraint::Fill(2),
            Constraint::Fill(2),
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new(["Branch", "Where", "Git", "Sessions", ""])
            .style(Style::new().fg(Color::Cyan).bold())
            .bottom_margin(1),
    )
    .column_spacing(2)
    .row_highlight_style(if focused {
        Style::new().fg(Color::White).bg(HIGHLIGHT)
    } else {
        Style::new()
    })
    .highlight_symbol(if focused { "▶ " } else { "  " })
    .highlight_spacing(HighlightSpacing::Always)
    .block(card("Worktrees & branches", Color::Cyan, focused));
    if let Some(page) = &mut app.project_page {
        frame.render_stateful_widget(table, table_area, &mut page.worktrees_state);
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" b ", Style::new().fg(Color::Black).bg(Color::Gray)),
            Span::styled(
                " review a branch: claudash fetches, checks it out in a worktree of its own and \
                 Claude Code reviews it; findings by file and line",
                dim(),
            ),
        ]))
        .wrap(Wrap { trim: false })
        .block(card("", Color::Cyan, false)),
        hint,
    );
}

fn draw_snapshots(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    dir: &std::path::Path,
    focused: bool,
) {
    let title = "Snapshots: undo what /rewind can't";
    if app.snapshots.is_empty() || !app.snapshots_for(dir) {
        let on = crate::snapshots::enabled();
        let mut lines = vec![
            Line::from(
                "Claude Code's checkpoints don't cover files changed by Bash commands or by \
                 subagents. With snapshots on, claudash's hook copies the project's files to a \
                 shadow git repository of its own before each prompt and after each reply, so \
                 you can see what changed and put files back.",
            ),
            Line::default(),
        ];
        if on {
            lines.push(Line::from(Span::styled(
                "Snapshots are on; none for this project yet. They start with the next prompt \
                 in a session here (git checkouts only).",
                dim(),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::raw("To turn them on, add "),
                Span::styled("snapshots = true", Style::new().fg(ACCENT)),
                Span::raw(" to claudash's config.toml and run "),
                Span::styled("claudash setup --apply", Style::new().fg(ACCENT)),
                Span::raw(" so the hook is installed."),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "The project's own .git is never touched, and files its .gitignore ignores \
             (build output, .env) are not copied.",
            dim(),
        )));
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(card(
                title,
                Color::Cyan,
                focused,
            )),
            area,
        );
        return;
    }
    let several = app
        .snapshots
        .iter()
        .any(|(root, _)| root != &app.snapshots[0].0);
    let rows: Vec<Row> = app
        .snapshots
        .iter()
        .map(|(root, snap)| {
            let session = app
                .sessions
                .iter()
                .find(|s| s.id == snap.session_id)
                .map_or_else(
                    || {
                        if snap.session_id == "claudash" {
                            "claudash".to_string()
                        } else {
                            snap.session_id.chars().take(8).collect()
                        }
                    },
                    |s| s.title.clone(),
                );
            let mut cells = vec![
                Cell::from(crate::snapshots::when(snap.at)).style(dim()),
                Cell::from(snap.label.clone()).style(if snap.label == "before restore" {
                    Style::new().fg(Color::Magenta)
                } else {
                    Style::new().bold()
                }),
                Cell::from(plural(u64::from(snap.files), "file")),
                Cell::from(session).style(dim()),
            ];
            if several {
                cells.push(Cell::from(paths::display(root)).style(dim()));
            }
            Row::new(cells)
        })
        .collect();
    let [table_area, hint] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(area);
    let mut widths = vec![
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Fill(2),
    ];
    let mut header = vec!["When", "", "Changed", "Session"];
    if several {
        widths.push(Constraint::Fill(1));
        header.push("Checkout");
    }
    let table = Table::new(rows, widths)
        .header(
            Row::new(header)
                .style(Style::new().fg(Color::Cyan).bold())
                .bottom_margin(1),
        )
        .column_spacing(2)
        .row_highlight_style(if focused {
            Style::new().fg(Color::White).bg(HIGHLIGHT)
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▶ " } else { "  " })
        .highlight_spacing(HighlightSpacing::Always)
        .block(card(title, Color::Cyan, focused));
    if let Some(page) = &mut app.project_page {
        frame.render_stateful_widget(table, table_area, &mut page.snapshots_state);
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter ", Style::new().fg(Color::Black).bg(Color::Gray)),
            Span::styled(" what changed  ", dim()),
            Span::styled(" U ", Style::new().fg(Color::Black).bg(Color::Gray)),
            Span::styled(
                " put the files back as they were (asks first; the current state is \
                 snapshotted too)",
                dim(),
            ),
        ]))
        .wrap(Wrap { trim: false })
        .block(card("", Color::Cyan, false)),
        hint,
    );
}

fn draw_project_security(frame: &mut Frame, app: &App, area: Rect, dir: &std::path::Path) {
    let sessions = app.project_sessions(dir);
    let mut lines = vec![Line::from(Span::styled(
        "Credentials in this project's transcripts",
        Style::new().fg(ACCENT).bold(),
    ))];
    let mut any = false;
    for s in &sessions {
        for secret in app
            .analysis(s)
            .map(|a| a.secrets.as_slice())
            .unwrap_or_default()
        {
            any = true;
            lines.push(Line::from(vec![
                Span::styled("  🔑 ", Style::new().fg(Color::Red)),
                Span::styled(
                    format!("{} {}", secret.kind, secret.masked),
                    Style::new().fg(Color::Red),
                ),
                Span::styled(format!("  in {} · {}", secret.place, s.title), dim()),
            ]));
        }
    }
    if !any {
        lines.push(Line::from(Span::styled("  none", dim())));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "What Claude did here that deserves a look",
        Style::new().fg(ACCENT).bold(),
    )));
    let mut events: Vec<(&crate::audit::Event, &str)> = sessions
        .iter()
        .filter_map(|s| app.analysis(s).map(|a| (s, a)))
        .flat_map(|(s, a)| a.audit.iter().map(move |e| (e, s.title.as_str())))
        .collect();
    events.sort_by_key(|(e, _)| (e.severity, std::cmp::Reverse(e.at)));
    if events.is_empty() {
        lines.push(Line::from(Span::styled("  nothing risky", dim())));
    }
    // One line each: the command is cut to the room left.
    // Borders and padding take 4 columns.
    let room = (area.width as usize).saturating_sub(4 + 2 + 7 + 34);
    for (e, title) in events.iter().take(40) {
        let color = match e.severity {
            Severity::High => Color::Red,
            Severity::Medium => Color::Yellow,
            Severity::Low => Color::Cyan,
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<7}", e.severity.label()),
                Style::new().fg(color),
            ),
            Span::styled(format!("{:<34}", e.what), Style::new().bold()),
            Span::styled(
                fit_text(
                    &e.detail,
                    room.saturating_sub(title.chars().count().min(24) + 4),
                ),
                Style::new().fg(Color::Gray),
            ),
            Span::styled(format!("  · {}", fit_text(title, 24)), dim()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Permission rules that apply here",
        Style::new().fg(ACCENT).bold(),
    )));
    match &app.eco {
        Some((_, eco)) => {
            let flagged: Vec<_> = eco
                .permissions
                .iter()
                .filter(|r| r.flag.is_some())
                .collect();
            if flagged.is_empty() {
                lines.push(Line::from(Span::styled("  nothing to flag", dim())));
            }
            for r in flagged {
                let (severity, why) = r.flag.clone().unwrap_or((Severity::Low, String::new()));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<7}", severity.label()),
                        Style::new().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("{} {}", r.kind.label(), r.rule),
                        Style::new().bold(),
                    ),
                    Span::styled(format!("  {} · {why}", r.scope), dim()),
                ]));
            }
        }
        None => lines.push(Line::from(Span::styled("  reading settings…", dim()))),
    }
    let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
    let scroll = app.project_page.as_ref().map_or(0, |p| p.scroll).min(max);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(card("Security", Color::Red, false)),
        area,
    );
}

/// `text` cut to `width` characters, with "…" when it's cut.
fn fit_text(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut s: String = text.chars().take(width.saturating_sub(1)).collect();
    s.push('…');
    s
}
