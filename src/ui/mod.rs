//! Rendering.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration as Days, Local, NaiveDate, TimeZone};
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Bar, BarChart, BarGroup, Block, BorderType, Borders, Clear, Gauge, HighlightSpacing, List,
        ListItem, ListState, Padding, Paragraph, Row, Table, Tabs, Wrap,
    },
};

use crate::{
    app::{
        App, CLEANUP_PRESETS, EcoTab, Input, McpSnapshot, Popup, TranscriptRow, TranscriptView,
        View,
    },
    ecosystem::Item,
    hooks::Activity,
    mcp::McpStatus,
    paths,
    sessions::{self, Usage, human_tokens},
    statusline::{self, Forecast, Window},
    transcript::{self, Kind},
};

mod insights;
mod now;
mod projects;

const HIGHLIGHT: Color = Color::Rgb(60, 40, 70);
/// The empty part of a gauge.
const GAUGE_TRACK: Color = Color::Rgb(40, 40, 40);
/// Accent of claudash's own chrome: tabs, menus, selected cards.
const ACCENT: Color = Color::LightMagenta;

/// A card: rounded borders and room inside. Selected cards get a heavier,
/// colored border.
fn card<'a>(title: &str, color: Color, selected: bool) -> Block<'a> {
    let block = Block::bordered()
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(if selected {
            Style::new().fg(color)
        } else {
            Style::new().fg(Color::DarkGray)
        })
        .padding(Padding::horizontal(1));
    if title.is_empty() {
        block
    } else {
        block.title(Span::styled(
            format!(" {title} "),
            Style::new().fg(color).bold(),
        ))
    }
}

/// A vertical menu: icon and label per entry, the selected one highlighted;
/// dimmed when the keys are elsewhere.
fn draw_menu(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    entries: &[(String, String)],
    selected: usize,
    focused: bool,
) {
    let items: Vec<ListItem> = entries
        .iter()
        .map(|(icon, label)| {
            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(format!(" {icon}  ")),
                    Span::raw(label.clone()),
                ]),
                Line::default(),
            ])
        })
        .collect();
    let highlight = if focused {
        Style::new().fg(Color::White).bg(HIGHLIGHT).bold()
    } else {
        Style::new().fg(ACCENT).bold()
    };
    let list = List::new(items)
        .block(card(title, ACCENT, focused))
        .highlight_style(highlight)
        .highlight_symbol("▌")
        .highlight_spacing(HighlightSpacing::Always);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Label and value on one line, the label dimmed and padded to `width`.
fn field(label: &str, width: usize, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<width$}"), dim())];
    spans.extend(value);
    Line::from(spans)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);
    match app.view {
        View::Sessions => draw_dashboard(frame, app, body),
        View::Now => now::draw(frame, app, body),
        View::Projects => projects::draw(frame, app, body),
        View::Insights => insights::draw(frame, app, body),
        View::Logs => draw_logs(frame, app, body),
        View::Transcript => draw_transcript(frame, app, body),
        View::Help => draw_help(frame, app, body),
        View::Inspect => draw_inspect(frame, app, body),
    }
    draw_footer(frame, app, footer);
    draw_popup(frame, app);
    if app.no_color {
        strip_colors(frame.buffer_mut());
    }
}

/// NO_COLOR: drops every color but keeps what color meant where it matters.
/// Anything drawn on a background (selections, key labels, gauges) is shown
/// reversed instead; bold, dim and the rest stay.
fn strip_colors(buffer: &mut ratatui::buffer::Buffer) {
    for cell in buffer.content.iter_mut() {
        // A gauge's track stays blank; its filled part is drawn with blocks.
        if cell.bg != Color::Reset && cell.bg != GAUGE_TRACK {
            cell.modifier.insert(Modifier::REVERSED);
        }
        cell.fg = Color::Reset;
        cell.bg = Color::Reset;
    }
}

/// A bordered panel; an empty `title` leaves room for a custom `.title(...)`.
fn panel(title: &str, accent: Color) -> Block<'_> {
    let block = Block::default();
    let block = if title.is_empty() {
        block
    } else {
        block.title(Line::from(format!(" {title} ")).bold().fg(accent))
    };
    block
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .padding(Padding::horizontal(1))
}

/// "1 use", "3 uses".
fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// "~1,067" (from `claude plugin details`) -> 1067.
fn parse_tokens(text: &str) -> u64 {
    text.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// `mcp__plugin_chrome-devtools-mcp_chrome-devtools__take_screenshot` ->
/// `chrome-devtools › take_screenshot`; other names unchanged.
fn tool_label(name: &str) -> String {
    match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
        Some((server, tool)) => {
            let server = server.rsplit('_').next().unwrap_or(server);
            format!("{server} › {tool}")
        }
        None => name.to_string(),
    }
}

fn focused(block: Block<'_>, is_focused: bool) -> Block<'_> {
    if is_focused {
        block
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(ACCENT))
    } else {
        block
    }
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn message<'a>(text: impl Into<String>, block: Block<'a>) -> Paragraph<'a> {
    Paragraph::new(text.into())
        .style(dim().italic())
        .wrap(Wrap { trim: true })
        .block(block)
}

/// Green below 60%, yellow below 85%, red above.
fn level_color(ratio: f64) -> Color {
    match ratio {
        r if r < 0.6 => Color::Green,
        r if r < 0.85 => Color::Yellow,
        _ => Color::Red,
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(10)]).areas(area);
    let mut spans = vec![
        Span::styled(
            " ◆ claudash ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw("  "),
    ];
    // The view a sub-view (conversation, inspector, logs, help) belongs to.
    let home = match app.view {
        View::Transcript | View::Inspect | View::Logs | View::Help => None,
        view => Some(view),
    };
    for (n, view) in crate::app::VIEW_KEYS.into_iter().enumerate() {
        let style = if home == Some(view) {
            Style::new().fg(Color::White).bg(HIGHLIGHT).bold()
        } else {
            dim()
        };
        spans.push(Span::styled(format!(" {} {} ", n + 1, view.title()), style));
        spans.push(Span::raw(" "));
    }
    // Where you are inside a view.
    let mut crumbs: Vec<String> = Vec::new();
    if app.view == View::Projects
        && let Some(page) = &app.project_page
    {
        crumbs.push(page.dir.file_name().map_or_else(
            || paths::display(&page.dir),
            |n| n.to_string_lossy().into_owned(),
        ));
        crumbs.push(page.section.title().to_string());
    }
    if home.is_none() {
        crumbs.push(app.view.title().to_string());
    }
    for crumb in crumbs {
        spans.push(Span::styled(" › ", dim()));
        spans.push(Span::styled(crumb, Style::new().fg(ACCENT).bold()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), left);
    frame.render_widget(
        Paragraph::new(Line::from("? help ").dark_gray().right_aligned()),
        right,
    );
}

// ---- Dashboard ---------------------------------------------------------------

fn draw_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let [top, tokens] = Layout::vertical([Constraint::Min(8), Constraint::Length(8)]).areas(area);
    let [sessions, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(top);
    let mut project_lines = project_lines(app);
    if let Some(line) = mcp_summary(app) {
        project_lines.push(Line::default());
        project_lines.push(line);
    }
    project_lines.push(Line::default());
    project_lines.push(Line::from(Span::styled(
        "Tab opens this project: MCP servers, specs, worktrees, setup",
        dim().italic(),
    )));

    draw_sessions(frame, app, sessions);
    draw_project(frame, app, right, project_lines);
    draw_tokens(frame, app, tokens);
}

fn draw_sessions(frame: &mut Frame, app: &mut App, area: Rect) {
    let count = if app.filter.is_empty() {
        format!(" {} sessions ", app.sessions.len())
    } else {
        format!(" {}/{} sessions ", app.visible.len(), app.sessions.len())
    };
    let mut block =
        panel("Sessions", Color::LightMagenta).title_bottom(Line::from(count).right_aligned());
    block = focused(block, true);
    if !app.hooks_configured || !app.statusline.configured {
        block = block.title_bottom(
            Line::from(" run `claudash setup` for alerts and plan usage ").dark_gray(),
        );
    }
    if !app.filter.is_empty() {
        block = block.title(Line::from(format!(" filter: {} ", app.filter)).fg(Color::Yellow));
    }
    if let Some(dir) = &app.folder_filter {
        block = block.title(
            Line::from(format!(" folder: {} · Esc shows all ", paths::display(dir)))
                .fg(Color::Yellow),
        );
    }

    if app.visible.is_empty() {
        let msg = match &app.sessions_error {
            Some(e) => e.clone(),
            None if !app.sessions.is_empty() => "No sessions match the filter.".into(),
            None => "No sessions in ~/.claude/projects yet.".into(),
        };
        frame.render_widget(message(msg, block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| &app.sessions[i])
        .map(|s| {
            let meta = app.library.get(&s.id);
            let mut title = Vec::new();
            if meta.is_some_and(|m| m.starred) {
                title.push(Span::styled("★ ", Style::new().fg(Color::Yellow)));
            }
            title.push(Span::styled(s.title.as_str(), Style::new().bold()));
            for tag in meta.map(|m| m.tags.as_slice()).unwrap_or_default() {
                title.push(Span::styled(
                    format!("  #{tag}"),
                    Style::new().fg(Color::Magenta),
                ));
            }
            let marker = match app.activity(&s.id) {
                Some(Activity::NeedsYou) => Some(("  ▲ needs you", Color::Yellow)),
                Some(Activity::Working) => Some(("  ● working", Color::Green)),
                Some(Activity::Waiting) => Some(("  ● waiting", Color::Cyan)),
                Some(Activity::Ended) | None => None,
            };
            if let Some((text, color)) = marker {
                title.push(Span::styled(text, Style::new().fg(color).bold()));
            }
            if app.live.get(&s.id).is_some_and(|l| l.is_background()) {
                title.push(Span::styled("  bg", dim()));
            }
            if app.live.contains_key(&s.id)
                && s.cwd.as_deref().is_some_and(|d| app.open_in_folder(d) >= 2)
            {
                title.push(Span::styled(
                    "  ⚠ shared folder",
                    Style::new().fg(Color::Red),
                ));
            }
            if app.analysis(s).is_some_and(|a| !a.secrets.is_empty()) {
                title.push(Span::styled("  🔑 secret", Style::new().fg(Color::Red)));
            }
            let mut detail = vec![Span::styled(
                format!("  {}", s.project_path),
                Style::new().fg(Color::Gray),
            )];
            if let Some(branch) = &s.git_branch {
                detail.push(Span::styled(
                    format!("  {branch}"),
                    Style::new().fg(Color::Cyan),
                ));
            }
            detail.push(Span::styled(
                format!("  · {}", sessions::relative_age(s.modified)),
                dim().italic(),
            ));
            let cost = s.tokens.total.cost + s.tokens.subagent_total.cost;
            if cost >= 0.01 {
                detail.push(Span::styled(
                    format!("  ≈{}", crate::pricing::format_usd(cost)),
                    Style::new().fg(Color::Green),
                ));
            }
            let mut lines = vec![Line::from(title), Line::from(detail)];
            if let Some(note) = meta.map(|m| m.note.as_str()).filter(|n| !n.is_empty()) {
                lines.push(Line::from(Span::styled(
                    format!("  ✎ {note}"),
                    dim().italic(),
                )));
            }
            lines.push(Line::default());
            ListItem::new(lines)
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::new()
                .fg(Color::White)
                .bg(HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always);
    frame.render_stateful_widget(list, area, &mut app.session_state);
}

/// Instruction files Claude Code loads for the selected project.
fn project_lines(app: &App) -> Vec<Line<'static>> {
    let Some(project) = &app.project else {
        let msg = app.selected_project_dir().err().unwrap_or_default();
        return vec![Line::from(msg).dark_gray().italic()];
    };
    let mut lines: Vec<Line> = project
        .instructions
        .files
        .iter()
        .map(|file| {
            let name = match file.path.strip_prefix(&project.cwd) {
                Ok(rel) => rel.display().to_string(),
                Err(_) => paths::display(&file.path),
            };
            let mut spans = match file.skipped {
                None => vec![
                    Span::styled("● ", Style::new().fg(Color::Green)),
                    Span::raw(name),
                ],
                Some(_) => vec![
                    Span::styled("○ ", dim()),
                    Span::styled(name, dim().crossed_out()),
                ],
            };
            spans.push(Span::styled(format!("  {}", file.scope.label()), dim()));
            if let Some(reason) = file.skipped {
                spans.push(Span::styled(
                    format!("  ({reason})"),
                    Style::new().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        })
        .collect();
    if !project.instructions.has_project_instructions() {
        lines.push(Line::from(vec![
            Span::styled("○ ", Style::new().fg(Color::Yellow)),
            Span::styled(
                "No CLAUDE.md or AGENTS.md · run /init",
                Style::new().fg(Color::Yellow),
            ),
        ]));
    }
    if let Some(git) = app.git_status(&project.cwd) {
        let mut spans = vec![
            Span::styled("⎇ ", Style::new().fg(Color::Cyan)),
            Span::raw(git.branch.clone().unwrap_or_else(|| "detached HEAD".into())),
        ];
        if git.linked_worktree {
            spans.push(Span::styled("  worktree", dim()));
        }
        if git.changed > 0 {
            spans.push(Span::styled(
                format!("  {} changed", git.changed),
                Style::new().fg(Color::Yellow),
            ));
        } else {
            spans.push(Span::styled("  clean", Style::new().fg(Color::Green)));
        }
        if git.ahead > 0 {
            spans.push(Span::styled(
                format!("  ↑{} to push", git.ahead),
                Style::new().fg(Color::Cyan),
            ));
        }
        if git.behind > 0 {
            spans.push(Span::styled(
                format!("  ↓{} to pull", git.behind),
                Style::new().fg(Color::Magenta),
            ));
        }
        lines.push(Line::from(spans));
    }
    if let Some((tokens, n)) = app.baseline_context(&project.cwd) {
        lines.push(Line::from(vec![
            Span::styled("◔ ", dim()),
            Span::raw(format!("~{} tokens at start", human_tokens(tokens))),
            Span::styled(format!("  (median of {n})"), dim()),
        ]));
    }
    let open = app.open_in_folder(&project.cwd);
    if open >= 2 {
        lines.push(Line::from(Span::styled(
            format!("⚠ {open} open sessions share this folder: their edits can collide"),
            Style::new().fg(Color::Red).bold(),
        )));
    }
    lines
}

fn draw_project(frame: &mut Frame, app: &App, area: Rect, lines: Vec<Line<'static>>) {
    let mut block = panel("Project", Color::Green);
    if let Some(project) = &app.project {
        block = block.title(
            Line::from(format!(" {} ", paths::display(&project.cwd)))
                .fg(Color::DarkGray)
                .right_aligned(),
        );
        let mode = project.instructions.mode;
        if mode != crate::instructions::Mode::default() {
            block = block.title_bottom(
                Line::from(format!(" instructions: {} ", mode.as_str())).right_aligned(),
            );
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// "MCP  3/5 online · 1 needs sign-in · 1 failed" for the selected project.
fn mcp_summary(app: &App) -> Option<Line<'static>> {
    let servers = app.project_servers()?;
    if servers.is_empty() {
        return None;
    }
    let online = servers
        .iter()
        .filter(|s| matches!(s.status, McpStatus::Connected))
        .count();
    let auth = servers
        .iter()
        .filter(|s| matches!(s.status, McpStatus::NeedsAuth))
        .count();
    let failed = servers
        .iter()
        .filter(|s| matches!(s.status, McpStatus::Failed(_)))
        .count();
    let mut spans = vec![
        Span::styled("MCP  ", dim()),
        Span::styled(
            format!("{online}/{} online", servers.len()),
            Style::new().fg(Color::Green),
        ),
    ];
    if auth > 0 {
        spans.push(Span::styled(
            format!(" · {auth} need sign-in"),
            Style::new().fg(Color::Yellow),
        ));
    }
    if failed > 0 {
        spans.push(Span::styled(
            format!(" · {failed} failed"),
            Style::new().fg(Color::Red),
        ));
    }
    Some(Line::from(spans))
}

fn draw_mcp(frame: &mut Frame, app: &mut App, area: Rect) {
    let mcp_focus = app
        .project_page
        .as_ref()
        .is_some_and(|p| p.section == crate::app::Section::Mcp && p.in_content);
    let block = card("MCP servers", Color::Cyan, mcp_focus);
    let Some(project) = &app.project else {
        let msg = message(
            "Select a session to check its project's MCP servers.",
            block,
        );
        frame.render_widget(msg, area);
        return;
    };
    let checking = app.mcp_checking(&project.cwd);
    let snapshot = app.mcp_cache.get(&project.cwd);
    let spinner = app.spinner();

    let footer = match (checking, snapshot) {
        (true, _) => format!(" {spinner} checking… "),
        (
            false,
            Some(McpSnapshot {
                result: Ok(servers),
                checked_at,
            }),
        ) => {
            let online = servers
                .iter()
                .filter(|s| s.status == McpStatus::Connected)
                .count();
            format!(
                " {online}/{} online · {}s ago ",
                servers.len(),
                checked_at.elapsed().as_secs()
            )
        }
        _ => String::new(),
    };
    let block = block.title_bottom(Line::from(footer).right_aligned());

    let servers = match snapshot.map(|s| &s.result) {
        Some(Ok(servers)) => servers,
        Some(Err(error)) => {
            let msg = Paragraph::new(error.as_str())
                .style(Style::new().fg(Color::Red))
                .wrap(Wrap { trim: true })
                .block(block);
            frame.render_widget(msg, area);
            return;
        }
        None => {
            let text = if checking {
                "Running `claude mcp list` in this project…"
            } else {
                "Waiting to check this project…"
            };
            frame.render_widget(message(format!("{spinner} {text}"), block), area);
            return;
        }
    };
    if servers.is_empty() {
        frame.render_widget(message("No MCP servers configured.", block), area);
        return;
    }

    let items: Vec<ListItem> = servers
        .iter()
        .map(|s| {
            let (dot, label, color) = match &s.status {
                McpStatus::Connected => ("●", "ONLINE", Color::Green),
                McpStatus::NeedsAuth => ("◐", "AUTH", Color::Yellow),
                McpStatus::Failed(_) => ("✘", "ERROR", Color::Red),
                McpStatus::NotConfigured => ("○", "NOT SET", Color::DarkGray),
                McpStatus::Unknown(_) => ("?", "UNKNOWN", Color::Gray),
            };
            let name: String = s.name.chars().take(18).collect();
            ListItem::new(Line::from(vec![
                Span::styled(format!("{dot} "), Style::new().fg(color)),
                Span::raw(format!("{name:<19}")),
                Span::styled(format!("{label:<11}"), Style::new().fg(color).bold()),
                Span::styled(format!(" {}", s.source), dim()),
            ]))
        })
        .collect();
    let mut list = List::new(items).block(block);
    if mcp_focus {
        list = list
            .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
            .highlight_symbol("▶ ")
            .highlight_spacing(HighlightSpacing::Always);
    }
    frame.render_stateful_widget(list, area, &mut app.mcp_state);
}

/// Context window usage of the selected session, its totals and plan usage.
fn draw_tokens(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("Token Usage", Color::Yellow);
    let Some(session) = app.selected_session() else {
        frame.render_widget(message("Select a session to see its usage.", block), area);
        return;
    };
    let t = &session.tokens;
    let block = block.title(
        Line::from(format!(" {} ", session.title))
            .fg(Color::DarkGray)
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [gauge_area, stats_area, cache_area, plan_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // The status line reports the real window size; otherwise use the setting.
    let reported = app
        .statusline
        .sessions
        .get(&session.id)
        .and_then(|s| s.context_window.context_window_size)
        .filter(|&size| size > 0);
    let limit = reported.unwrap_or(app.context_limit);
    let ratio = t.context_used as f64 / limit as f64;
    let label = format!(
        "Context {:.1}%  ({} / {}{})",
        ratio * 100.0,
        human_tokens(t.context_used),
        human_tokens(limit),
        if reported.is_some() {
            ", from status line"
        } else {
            ""
        },
    );
    let gauge = Gauge::default()
        .gauge_style(Style::new().fg(level_color(ratio)).bg(GAUGE_TRACK))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(Span::styled(label, Style::new().fg(Color::White).bold()))
        .use_unicode(true);
    frame.render_widget(gauge, gauge_area);

    let value = Style::new().fg(Color::White).bold();
    let stat = |name: &'static str, n: u64| {
        [
            Span::styled(name, dim()),
            Span::styled(human_tokens(n), value),
            Span::raw("   "),
        ]
    };
    let mut stats: Vec<Span> = [
        stat("input ", t.total.input_tokens),
        stat("cache write ", t.total.cache_creation_input_tokens),
        stat("cache read ", t.total.cache_read_input_tokens),
        stat("output ", t.total.output_tokens),
    ]
    .into_iter()
    .flatten()
    .collect();
    // API-equivalent, subagents included; Claude Code's own figure when that's all there is.
    let cost = t.total.cost + t.subagent_total.cost;
    if cost > 0.0 {
        stats.push(Span::styled("cost ", dim()));
        stats.push(Span::styled(
            crate::pricing::format_usd(cost),
            value.fg(Color::Green),
        ));
        stats.push(Span::styled(" API-equivalent", dim()));
    } else if let Some(cost) = t.cost_usd {
        stats.push(Span::styled("cost ", dim()));
        stats.push(Span::styled(format!("${cost:.2}"), value.fg(Color::Green)));
    }
    let mut model_spans = vec![
        Span::styled("model ", dim()),
        Span::styled(
            t.model.as_deref().unwrap_or("—").to_string(),
            Style::new().fg(Color::Cyan),
        ),
    ];
    if t.subagents > 0 {
        model_spans.push(Span::styled("   subagents ", dim()));
        model_spans.push(Span::styled(
            format!(
                "{} · {}",
                t.subagents,
                human_tokens(t.subagent_total.processed())
            ),
            value,
        ));
    }
    let model = Line::from(model_spans);
    frame.render_widget(Paragraph::new(vec![Line::from(stats), model]), stats_area);
    frame.render_widget(Paragraph::new(cache_line(app, session)), cache_area);
    frame.render_widget(Paragraph::new(plan_line(app)), plan_area);
}

/// Current plan windows reported by the status line: (label, window).
fn plan_windows(app: &App) -> Vec<(&'static str, Window)> {
    let Some((limits, _)) = &app.statusline.rate_limits else {
        return Vec::new();
    };
    [("5h", limits.five_hour), ("7d", limits.seven_day)]
        .into_iter()
        .filter_map(|(label, w)| w.filter(Window::is_current).map(|w| (label, w)))
        .collect()
}

fn reset_time(window: &Window) -> String {
    local_time(window.resets_at)
}

/// "14:20" today, "Sat 09:00" on another day.
fn local_time(epoch: i64) -> String {
    let Some(at) = Local.timestamp_opt(epoch, 0).single() else {
        return String::new();
    };
    if at.date_naive() == Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%a %H:%M").to_string()
    }
}

/// "limit ~15:40" when the current pace reaches 100% before the reset.
fn forecast_span(app: &App, label: &str) -> Option<Span<'static>> {
    let (pick, lookback): (statusline::Pick, i64) = match label {
        "5h" => (|s| s.five_hour, 3_600),
        _ => (|s| s.seven_day, 24 * 3_600),
    };
    let now = chrono::Utc::now().timestamp();
    match statusline::forecast(&app.statusline.samples, pick, lookback, now)? {
        Forecast::LimitAt(at) => Some(Span::styled(
            format!(" · limit ~{} at this pace", local_time(at)),
            Style::new().fg(Color::Red).bold(),
        )),
        Forecast::Safe => Some(Span::styled(" · on pace", Style::new().fg(Color::Green))),
    }
}

/// Cache reuse for the session, plus Claude Code's live cache diagnostics when
/// the status line has reported them.
/// Seconds until a session's cached context expires (negative once it has),
/// and what the next prompt would then re-write: tokens and API cost.
fn cache_clock(app: &App, s: &sessions::Session) -> Option<(i64, u64, f64)> {
    let a = app.analysis(s)?;
    let (last, ttl) = (a.last_request?, a.cache_ttl?);
    let left = ttl - (Local::now() - last).num_seconds();
    let tokens = a.last_context;
    let cost = s
        .tokens
        .model
        .as_deref()
        .and_then(crate::pricing::lookup)
        .map_or(0.0, |p| {
            let write = if ttl >= 3_600 { p.write_1h } else { p.write_5m };
            tokens as f64 * write / 1e6
        });
    Some((left, tokens, cost))
}

/// "cache warm 12m" or "cache cold: next prompt re-caches 412k ≈$3.30".
fn cache_clock_span(app: &App, s: &sessions::Session) -> Option<Span<'static>> {
    let (left, tokens, cost) = cache_clock(app, s)?;
    if tokens < 20_000 {
        return None;
    }
    Some(if left > 0 {
        let minutes = (left + 59) / 60;
        Span::styled(
            format!("  · cache warm {minutes}m"),
            Style::new().fg(if minutes <= 2 {
                Color::Yellow
            } else {
                Color::Green
            }),
        )
    } else {
        Span::styled(
            format!(
                "  · cache cold: next prompt re-caches {} ≈{}",
                human_tokens(tokens),
                crate::pricing::format_usd(cost)
            ),
            Style::new().fg(Color::Yellow),
        )
    })
}

fn cache_line(app: &App, session: &sessions::Session) -> Line<'static> {
    let t = &session.tokens.total;
    let total = t.input_tokens + t.cache_creation_input_tokens + t.cache_read_input_tokens;
    let mut spans = vec![Span::styled("cache ", dim())];
    if total == 0 {
        spans.push(Span::styled("no data", dim()));
        return Line::from(spans);
    }
    let snapshot_cache = app
        .statusline
        .sessions
        .get(&session.id)
        .and_then(|s| s.prompt_cache.as_ref());
    // Claude Code's own figure when the status line reported it.
    let hit = snapshot_cache
        .and_then(|c| c.hit_ratio)
        .unwrap_or(t.cache_read_input_tokens as f64 / total as f64);
    let color = match hit {
        h if h >= 0.8 => Color::Green,
        h if h >= 0.5 => Color::Yellow,
        _ => Color::Red,
    };
    spans.push(Span::styled(
        format!("hit {:.0}%", hit * 100.0),
        Style::new().fg(color).bold(),
    ));
    if hit < 0.5 && t.cache_creation_input_tokens > 100_000 {
        spans.push(Span::styled(
            "  low reuse: cache writes cost more than reads",
            Style::new().fg(Color::Yellow),
        ));
    }
    if let Some(a) = app.analysis(session)
        && !a.cold_restarts.is_empty()
    {
        let cost: f64 = a.cold_restarts.iter().map(|r| r.cost).sum();
        spans.push(Span::styled(
            format!(
                "  · {} cold restart(s) ≈{}",
                a.cold_restarts.len(),
                crate::pricing::format_usd(cost)
            ),
            Style::new().fg(Color::Yellow),
        ));
    }
    let Some(cache) = snapshot_cache else {
        // Without the status line, claudash's own estimate from the transcript.
        spans.extend(cache_clock_span(app, session));
        return Line::from(spans);
    };
    let now = chrono::Utc::now().timestamp();
    match cache.expires_at {
        Some(at) if at > now => spans.push(Span::styled(
            format!("  · warm until {}", local_time(at)),
            Style::new().fg(Color::Green),
        )),
        _ => {
            spans.push(Span::styled("  · cold", Style::new().fg(Color::Yellow)));
            if let Some(tokens) = cache.recache_tokens_if_cold.filter(|&t| t > 0) {
                spans.push(Span::styled(
                    format!(" (next reply re-caches {})", human_tokens(tokens)),
                    dim(),
                ));
            }
        }
    }
    if let Some(ttl) = &cache.ttl {
        spans.push(Span::styled(format!(" · ttl {ttl}"), dim()));
    }
    if let Some(misses) = cache.misses.filter(|&m| m > 0) {
        let cause = cache
            .last_miss_cause
            .as_ref()
            .and_then(|c| c.causes.first())
            .map(|c| format!(", last: {c}"))
            .unwrap_or_default();
        spans.push(Span::styled(
            format!(" · {misses} miss(es){cause}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    Line::from(spans)
}

fn plan_line(app: &App) -> Line<'static> {
    let windows = plan_windows(app);
    if windows.is_empty() {
        let hint = if app.statusline.configured {
            "plan usage: not reported yet (Pro/Max only, after a session's first reply)"
        } else {
            "plan usage: run `claudash setup` to enable"
        };
        return Line::from(Span::styled(hint, dim().italic()));
    }
    let mut spans = vec![Span::styled("plan ", dim())];
    for (label, w) in windows {
        let color = level_color(w.used_percentage / 100.0);
        spans.push(Span::styled(format!("{label} "), dim()));
        spans.push(Span::styled(
            format!("{:.0}%", w.used_percentage),
            Style::new().fg(color).bold(),
        ));
        spans.push(Span::styled(format!(" (resets {})", reset_time(&w)), dim()));
        spans.extend(forecast_span(app, label));
        spans.push(Span::raw("   "));
    }
    Line::from(spans)
}

// ---- Ecosystem ---------------------------------------------------------------

fn draw_ecosystem(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
    let titles: Vec<String> = EcoTab::ALL
        .iter()
        .map(|&tab| format!("{} ({})", tab.title(), app.eco_len(tab)))
        .collect();
    let selected = EcoTab::ALL
        .iter()
        .position(|&t| t == app.eco_tab)
        .unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(dim())
        .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT).bold())
        .divider(" ");
    frame.render_widget(tabs, tabs_area);

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let mut scope = match &app.project {
        Some(p) => format!(
            " uses: last {} days · user + {} ",
            crate::app::USAGE_WINDOW_DAYS,
            paths::display(&p.cwd)
        ),
        None => " user scope ".to_string(),
    };
    if app.eco_loading() && app.eco.is_some() {
        scope = format!(" {} reloading ·{scope}", app.spinner());
    }
    let block =
        panel(app.eco_tab.title(), Color::Magenta).title_bottom(Line::from(scope).right_aligned());

    let Some((_, eco)) = &app.eco else {
        let text = format!(
            "{} Loading skills, agents, hooks and plugins…",
            app.spinner()
        );
        frame.render_widget(message(text, block), list_area);
        frame.render_widget(panel("Details", Color::Magenta), detail_area);
        return;
    };

    let tab_index = selected;
    if app.eco_tab == EcoTab::Plugins {
        if let Some(error) = &eco.plugin_error {
            frame.render_widget(message(error.clone(), block), list_area);
            frame.render_widget(panel("Details", Color::Magenta), detail_area);
            return;
        }
        let toggling = app.toggling_plugin();
        let usage = app.recent_usage();
        // What enabled plugins nobody used cost in every session.
        let wasted: u64 = eco
            .plugins
            .iter()
            .filter(|p| p.enabled && usage.plugin(p.short_name()) == 0)
            .filter_map(|p| app.plugin_details.get(&p.id)?.as_ref().ok())
            .filter_map(|d| crate::ecosystem::always_on_tokens(d))
            .map(|t| parse_tokens(&t))
            .sum();
        let block = if wasted > 0 && !app.analyzing() {
            block.title_bottom(
                Line::from(Span::styled(
                    format!(
                        " plugins unused in {} days cost ~{} tokens per session ",
                        crate::app::USAGE_WINDOW_DAYS,
                        human_tokens(wasted)
                    ),
                    Style::new().fg(Color::Red),
                ))
                .left_aligned(),
            )
        } else {
            block
        };
        let items: Vec<ListItem> = eco
            .plugins
            .iter()
            .map(|p| {
                let (dot, color) = if p.enabled {
                    ("●", Color::Green)
                } else {
                    ("○", Color::DarkGray)
                };
                let mut spans = vec![
                    Span::styled(format!("{dot} "), Style::new().fg(color)),
                    Span::styled(p.short_name().to_string(), Style::new().bold()),
                    Span::styled(format!("  {}", p.version), dim()),
                ];
                let tokens = app
                    .plugin_details
                    .get(&p.id)
                    .and_then(|d| d.as_ref().ok())
                    .and_then(|d| crate::ecosystem::always_on_tokens(d));
                if let Some(tokens) = &tokens {
                    spans.push(Span::styled(
                        format!("  {tokens} tok/session"),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                let uses = usage.plugin(p.short_name());
                // Enabled, costs tokens in every session, and nothing of it was used.
                let costly = tokens.as_deref().is_some_and(|t| t != "~0");
                if uses > 0 {
                    spans.push(Span::styled(
                        format!("  {}", plural(uses as u64, "use")),
                        dim(),
                    ));
                } else if p.enabled && costly && !app.analyzing() {
                    spans.push(Span::styled("  unused", Style::new().fg(Color::Red).bold()));
                }
                if toggling == Some(p.id.as_str()) {
                    spans.push(Span::styled(format!("  {}", app.spinner()), dim()));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        render_list(
            frame,
            items,
            block,
            list_area,
            &mut app.eco_states[tab_index],
        );
        draw_plugin_details(frame, app, detail_area);
        return;
    }

    if app.eco_tab == EcoTab::Permissions {
        let rules = eco.permissions.clone();
        if rules.is_empty() {
            frame.render_widget(
                message(
                    "No permission rules: Claude Code asks before anything that needs it.",
                    block,
                ),
                list_area,
            );
            frame.render_widget(panel("Details", Color::Magenta), detail_area);
            return;
        }
        let severity_color = |s: crate::audit::Severity| match s {
            crate::audit::Severity::High => Color::Red,
            crate::audit::Severity::Medium => Color::Yellow,
            crate::audit::Severity::Low => Color::Cyan,
        };
        let items: Vec<ListItem> = rules
            .iter()
            .map(|r| {
                let mut spans = vec![
                    Span::styled(format!("{:<6}", r.kind.label()), dim()),
                    Span::styled(r.rule.clone(), Style::new().bold()),
                    Span::styled(format!("  {}", r.scope), dim()),
                ];
                if let Some((severity, _)) = &r.flag {
                    spans.push(Span::styled(
                        format!("  ⚠ {}", severity.label()),
                        Style::new().fg(severity_color(*severity)),
                    ));
                }
                if r.kind == crate::permissions::Kind::Allow
                    && app.rule_used(&r.rule) == Some(false)
                {
                    spans.push(Span::styled("  unused", dim()));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let selected = app.eco_states[tab_index]
            .selected()
            .and_then(|i| rules.get(i))
            .cloned();
        render_list(
            frame,
            items,
            block,
            list_area,
            &mut app.eco_states[tab_index],
        );
        let mut lines = Vec::new();
        if let Some(r) = selected {
            lines.push(Line::from(Span::styled(
                r.rule.clone(),
                Style::new().bold(),
            )));
            lines.push(Line::from(Span::styled(
                format!("{} · {} settings", r.kind.label(), r.scope),
                dim(),
            )));
            lines.push(Line::from(Span::styled(paths::display(&r.file), dim())));
            lines.push(Line::default());
            match &r.flag {
                Some((severity, why)) => lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {} ", severity.label()),
                        Style::new().fg(Color::Black).bg(severity_color(*severity)),
                    ),
                    Span::raw(format!("  {why}")),
                ])),
                None => lines.push(Line::from(Span::styled("Nothing to flag.", dim()))),
            }
            if r.kind == crate::permissions::Kind::Allow {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    match app.rule_used(&r.rule) {
                        Some(true) => {
                            format!("Used in the last {} days.", crate::app::USAGE_WINDOW_DAYS)
                        }
                        Some(false) => format!(
                            "Not used in the last {} days: removing it costs nothing.",
                            crate::app::USAGE_WINDOW_DAYS
                        ),
                        None => "claudash can't tell whether this rule was used.".into(),
                    },
                    dim(),
                )));
            }
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "Change rules with /permissions in Claude Code, or edit the file.",
                dim().italic(),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(panel("Details", Color::Magenta)),
            detail_area,
        );
        return;
    }

    let list = match app.eco_tab {
        EcoTab::Skills => &eco.skills,
        EcoTab::Agents => &eco.agents,
        EcoTab::Commands => &eco.commands,
        _ => &eco.hooks,
    };
    if list.is_empty() {
        let text = format!("No {} found.", app.eco_tab.title().to_lowercase());
        frame.render_widget(message(text, block), list_area);
        frame.render_widget(panel("Details", Color::Magenta), detail_area);
        return;
    }
    let usage = app.recent_usage();
    let counts = match app.eco_tab {
        EcoTab::Skills => Some(&usage.skills),
        EcoTab::Agents => Some(&usage.agents),
        EcoTab::Commands => Some(&usage.commands),
        _ => None,
    };
    let items: Vec<ListItem> = list
        .iter()
        .map(|item| {
            let mut spans = vec![
                Span::styled(item.name.clone(), Style::new().bold()),
                Span::styled(format!("  {}", item.source), dim()),
            ];
            if let Some(map) = counts {
                let n = crate::app::UsageCounts::count(map, &item.name);
                spans.push(if n > 0 {
                    Span::styled(
                        format!("  {}", plural(n as u64, "use")),
                        Style::new().fg(Color::Cyan),
                    )
                } else {
                    Span::styled("  unused", dim())
                });
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let selected_item = app.eco_states[tab_index]
        .selected()
        .and_then(|i| list.get(i))
        .cloned();
    render_list(
        frame,
        items,
        block,
        list_area,
        &mut app.eco_states[tab_index],
    );
    draw_item_details(frame, selected_item.as_ref(), detail_area);
}

fn render_list(
    frame: &mut Frame,
    items: Vec<ListItem>,
    block: Block,
    area: Rect,
    state: &mut ratatui::widgets::ListState,
) {
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always);
    frame.render_stateful_widget(list, area, state);
}

fn draw_item_details(frame: &mut Frame, item: Option<&Item>, area: Rect) {
    let block = panel("Details", Color::Magenta);
    let Some(item) = item else {
        frame.render_widget(block, area);
        return;
    };
    let lines = vec![
        Line::from(Span::styled(item.name.clone(), Style::new().bold())),
        Line::from(vec![
            Span::styled("source  ", dim()),
            Span::raw(item.source.clone()),
        ]),
        Line::from(vec![
            Span::styled("file    ", dim()),
            Span::raw(paths::display(&item.path)),
        ]),
        Line::default(),
        Line::from(item.description.clone()),
    ];
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block);
    frame.render_widget(paragraph, area);
}

fn draw_plugin_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("Details", Color::Magenta);
    let Some(plugin) = app.selected_plugin() else {
        frame.render_widget(block, area);
        return;
    };
    let mut lines = vec![
        Line::from(Span::styled(plugin.id.clone(), Style::new().bold())),
        Line::from(vec![
            Span::styled("status  ", dim()),
            if plugin.enabled {
                Span::styled("enabled", Style::new().fg(Color::Green))
            } else {
                Span::styled("disabled", dim())
            },
            Span::styled(
                format!("   scope {}   version {}", plugin.scope, plugin.version),
                dim(),
            ),
        ]),
    ];
    if !plugin.mcp_servers.is_empty() {
        let names: Vec<&str> = plugin.mcp_servers.keys().map(String::as_str).collect();
        lines.push(Line::from(vec![
            Span::styled("mcp     ", dim()),
            Span::raw(names.join(", ")),
        ]));
    }
    if !plugin.description.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(plugin.description.clone()));
    }
    lines.push(Line::default());
    match app.plugin_details.get(&plugin.id) {
        Some(Ok(details)) => {
            // Everything after the header: inventory and projected token cost.
            let body = details
                .lines()
                .skip_while(|l| !l.starts_with("Component inventory"));
            lines.extend(body.map(|l| Line::from(l.to_string())));
        }
        Some(Err(e)) => lines.push(Line::from(Span::styled(
            e.clone(),
            Style::new().fg(Color::Red),
        ))),
        None => lines.push(Line::from(Span::styled(
            format!("{} claude plugin details…", app.spinner()),
            dim(),
        ))),
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Space to enable/disable · Enter for the full report",
        dim().italic(),
    )));
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block);
    frame.render_widget(paragraph, area);
}

// ---- Usage -------------------------------------------------------------------

fn sum_since(days: &BTreeMap<NaiveDate, Usage>, since: NaiveDate) -> Usage {
    let mut total = Usage::default();
    for usage in days.range(since..).map(|(_, u)| u) {
        total.add(usage);
    }
    total
}

/// Bars for the last days (or months) that fit in `width` columns.
fn usage_bars(
    app: &App,
    days: &BTreeMap<NaiveDate, Usage>,
    width: u16,
) -> (Vec<Bar<'static>>, String, u64) {
    let today = Local::now().date_naive();
    // Each bar takes 4 columns (3 wide + 1 gap) inside the borders.
    let fit = ((width.saturating_sub(4)) / 4).max(1) as i64;
    // Dollars are charted in cents.
    let measure = |u: &Usage| {
        if app.dollars {
            (u.cost * 100.0).round() as u64
        } else {
            u.processed()
        }
    };
    let mut peak = 0;
    let mut bar = |label: String, tokens: u64, current: bool| {
        peak = peak.max(tokens);
        let color = if current { Color::Yellow } else { Color::Cyan };
        Bar::default()
            .value(tokens)
            .text_value(String::new())
            .label(Line::from(label))
            .style(Style::new().fg(color))
    };
    if app.monthly {
        let n = fit.min(12);
        let mut months: BTreeMap<(i32, u32), u64> = BTreeMap::new();
        for (day, usage) in days {
            *months.entry((day.year(), day.month())).or_default() += measure(usage);
        }
        let (mut year, mut month) = (today.year(), today.month());
        let mut bars = Vec::new();
        for i in 0..n {
            let tokens = months.get(&(year, month)).copied().unwrap_or(0);
            let name = NaiveDate::from_ymd_opt(year, month, 1)
                .map(|d| d.format("%b").to_string())
                .unwrap_or_default();
            bars.push(bar(name, tokens, i == 0));
            (year, month) = if month == 1 {
                (year - 1, 12)
            } else {
                (year, month - 1)
            };
        }
        bars.reverse();
        (bars, format!("last {n} months"), peak)
    } else {
        let n = fit.min(30);
        let bars = (0..n)
            .rev()
            .map(|back| today - Days::days(back))
            .map(|day| {
                let tokens = days.get(&day).map_or(0, measure);
                bar(day.format("%d").to_string(), tokens, day == today)
            })
            .collect();
        (bars, format!("last {n} days"), peak)
    }
}

/// Days the "where tokens go" page covers.
const TOKEN_WINDOW_DAYS: i64 = 7;

/// "+12%" / "−8%" from `before` to `now`; empty when there's nothing before.
fn change(now: f64, before: f64) -> String {
    if before <= 0.0 {
        return String::new();
    }
    let pct = (now - before) / before * 100.0;
    if pct >= 0.0 {
        format!("+{pct:.0}%")
    } else {
        format!("−{:.0}%", -pct)
    }
}

fn draw_token_report(frame: &mut Frame, app: &App, area: Rect) {
    let report = app.token_report(TOKEN_WINDOW_DAYS);
    let [summary_area, body] =
        Layout::vertical([Constraint::Length(6), Constraint::Min(6)]).areas(area);

    let per = |total: u64, n: u32| if n > 0 { total as f64 / n as f64 } else { 0.0 };
    let ((r_now, o_now), (r_before, o_before)) = report.replies;
    let ((c_now, b_now), (c_before, b_before)) = report.tool_calls;
    let reply_now = per(o_now, r_now);
    let reply_before = per(o_before, r_before);
    let call_now = per(b_now, c_now) / 4.0;
    let call_before = per(b_before, c_before) / 4.0;
    let compare = |now: f64, before: f64| -> Vec<Span<'static>> {
        if before <= 0.0 {
            return Vec::new();
        }
        let delta = change(now, before);
        let color = if now <= before {
            Color::Green
        } else {
            Color::Yellow
        };
        vec![
            Span::styled(format!("   previous 30 days: {before:.0}  "), dim()),
            Span::styled(delta, Style::new().fg(color)),
        ]
    };
    let mut reply_line = vec![
        Span::styled("Replies      ", dim()),
        Span::raw(format!(
            "{r_now} in {TOKEN_WINDOW_DAYS} days · {reply_now:.0} output tokens per reply"
        )),
    ];
    reply_line.extend(compare(reply_now, reply_before));
    // Prompts sent after the cache expired, re-writing the whole context.
    let week_ago = Local::now() - Days::days(TOKEN_WINDOW_DAYS);
    let (mut restarts, mut restart_tokens, mut restart_cost) = (0, 0u64, 0.0);
    for s in &app.sessions {
        for r in app
            .analysis(s)
            .map(|a| a.cold_restarts.as_slice())
            .unwrap_or_default()
        {
            if r.at.is_some_and(|t| t >= week_ago) {
                restarts += 1;
                restart_tokens += r.tokens;
                restart_cost += r.cost;
            }
        }
    }
    let idle_line = Line::from(vec![
        Span::styled("Idle gaps    ", dim()),
        Span::raw(format!(
            "{restarts} prompts after the cache expired re-cached {} tokens ",
            human_tokens(restart_tokens)
        )),
        Span::styled(
            format!("≈{}", crate::pricing::format_usd(restart_cost)),
            Style::new().fg(if restart_cost > 0.0 {
                Color::Yellow
            } else {
                Color::Green
            }),
        ),
        Span::styled(
            " · /compact before a break makes the next one cheaper",
            dim(),
        ),
    ]);
    let mut tool_line = vec![
        Span::styled("Tool output  ", dim()),
        Span::raw(format!(
            "{c_now} calls · ~{call_now:.0} tokens per call into the context"
        )),
    ];
    tool_line.extend(compare(call_now, call_before));
    let savers = match app.token_savers.as_deref() {
        Some([]) | None => Line::from(vec![
            Span::styled("Savers       ", dim()),
            Span::styled(
                "none installed (caveman shortens replies, rtk shortens command output)",
                dim(),
            ),
        ]),
        Some(found) => Line::from(vec![
            Span::styled("Savers       ", dim()),
            Span::styled(
                format!("{} installed", found.join(", ")),
                Style::new().fg(Color::Green),
            ),
            Span::styled(
                " · the change against the previous 30 days shows what they save",
                dim(),
            ),
        ]),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(reply_line),
            Line::from(tool_line),
            idle_line,
            savers,
        ])
        .block(panel(
            &format!("Where tokens go · last {TOKEN_WINDOW_DAYS} days"),
            Color::Cyan,
        )),
        summary_area,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let total: u64 = report
        .outputs
        .iter()
        .map(|(_, s)| s.bytes)
        .sum::<u64>()
        .max(1);
    let label_width = left.width.saturating_sub(40).clamp(10, 40) as usize;
    let rows: Vec<Line> = report
        .outputs
        .iter()
        .take(left.height.saturating_sub(2) as usize)
        .map(|(label, s)| {
            let share = s.bytes as f64 / total as f64;
            let bar = "█".repeat((share * 10.0).round() as usize);
            let label = match label.strip_prefix("Bash: ") {
                Some(cmd) => format!("$ {cmd}"),
                None => tool_label(label),
            };
            let label: String = label.chars().take(label_width).collect();
            Line::from(vec![
                Span::raw(format!("{label:<label_width$} ")),
                Span::styled(
                    format!("{:>7} ", human_tokens(s.tokens())),
                    Style::new().bold(),
                ),
                Span::styled(
                    format!(
                        "{:>5} calls {:>6}/call ",
                        s.calls,
                        human_tokens(s.tokens() / s.calls.max(1) as u64)
                    ),
                    dim(),
                ),
                Span::styled(bar, Style::new().fg(Color::Cyan)),
            ])
        })
        .collect();
    let block = panel("Tool output into the context", Color::Cyan)
        .title_bottom(Line::from(" ~4 bytes per token ").right_aligned());
    if rows.is_empty() {
        frame.render_widget(message("No tool calls in these days.", block), left);
    } else {
        frame.render_widget(Paragraph::new(rows).block(block), left);
    }

    let width = right.width.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    for (title, project, p) in &report.prompts {
        if lines.len() + 2 > right.height.saturating_sub(2) as usize {
            break;
        }
        let text: String = p
            .text
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(width.saturating_sub(10))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>7} ", human_tokens(p.usage.processed())),
                Style::new().fg(Color::Yellow).bold(),
            ),
            Span::raw(text),
        ]));
        let when =
            p.at.map(|t| t.format("%b %d %H:%M").to_string())
                .unwrap_or_default();
        let meta: String = format!(
            "        {when} · {} · {} · {}",
            plural(p.requests as u64, "request"),
            title,
            project
        )
        .chars()
        .take(width)
        .collect();
        lines.push(Line::from(Span::styled(meta, dim())));
    }
    let block = panel("Costliest prompts", Color::Yellow).title_bottom(
        Line::from(" tokens processed by every request a prompt caused ").right_aligned(),
    );
    if lines.is_empty() {
        frame.render_widget(message("No prompts in these days.", block), right);
    } else {
        frame.render_widget(Paragraph::new(lines).block(block), right);
    }
}

fn draw_usage(frame: &mut Frame, app: &App, area: Rect) {
    let [plan_area, chart_area, bottom] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(8),
        Constraint::Length(8),
    ])
    .areas(area);
    draw_plan(frame, app, plan_area);

    let today = Local::now().date_naive();
    let days = app.history.per_day();
    let (bars, range, peak) = usage_bars(app, &days, chart_area.width);
    let since = app
        .history
        .first_day()
        .map(|d| format!(" · history since {}", d.format("%b %d")))
        .unwrap_or_default();
    let (title, what, peak) = match (app.dollars, app.monthly) {
        (true, true) => (
            "Dollars per month",
            "API-equivalent",
            crate::pricing::format_usd(peak as f64 / 100.0),
        ),
        (true, false) => (
            "Dollars per day",
            "API-equivalent",
            crate::pricing::format_usd(peak as f64 / 100.0),
        ),
        (false, true) => (
            "Tokens per month",
            "input + cache write + output",
            human_tokens(peak),
        ),
        (false, false) => (
            "Tokens per day",
            "input + cache write + output",
            human_tokens(peak),
        ),
    };
    let chart = BarChart::default()
        .block(
            panel(title, Color::Cyan).title_bottom(
                Line::from(format!(
                    " {what} · {range} · peak {peak}{since} · m days/months · $ dollars/tokens "
                ))
                .right_aligned(),
            ),
        )
        .bar_width(3)
        .bar_gap(1)
        .data(BarGroup::default().bars(&bars));
    frame.render_widget(chart, chart_area);

    let [totals_area, models_area, projects_area] = Layout::horizontal([
        Constraint::Percentage(40),
        Constraint::Percentage(32),
        Constraint::Percentage(28),
    ])
    .areas(bottom);

    let header = Row::new(["", "input", "c.write", "c.read", "output", "≈ USD"]).style(dim());
    let rows = [
        ("Today", today),
        ("7 days", today - Days::days(6)),
        ("30 days", today - Days::days(29)),
        ("1 year", today - Days::days(364)),
    ]
    .map(|(label, since)| {
        let u = sum_since(&days, since);
        Row::new([
            label.to_string(),
            human_tokens(u.input_tokens),
            human_tokens(u.cache_creation_input_tokens),
            human_tokens(u.cache_read_input_tokens),
            human_tokens(u.output_tokens),
            crate::pricing::format_usd(u.cost),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .block(
        panel("Totals", Color::Yellow)
            .title_bottom(Line::from(" incl. subagents ").right_aligned()),
    );
    frame.render_widget(table, totals_area);

    // By model over the last 30 days, with each model's share.
    let models = app.history.per_model(today - Days::days(29));
    let all: u64 = models.iter().map(|(_, u)| u.processed()).sum();
    let lines: Vec<Line> = models
        .iter()
        .take(5)
        .map(|(model, usage)| {
            let share = if all > 0 {
                usage.processed() as f64 / all as f64 * 100.0
            } else {
                0.0
            };
            Line::from(vec![
                Span::styled(
                    format!("{:>7}  ", human_tokens(usage.processed())),
                    Style::new().bold(),
                ),
                Span::styled(format!("{share:>3.0}%  "), dim()),
                Span::styled(
                    format!("{:>8}  ", crate::pricing::format_usd(usage.cost)),
                    Style::new().fg(Color::Green),
                ),
                Span::raw(model.clone()),
            ])
        })
        .collect();
    let block = panel("By model · 30 days", Color::Magenta);
    if lines.is_empty() {
        frame.render_widget(message("No usage in the last 30 days.", block), models_area);
    } else {
        frame.render_widget(Paragraph::new(lines).block(block), models_area);
    }

    let lines: Vec<Line> = app
        .history
        .per_project(today - Days::days(6))
        .into_iter()
        .take(6)
        .map(|(project, usage)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>7}  ", human_tokens(usage.processed())),
                    Style::new().bold(),
                ),
                Span::styled(
                    format!("{:>8}  ", crate::pricing::format_usd(usage.cost)),
                    Style::new().fg(Color::Green),
                ),
                Span::raw(project),
            ])
        })
        .collect();
    let block = panel("Top projects · 7 days", Color::Green);
    if lines.is_empty() {
        frame.render_widget(
            message("No usage in the last 7 days.", block),
            projects_area,
        );
    } else {
        frame.render_widget(Paragraph::new(lines).block(block), projects_area);
    }
}

fn draw_plan(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("Plan usage", Color::LightMagenta);
    let windows = plan_windows(app);
    if windows.is_empty() {
        let text = if app.statusline.configured {
            "Claude Code hasn't reported plan usage yet. It's only sent to Pro and Max \
             subscribers, after the first reply of a session."
        } else {
            "Plan usage comes from Claude Code's status line. Run `claudash setup` \
             for the one-line settings change."
        };
        frame.render_widget(message(text, block), area);
        return;
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical(vec![Constraint::Length(1); windows.len()]).split(inner);
    for ((label, w), row) in windows.iter().zip(rows.iter()) {
        let ratio = w.used_percentage / 100.0;
        let name = if *label == "5h" { "5-hour " } else { "7-day  " };
        let gauge = Gauge::default()
            .gauge_style(Style::new().fg(level_color(ratio)).bg(GAUGE_TRACK))
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!(
                "{name} {:.0}%  · resets {}",
                w.used_percentage,
                reset_time(w)
            ))
            .use_unicode(true);
        // Gauge on the left, the forecast next to it.
        let [bar, note] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(*row);
        frame.render_widget(gauge, bar);
        if let Some(span) = forecast_span(app, label) {
            frame.render_widget(Paragraph::new(Line::from(span)), note);
        }
    }
}

// ---- Activity ----------------------------------------------------------------

/// How long ago a file changed.
fn file_age(path: &std::path::Path) -> Option<std::time::Duration> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
}

/// "12s", "4m", "2h".
fn short_age(secs: i64) -> String {
    match secs.max(0) {
        s @ 0..60 => format!("{s}s"),
        s @ 60..3_600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3_600),
    }
}

fn project_name(project_path: &str) -> String {
    project_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(project_path)
        .to_string()
}

fn draw_activity(frame: &mut Frame, app: &mut App, area: Rect) {
    let now = chrono::Local::now();
    let open = app.open_sessions();
    let open_height = ((open.len().max(1) * 3) as u16 + 2)
        .min(area.height / 2)
        .max(4);
    let bg_height = if app.background.is_empty() {
        0
    } else {
        (app.background.len() as u16 + 2).min(8)
    };
    let [open_area, bg_area, feed_area] = Layout::vertical([
        Constraint::Length(open_height),
        Constraint::Length(bg_height),
        Constraint::Min(4),
    ])
    .areas(area);

    let block = focused(
        panel("Open sessions", Color::Green)
            .title_bottom(Line::from(format!(" {} open ", open.len())).right_aligned()),
        app.activity_focus == crate::app::ActivityFocus::Open,
    );
    if open.is_empty() {
        frame.render_widget(
            message("No Claude Code session is open right now.", block),
            open_area,
        );
    } else {
        let items: Vec<ListItem> = open
            .iter()
            .map(|s| {
                let (mark, color, label) = match app.activity(&s.id) {
                    Some(Activity::NeedsYou) => ("▲", Color::Yellow, "needs you"),
                    Some(Activity::Working) => ("●", Color::Green, "working"),
                    _ => ("●", Color::Cyan, "waiting"),
                };
                let mut head = vec![
                    Span::styled(format!("{mark} "), Style::new().fg(color).bold()),
                    Span::styled(s.title.clone(), Style::new().bold()),
                    Span::styled(format!("  {label}"), Style::new().fg(color)),
                    Span::styled(format!("  · {}", s.project_path), dim()),
                ];
                if let Some(why) = app.live.get(&s.id).and_then(|l| l.waiting_for.clone()) {
                    head.push(Span::styled(
                        format!("  ({why})"),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                let analysis = app.analysis(s);
                let mut detail = vec![Span::raw("    ")];
                if let Some(tool) = app.pending_tool(&s.id) {
                    let summary = crate::transcript::describe(tool)
                        .into_iter()
                        .find(|l| !l.is_empty())
                        .unwrap_or_default();
                    detail.push(Span::styled(
                        format!(
                            "asks: ⚙ {} {}",
                            tool_label(&tool.name),
                            summary.chars().take(70).collect::<String>()
                        ),
                        Style::new().fg(Color::Yellow),
                    ));
                    detail.push(Span::styled("  · Enter to see it all", dim()));
                } else if let Some(e) = analysis.and_then(|a| a.recent.last()) {
                    let ago =
                        e.at.map(|t| short_age((now - t).num_seconds()))
                            .unwrap_or_default();
                    detail.push(Span::styled(
                        format!(
                            "last: ⚙ {} {}",
                            tool_label(&e.tool),
                            e.summary.chars().take(50).collect::<String>()
                        ),
                        if e.failed {
                            Style::new().fg(Color::Red)
                        } else {
                            Style::new().fg(Color::Gray)
                        },
                    ));
                    detail.push(Span::styled(format!(" · {ago} ago"), dim()));
                }
                let limit = app
                    .statusline
                    .sessions
                    .get(&s.id)
                    .and_then(|x| x.context_window.context_window_size)
                    .filter(|&n| n > 0)
                    .unwrap_or(app.context_limit);
                let pct = s.tokens.context_used as f64 / limit as f64 * 100.0;
                detail.push(Span::styled(
                    format!("  · context {pct:.0}%"),
                    Style::new().fg(level_color(pct / 100.0)),
                ));
                // Only while the session waits for a prompt: a working one keeps its cache warm.
                if !matches!(app.activity(&s.id), Some(Activity::Working))
                    && let Some(span) = cache_clock_span(app, s)
                {
                    detail.push(span);
                }
                let running = analysis.map_or(0, |a| {
                    a.subagents
                        .iter()
                        .filter(|sub| file_age(&sub.file).is_some_and(|age| age.as_secs() < 60))
                        .count()
                });
                if running > 0 {
                    detail.push(Span::styled(
                        format!("  · {} running", plural(running as u64, "subagent")),
                        Style::new().fg(Color::Green),
                    ));
                }
                ListItem::new(vec![Line::from(head), Line::from(detail), Line::default()])
            })
            .collect();
        if app
            .activity_state
            .selected()
            .is_none_or(|i| i >= items.len())
        {
            app.activity_state.select(Some(0));
        }
        render_list(frame, items, block, open_area, &mut app.activity_state);
    }

    if bg_height > 0 {
        draw_background(frame, app, bg_area);
    }

    // Live feed of tool calls.
    let rows = feed_area.height.saturating_sub(2) as usize;
    let feed = app.feed(rows.max(1));
    let block = panel("Live feed", Color::Cyan).title_bottom(
        Line::from(" tool calls of sessions active in the last hour · refreshes every 2s ")
            .right_aligned(),
    );
    if feed.is_empty() {
        let text = if app.analyses.is_empty() {
            format!("{} Reading sessions…", app.spinner())
        } else {
            "Nothing happened in the last hour.".into()
        };
        frame.render_widget(message(text, block), feed_area);
        return;
    }
    let lines: Vec<Line> = feed
        .iter()
        .map(|item| {
            let e = item.event;
            let time =
                e.at.map(|t| t.format("%H:%M:%S").to_string())
                    .unwrap_or_default();
            let (mark, style) = if e.failed {
                ("✘", Style::new().fg(Color::Red))
            } else {
                ("⚙", Style::new().fg(Color::Yellow))
            };
            Line::from(vec![
                Span::styled(format!("{time}  "), dim()),
                Span::styled(
                    format!(
                        "{:<16}",
                        project_name(&item.session.project_path)
                            .chars()
                            .take(16)
                            .collect::<String>()
                    ),
                    Style::new().fg(Color::LightMagenta),
                ),
                Span::styled(
                    format!(
                        "{:<28}  ",
                        item.session.title.chars().take(28).collect::<String>()
                    ),
                    dim(),
                ),
                Span::styled(format!("{mark} {}  ", tool_label(&e.tool)), style),
                Span::raw(e.summary.clone()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), feed_area);
}

fn draw_background(frame: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .background
        .iter()
        .map(|bg| {
            let state = bg.state.clone().unwrap_or_else(|| bg.status.clone());
            let color = match state.as_str() {
                "working" | "busy" => Color::Green,
                "blocked" | "waiting" => Color::Yellow,
                "done" | "idle" => Color::Cyan,
                "failed" => Color::Red,
                _ => Color::DarkGray,
            };
            let mut spans = vec![
                Span::styled(format!("{state:<8}"), Style::new().fg(color).bold()),
                Span::styled(
                    bg.name
                        .clone()
                        .unwrap_or_else(|| "background session".into()),
                    Style::new().bold(),
                ),
            ];
            if let Some(id) = &bg.id {
                spans.push(Span::styled(format!("  {id}"), dim()));
            }
            if let Some(why) = &bg.waiting_for {
                spans.push(Span::styled(
                    format!("  ({why})"),
                    Style::new().fg(Color::Yellow),
                ));
            }
            if let Some(cwd) = &bg.cwd {
                spans.push(Span::styled(
                    format!("  · {}", paths::display(std::path::Path::new(cwd))),
                    dim(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let block = focused(
        panel("Background sessions", Color::Magenta).title_bottom(
            Line::from(" Tab to select · Enter attach · l log · S stop · R respawn ")
                .right_aligned(),
        ),
        app.activity_focus == crate::app::ActivityFocus::Background,
    );
    let mut list = List::new(items).block(block);
    if app.activity_focus == crate::app::ActivityFocus::Background {
        list = list
            .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
            .highlight_symbol("▶ ")
            .highlight_spacing(HighlightSpacing::Always);
    }
    frame.render_stateful_widget(list, area, &mut app.background_state);
}

// ---- Logs --------------------------------------------------------------------

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let [list_area, content_area] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(area);
    let logs = &mut app.logs;
    if logs.sources.is_empty() {
        let text = "No logs yet: MCP server logs appear once Claude Code has started the selected \
                    project's servers, and background sessions once you run one (claude --bg).";
        frame.render_widget(message(text, panel("Sources", Color::Cyan)), list_area);
        frame.render_widget(panel("Log", Color::Cyan), content_area);
        return;
    }
    let items: Vec<ListItem> = logs
        .sources
        .iter()
        .map(|s| {
            ListItem::new(vec![
                Line::from(Span::styled(s.label.clone(), Style::new().bold())),
                Line::from(Span::styled(format!("  {}", s.group), dim())),
            ])
        })
        .collect();
    render_list(
        frame,
        items,
        panel("Sources", Color::Cyan),
        list_area,
        &mut logs.state,
    );

    let source = logs.state.selected().and_then(|i| logs.sources.get(i));
    let mut block = panel("", Color::Cyan).title(
        Line::from(format!(
            " {} ",
            source.map(|s| s.label.as_str()).unwrap_or("Log")
        ))
        .bold()
        .fg(Color::Cyan),
    );
    let mut flags = Vec::new();
    if !logs.filter.is_empty() {
        flags.push(format!("filter \"{}\"", logs.filter));
    }
    if logs.errors_only {
        flags.push("errors only".into());
    }
    flags.push(if logs.follow {
        "following".into()
    } else {
        "paused".into()
    });
    if logs.loading() {
        flags.push("loading…".into());
    }
    block = block.title(
        Line::from(format!(" {} ", flags.join(" · ")))
            .fg(Color::DarkGray)
            .right_aligned(),
    );

    let lines = match &logs.lines {
        Err(e) => {
            frame.render_widget(message(e.clone(), block), content_area);
            return;
        }
        Ok(lines) => lines,
    };
    let needle = logs.filter.to_lowercase();
    let shown: Vec<&String> = lines
        .iter()
        .filter(|l| needle.is_empty() || l.to_lowercase().contains(&needle))
        .filter(|l| !logs.errors_only || l.to_lowercase().contains("error") || l.contains("✘"))
        .collect();
    let inner = block.inner(content_area);
    let width = inner.width.max(1) as usize;
    let rows: Vec<(String, bool)> = shown
        .iter()
        .flat_map(|l| {
            let error = l.to_lowercase().contains("error");
            hard_wrap(l, width).into_iter().map(move |r| (r, error))
        })
        .collect();
    let visible = inner.height as usize;
    let last_page = rows.len().saturating_sub(visible);
    if logs.follow {
        logs.scroll = last_page;
    }
    logs.scroll = logs.scroll.min(last_page);
    let text: Vec<Line> = rows[logs.scroll..(logs.scroll + visible).min(rows.len())]
        .iter()
        .map(|(r, error)| {
            if *error {
                Line::from(Span::styled(r.clone(), Style::new().fg(Color::Red)))
            } else {
                Line::from(r.clone())
            }
        })
        .collect();
    let block = block.title_bottom(
        Line::from(format!(" {} of {} lines ", shown.len(), lines.len())).right_aligned(),
    );
    frame.render_widget(Paragraph::new(text).block(block), content_area);
}

// ---- Projects ----------------------------------------------------------------

fn problem_line(problem: &crate::app::Problem) -> Line<'static> {
    use crate::app::Problem;
    let red = Style::new().fg(Color::Red).bold();
    let yellow = Style::new().fg(Color::Yellow);
    match problem {
        Problem::SharedFolder { dir, sessions } => Line::from(vec![
            Span::styled("⚠ ", red),
            Span::styled(
                format!(
                    "{} open sessions share {}",
                    sessions.len(),
                    paths::display(dir)
                ),
                red,
            ),
            Span::styled(
                format!(": {} · their edits can collide", sessions.join(", ")),
                dim(),
            ),
        ]),
        Problem::SameFile { file, sessions } => Line::from(vec![
            Span::styled("⚠ ", red),
            Span::styled(paths::display(std::path::Path::new(file)), red),
            Span::styled(
                format!(
                    " edited by {} open sessions: {}",
                    sessions.len(),
                    sessions.join(", ")
                ),
                dim(),
            ),
        ]),
        Problem::IdleWorktree {
            path,
            changed,
            ahead,
        } => {
            let mut what = Vec::new();
            if *changed > 0 {
                what.push(format!("{changed} changed"));
            }
            if *ahead > 0 {
                what.push(format!("{ahead} to push"));
            }
            Line::from(vec![
                Span::styled("! ", yellow),
                Span::styled(format!("worktree {}", paths::display(path)), yellow),
                Span::styled(format!(": {} and no open session", what.join(", ")), dim()),
            ])
        }
        Problem::Secrets { session, found, .. } => {
            let place = if session.is_empty() {
                "your prompt history".to_string()
            } else {
                format!("\"{session}\"")
            };
            Line::from(vec![
                Span::styled("🔑 ", Style::new().fg(Color::Red)),
                Span::styled(
                    format!("credentials in {place}"),
                    Style::new().fg(Color::Red),
                ),
                Span::styled(format!(": {}", found.join(", ")), dim()),
            ])
        }
        Problem::Risky {
            session,
            what,
            detail,
            ..
        } => Line::from(vec![
            Span::styled("⚠ ", Style::new().fg(Color::Red)),
            Span::styled(what.to_string(), Style::new().fg(Color::Red)),
            Span::styled(
                format!(
                    " in \"{session}\": {}",
                    detail.chars().take(60).collect::<String>()
                ),
                dim(),
            ),
        ]),
        Problem::StaleChange {
            framework,
            id,
            done,
            total,
            days,
            ..
        } => Line::from(vec![
            Span::styled("◷ ", Style::new().fg(Color::Yellow)),
            Span::styled(
                format!("{framework} change {id}"),
                Style::new().fg(Color::Yellow),
            ),
            Span::styled(
                format!(": {done}/{total} tasks done, untouched for {days} days"),
                dim(),
            ),
        ]),
        Problem::FinishedChange {
            framework,
            id,
            next,
            ..
        } => Line::from(vec![
            Span::styled("✓ ", Style::new().fg(Color::Green)),
            Span::styled(
                format!("{framework} change {id}"),
                Style::new().fg(Color::Green),
            ),
            Span::styled(format!(": every task done; wrap it up with {next}"), dim()),
        ]),
        Problem::MissingWorktree { path } => Line::from(vec![
            Span::styled("· ", dim()),
            Span::styled(
                format!(
                    "worktree {} no longer exists (git worktree prune)",
                    paths::display(path)
                ),
                dim(),
            ),
        ]),
    }
}

/// "3 changed ↑2 ↓1" or "clean".
fn git_summary(status: Option<&crate::git::Status>) -> Vec<Span<'static>> {
    let Some(s) = status else {
        return vec![Span::styled("no git data", dim())];
    };
    let mut spans = vec![if s.changed > 0 {
        Span::styled(
            format!("{} changed", s.changed),
            Style::new().fg(Color::Yellow),
        )
    } else {
        Span::styled("clean", Style::new().fg(Color::Green))
    }];
    if s.ahead > 0 {
        spans.push(Span::styled(
            format!(" ↑{}", s.ahead),
            Style::new().fg(Color::Cyan),
        ));
    }
    if s.behind > 0 {
        spans.push(Span::styled(
            format!(" ↓{}", s.behind),
            Style::new().fg(Color::Magenta),
        ));
    }
    spans
}

// ---- Inspector ---------------------------------------------------------------

/// Downsamples `values` to `width` buckets, keeping each bucket's peak.
fn downsample(values: &[u64], width: usize) -> Vec<u64> {
    if values.len() <= width || width == 0 {
        return values.to_vec();
    }
    (0..width)
        .map(|i| {
            let start = i * values.len() / width;
            let end = ((i + 1) * values.len() / width).max(start + 1);
            values[start..end].iter().copied().max().unwrap_or(0)
        })
        .collect()
}

fn draw_inspect(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = app.selected_session() else {
        frame.render_widget(
            message("Select a session first.", panel("Inspect", Color::Cyan)),
            area,
        );
        return;
    };
    let block = panel("", Color::Cyan).title(
        Line::from(format!(" {} ", session.title))
            .bold()
            .fg(Color::Cyan),
    );
    let Some(a) = app.analysis(session) else {
        let text = format!("{} Analyzing the conversation…", app.spinner());
        frame.render_widget(message(text, block), area);
        return;
    };
    let block = block.title(
        Line::from(format!(" {} ", session.project_path))
            .fg(Color::DarkGray)
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [chart_area, body] =
        Layout::vertical([Constraint::Length(8), Constraint::Min(3)]).areas(inner);

    // Context per request.
    let points: Vec<u64> = a.context.iter().map(|p| p.tokens).collect();
    let compactions = a.context.iter().filter(|p| p.after_compaction).count();
    let peak = points.iter().copied().max().unwrap_or(0);
    let data = downsample(&points, chart_area.width.saturating_sub(2) as usize);
    let chart = ratatui::widgets::Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(dim())
                .title(Line::from(vec![
                    Span::styled("Context per request  ", Style::new().bold()),
                    Span::styled(
                        format!(
                            "{} requests · first {} · peak {} · now {} · {} compaction(s)",
                            points.len(),
                            human_tokens(a.first_context().unwrap_or(0)),
                            human_tokens(peak),
                            human_tokens(points.last().copied().unwrap_or(0)),
                            compactions
                        ),
                        dim(),
                    ),
                ])),
        )
        .data(&data)
        .style(Style::new().fg(Color::Cyan));
    frame.render_widget(chart, chart_area);

    let mut lines: Vec<Line> = Vec::new();
    let section = |lines: &mut Vec<Line>, title: &str| {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            title.to_string(),
            Style::new().fg(Color::LightMagenta).bold(),
        )));
    };

    section(&mut lines, "Tools");
    let mut tools: Vec<(&String, &crate::analysis::ToolStat)> = a.tools.iter().collect();
    tools.sort_by_key(|(_, t)| std::cmp::Reverse(t.calls));
    if tools.is_empty() {
        lines.push(Line::from(Span::styled("  no tool calls", dim())));
    }
    for (name, t) in tools {
        let name = tool_label(name);
        let rate = if t.calls > 0 {
            t.errors as f64 / t.calls as f64 * 100.0
        } else {
            0.0
        };
        let color = if t.errors == 0 {
            Color::DarkGray
        } else if rate >= 20.0 {
            Color::Red
        } else {
            Color::Yellow
        };
        lines.push(Line::from(vec![
            Span::raw(format!(
                "  {:<40}",
                name.chars().take(40).collect::<String>()
            )),
            Span::styled(
                format!("{:>11}", plural(t.calls as u64, "call")),
                Style::new().bold(),
            ),
            Span::styled(
                format!("   {:>3} failed ({rate:.0}%)", t.errors),
                Style::new().fg(color),
            ),
        ]));
    }

    section(&mut lines, "Audit");
    let mut events: Vec<&crate::audit::Event> = a.audit.iter().collect();
    events.sort_by_key(|e| e.severity);
    events.dedup_by(|x, y| x.what == y.what && x.detail == y.detail);
    if events.is_empty() && a.secrets.is_empty() {
        lines.push(Line::from(Span::styled(
            "  nothing risky: no dangerous commands, no edits outside the project, no secrets",
            dim(),
        )));
    }
    for s in &a.secrets {
        lines.push(Line::from(vec![
            Span::styled("  🔑 ", Style::new().fg(Color::Red)),
            Span::styled(
                format!("{} {}", s.kind, s.masked),
                Style::new().fg(Color::Red),
            ),
            Span::styled(format!("  in {} · rotate it", s.place), dim()),
        ]));
    }
    for e in events.iter().take(15) {
        let color = match e.severity {
            crate::audit::Severity::High => Color::Red,
            crate::audit::Severity::Medium => Color::Yellow,
            crate::audit::Severity::Low => Color::Cyan,
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<7}", e.severity.label()),
                Style::new().fg(color),
            ),
            Span::styled(format!("{:<34}", e.what), Style::new().bold()),
            Span::styled(e.detail.chars().take(90).collect::<String>(), dim()),
        ]));
    }
    if events.len() > 15 {
        lines.push(Line::from(Span::styled(
            format!("  and {} more", events.len() - 15),
            dim(),
        )));
    }

    section(&mut lines, "Tool output that entered the context");
    let mut outputs: BTreeMap<&str, crate::analysis::OutputStat> = BTreeMap::new();
    for ((_, label), stat) in &a.tool_output {
        let o = outputs.entry(label.as_str()).or_default();
        o.calls += stat.calls;
        o.bytes += stat.bytes;
    }
    let mut outputs: Vec<(&str, crate::analysis::OutputStat)> = outputs.into_iter().collect();
    outputs.sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes));
    if outputs.is_empty() {
        lines.push(Line::from(Span::styled("  none", dim())));
    }
    for (label, stat) in outputs.iter().take(8) {
        let label = match label.strip_prefix("Bash: ") {
            Some(cmd) => format!("$ {cmd}"),
            None => tool_label(label),
        };
        lines.push(Line::from(vec![
            Span::raw(format!(
                "  {:<40}",
                label.chars().take(40).collect::<String>()
            )),
            Span::styled(
                format!("{:>8}", human_tokens(stat.tokens())),
                Style::new().bold(),
            ),
            Span::styled(
                format!(
                    "   {} · ~4 bytes per token",
                    plural(stat.calls as u64, "call")
                ),
                dim(),
            ),
        ]));
    }

    section(&mut lines, "Costliest prompts");
    let mut prompts: Vec<&crate::analysis::PromptCost> = a.prompts.iter().collect();
    prompts.sort_by_key(|p| std::cmp::Reverse(p.usage.processed()));
    if prompts.is_empty() {
        lines.push(Line::from(Span::styled("  none", dim())));
    }
    for p in prompts.iter().take(5) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>8}  ", human_tokens(p.usage.processed())),
                Style::new().fg(Color::Yellow).bold(),
            ),
            Span::raw(
                p.text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(80)
                    .collect::<String>(),
            ),
            Span::styled(
                format!("  · {}", plural(p.requests as u64, "request")),
                dim(),
            ),
        ]));
    }

    section(&mut lines, &format!("Subagents ({})", a.subagents.len()));
    if a.subagents.is_empty() {
        lines.push(Line::from(Span::styled("  none", dim())));
    }
    for (i, sub) in a.subagents.iter().enumerate() {
        let selected = app.inspect_sub == Some(i);
        let marker = if selected { "▶ " } else { "├ " };
        let running = file_age(&sub.file).is_some_and(|age| age.as_secs() < 60);
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {marker}{}", sub.agent_type),
                if selected {
                    Style::new().fg(Color::Black).bg(Color::Cyan).bold()
                } else {
                    Style::new().fg(Color::Cyan).bold()
                },
            ),
            Span::styled(
                if running { "  ● running" } else { "" },
                Style::new().fg(Color::Green),
            ),
            Span::raw(format!("  {}", sub.description)),
            Span::styled(
                {
                    let mut parts = Vec::new();
                    if !sub.model.is_empty() {
                        parts.push(sub.model.clone());
                    }
                    if sub.background {
                        parts.push("background".into());
                    }
                    parts.push(plural(sub.tool_calls as u64, "tool call"));
                    parts.push(format!("{} tokens", human_tokens(sub.usage.processed())));
                    format!("  · {}", parts.join(" · "))
                },
                dim(),
            ),
        ]));
    }

    section(&mut lines, &format!("Files edited ({})", a.edits.len()));
    let mut edits: Vec<(&String, &Option<chrono::DateTime<Local>>)> = a.edits.iter().collect();
    edits.sort_by_key(|(_, at)| std::cmp::Reverse(**at));
    if edits.is_empty() {
        lines.push(Line::from(Span::styled("  none", dim())));
    }
    for (file, at) in edits {
        let when = at
            .map(|t| t.format("%b %d %H:%M").to_string())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw(format!("  {}", paths::display(std::path::Path::new(file)))),
            Span::styled(format!("  {when}"), dim()),
        ]));
    }

    section(&mut lines, "Used");
    for (label, map) in [
        ("skills", &a.skills),
        ("subagents", &a.agents),
        ("MCP servers", &a.mcp_servers),
        ("commands", &a.commands),
    ] {
        let mut items: Vec<(&String, &u32)> = map.iter().collect();
        items.sort_by_key(|(k, v)| (std::cmp::Reverse(**v), (*k).clone()));
        let text = if items.is_empty() {
            "—".to_string()
        } else {
            items
                .iter()
                .map(|(k, v)| format!("{k} ×{v}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:<12}"), dim()),
            Span::raw(text),
        ]));
    }

    let visible = body.height as usize;
    let last_page = lines.len().saturating_sub(visible).min(u16::MAX as usize) as u16;
    app.inspect_scroll = app.inspect_scroll.min(last_page);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.inspect_scroll, 0)),
        body,
    );
}

// ---- Help --------------------------------------------------------------------

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let lines = crate::help::lines(app);
    let block = panel("Help", Color::LightMagenta);
    let visible = block.inner(area).height as usize;
    let last_page = lines.len().saturating_sub(visible).min(u16::MAX as usize) as u16;
    app.help_scroll = app.help_scroll.min(last_page);
    let block =
        block.title_bottom(Line::from(" ↑/↓ PgUp/PgDn scroll · ? or Esc close ").right_aligned());
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.help_scroll, 0)),
        area,
    );
}

// ---- Conversation ------------------------------------------------------------

/// Builds the screen rows for a conversation at `width` columns.
fn transcript_rows(view: &TranscriptView, width: u16) -> Vec<TranscriptRow> {
    let width = width.max(10) as usize;
    let mut rows = Vec::new();
    let needle = view.query.as_ref().map(|q| q.to_lowercase());
    let mut push =
        |entry: usize, text: String, style: Style| rows.push(TranscriptRow { entry, text, style });
    // Word-wrap, then hard-wrap anything still too long (paths, URLs).
    let wrap = |text: &str, indent: usize| -> Vec<String> {
        crate::app::textwrap(text, width.saturating_sub(indent).max(1))
            .into_iter()
            .flat_map(|l| hard_wrap(&l, width.saturating_sub(indent).max(1)))
            .map(|l| format!("{}{l}", " ".repeat(indent)))
            .collect()
    };
    let time = |e: &transcript::Entry| {
        e.at.map(|t| t.format(" · %b %d %H:%M").to_string())
            .unwrap_or_default()
    };

    for (i, e) in view.entries.iter().enumerate() {
        let hit = needle.as_ref().is_some_and(|n| {
            transcript::searchable(e).is_some_and(|t| t.to_lowercase().contains(n))
        });
        let mark = |style: Style| {
            if hit {
                style.bg(Color::Rgb(70, 60, 20))
            } else {
                style
            }
        };
        match &e.kind {
            Kind::User | Kind::Assistant => {
                let (who, color) = if e.kind == Kind::User {
                    ("You", Color::LightMagenta)
                } else {
                    ("Claude", Color::Cyan)
                };
                push(i, String::new(), Style::new());
                push(
                    i,
                    format!("▌ {who}{}", time(e)),
                    Style::new().fg(color).bold(),
                );
                for line in wrap(&e.text, 2) {
                    push(i, line, mark(Style::new()));
                }
            }
            Kind::Thinking => push(i, "  ✻ thinking".into(), dim().italic()),
            Kind::ToolUse { name } => {
                let line = format!("  ⚙ {name}  {}", e.text);
                let line = if line.chars().count() > width {
                    let cut: String = line.chars().take(width.saturating_sub(1)).collect();
                    format!("{cut}…")
                } else {
                    line
                };
                push(i, line, mark(Style::new().fg(Color::Yellow)));
            }
            Kind::ToolResult { is_error } => {
                if e.text.is_empty() {
                    continue;
                }
                let color = if *is_error {
                    Color::Red
                } else {
                    Color::DarkGray
                };
                if view.show_output {
                    let (lines, extra) = transcript::clip_lines(&e.text);
                    for line in lines {
                        for l in hard_wrap(&format!("    {line}"), width) {
                            push(i, l, mark(Style::new().fg(color)));
                        }
                    }
                    if extra > 0 {
                        push(i, format!("    … {extra} more lines"), dim());
                    }
                } else {
                    let count = e.text.lines().count();
                    let summary = if *is_error {
                        format!("    ↳ error: {}", e.text.lines().next().unwrap_or_default())
                    } else {
                        format!("    ↳ {count} line(s) of output")
                    };
                    let summary: String = summary.chars().take(width).collect();
                    push(i, summary, mark(Style::new().fg(color)));
                }
            }
            Kind::Compaction { pre_tokens } => {
                let size = pre_tokens
                    .map(|t| format!(" · {} tokens before", human_tokens(t)))
                    .unwrap_or_default();
                push(i, String::new(), Style::new());
                push(
                    i,
                    format!("──── conversation compacted{size}{} ────", time(e)),
                    Style::new().fg(Color::Yellow).bold(),
                );
            }
            Kind::CompactSummary => {
                if view.show_output {
                    push(
                        i,
                        "  ▾ compaction summary".into(),
                        Style::new().fg(Color::Yellow),
                    );
                    for line in wrap(&e.text, 4) {
                        push(i, line, mark(dim()));
                    }
                } else {
                    let n = e.text.lines().count();
                    push(
                        i,
                        format!("  ▸ compaction summary ({n} lines, o to show)"),
                        mark(Style::new().fg(Color::Yellow)),
                    );
                }
            }
            Kind::Notice => {
                for line in wrap(&format!("※ {}", e.text), 2) {
                    push(i, line, mark(dim().italic()));
                }
            }
        }
    }
    rows
}

fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = &mut app.transcript else {
        return;
    };
    let mut block = panel("", Color::Cyan).title(
        Line::from(format!(" {} ", view.title))
            .bold()
            .fg(Color::Cyan),
    );
    let meta = match &view.branch {
        Some(branch) => format!(" {}  {branch} ", view.project),
        None => format!(" {} ", view.project),
    };
    block = block.title(Line::from(meta).fg(Color::DarkGray).right_aligned());
    let inner = block.inner(area);

    let key = (inner.width, view.show_output, view.query.clone());
    if view.rows_key.as_ref() != Some(&key) {
        view.rows = transcript_rows(view, inner.width);
        view.rows_key = Some(key);
    }
    let visible = inner.height as usize;
    let last_page = view.rows.len().saturating_sub(visible);
    if let Some(entry) = view.jump_to.take() {
        view.scroll = if entry == usize::MAX {
            last_page
        } else {
            // A little context above the target.
            view.rows
                .iter()
                .position(|r| r.entry >= entry)
                .unwrap_or(last_page)
                .saturating_sub(2)
        };
    }
    view.scroll = view.scroll.min(last_page);
    view.at_end = view.scroll == last_page;

    let mut position = format!(
        " {}-{} of {} ",
        (view.scroll + 1).min(view.rows.len()),
        (view.scroll + visible).min(view.rows.len()),
        view.rows.len()
    );
    if let Some(query) = &view.query {
        position = format!(
            " \"{query}\" {}/{} ·{position}",
            if view.matches.is_empty() {
                0
            } else {
                view.match_pos + 1
            },
            view.matches.len()
        );
    }
    let block = block.title_bottom(Line::from(position).right_aligned());
    let lines: Vec<Line> = view.rows[view.scroll..(view.scroll + visible).min(view.rows.len())]
        .iter()
        .map(|r| Line::from(Span::styled(r.text.clone(), r.style)))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

// ---- Footer and popups ----------------------------------------------------------

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = |t: &'static str| Span::raw(t).dark_gray();

    let line = match &app.input {
        Some(Input::Search) => Line::from(vec![
            Span::styled(" / ", Style::new().fg(Color::Black).bg(Color::Yellow)),
            Span::raw(format!(" {}", app.filter)),
            Span::styled("▌", Style::new().fg(Color::Yellow)),
            hint("   Enter keep · Esc clear"),
        ]),
        Some(Input::TranscriptSearch { text }) => Line::from(vec![
            Span::styled(" / ", Style::new().fg(Color::Black).bg(Color::Yellow)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Yellow)),
            hint("   Enter search this conversation · Esc cancel"),
        ]),
        Some(Input::FindAll { text }) => Line::from(vec![
            Span::styled(" find ", Style::new().fg(Color::Black).bg(Color::Magenta)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Magenta)),
            hint("   Enter search every conversation · Esc cancel"),
        ]),
        Some(Input::LogFilter { text }) => Line::from(vec![
            Span::styled(" / ", Style::new().fg(Color::Black).bg(Color::Yellow)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Yellow)),
            hint("   show only lines containing this · Enter apply · Esc clear"),
        ]),
        Some(Input::Tags { text, .. }) => Line::from(vec![
            Span::styled(" tags ", Style::new().fg(Color::Black).bg(Color::Magenta)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Magenta)),
            hint("   comma or space separated · Enter save · Esc cancel"),
        ]),
        Some(Input::Note { text, .. }) => Line::from(vec![
            Span::styled(" note ", Style::new().fg(Color::Black).bg(Color::Magenta)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Magenta)),
            hint("   empty to remove · Enter save · Esc cancel"),
        ]),
        Some(Input::Prompt { text, .. }) => Line::from(vec![
            Span::styled(" prompt ", Style::new().fg(Color::Black).bg(Color::Cyan)),
            Span::raw(format!(" {text}")),
            Span::styled("▌", Style::new().fg(Color::Cyan)),
            hint("   Enter send · Esc cancel"),
        ]),
        None => {
            if let Some((msg, is_error, _)) = &app.flash {
                let color = if *is_error { Color::Red } else { Color::Green };
                Line::from(Span::styled(
                    format!(" {msg}"),
                    Style::new().fg(color).bold(),
                ))
            } else if let Some(query) = app.finding() {
                Line::from(Span::styled(
                    format!(
                        " {} searching every conversation for \"{query}\"…",
                        app.spinner()
                    ),
                    Style::new().fg(Color::Magenta),
                ))
            } else if app.fetching_branches() {
                Line::from(Span::styled(
                    format!(" {} fetching branches…", app.spinner()),
                    Style::new().fg(Color::LightMagenta),
                ))
            } else if let Some(branch) = app.reviewing()
                && app.flash.is_none()
            {
                Line::from(Span::styled(
                    format!(" {} Claude is reviewing {branch}…", app.spinner()),
                    Style::new().fg(Color::LightMagenta),
                ))
            } else if app.summarizing() {
                Line::from(Span::styled(
                    format!(" {} building today's summary…", app.spinner()),
                    Style::new().fg(Color::LightMagenta),
                ))
            } else if app.prompt_running() {
                Line::from(Span::styled(
                    format!(" {} waiting for Claude's reply…", app.spinner()),
                    Style::new().fg(Color::Cyan),
                ))
            } else {
                let mut spans = Vec::new();
                for b in crate::keys::footer(app.context()) {
                    spans.push(Span::styled(
                        format!(" {} ", b.keys.split("  ").next().unwrap_or(b.keys)),
                        Style::new().fg(Color::Black).bg(Color::Gray),
                    ));
                    spans.push(
                        Span::raw(format!(" {}  ", b.footer.unwrap_or_default())).dark_gray(),
                    );
                }
                Line::from(spans)
            }
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// Splits a line into pieces of at most `width` characters.
fn hard_wrap(line: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}

fn centered(area: Rect, width: Constraint, height: Constraint) -> Rect {
    let [area] = Layout::vertical([height]).flex(Flex::Center).areas(area);
    let [area] = Layout::horizontal([width]).flex(Flex::Center).areas(area);
    area
}

fn draw_popup(frame: &mut Frame, app: &mut App) {
    match &mut app.popup {
        None => {}
        Some(Popup::Text {
            title,
            lines,
            scroll,
        }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(85),
                Constraint::Percentage(80),
            );
            frame.render_widget(Clear, area);
            // Wrap to the popup's inner width up front (borders + padding take 4
            // columns), so scrolling and the counter work in screen rows.
            let width = area.width.saturating_sub(4).max(1) as usize;
            let rows: Vec<String> = lines.iter().flat_map(|l| hard_wrap(l, width)).collect();
            // Don't scroll past the last page: the end of a log should fill the popup.
            let visible = area.height.saturating_sub(2) as usize;
            let last_page = rows.len().saturating_sub(visible).min(u16::MAX as usize) as u16;
            *scroll = (*scroll).min(last_page);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::Cyan))
                .padding(Padding::horizontal(1))
                .title(Line::from(title.clone()).bold().fg(Color::Cyan))
                .title_bottom(
                    Line::from(format!(
                        " {}-{} of {} · ↑/↓ PgUp/PgDn scroll · Esc close ",
                        (*scroll as usize + 1).min(rows.len()),
                        (*scroll as usize + visible).min(rows.len()),
                        rows.len()
                    ))
                    .right_aligned(),
                );
            let text: Vec<Line> = rows.into_iter().map(Line::from).collect();
            let paragraph = Paragraph::new(text).block(block).scroll((*scroll, 0));
            frame.render_widget(paragraph, area);
        }
        Some(Popup::Trash {
            items,
            state,
            purge,
        }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(80),
                Constraint::Percentage(70),
            );
            frame.render_widget(Clear, area);
            let now = chrono::Utc::now().timestamp();
            let items_view: Vec<ListItem> = items
                .iter()
                .map(|t| {
                    let days_left = crate::library::TRASH_DAYS - (now - t.trashed_at) / 86_400;
                    ListItem::new(vec![
                        Line::from(Span::styled(t.title.clone(), Style::new().bold())),
                        Line::from(Span::styled(
                            format!(
                                "  {}  · {:.1} MB · deleted for good in {} day(s)",
                                t.project,
                                t.size as f64 / 1e6,
                                days_left.max(0)
                            ),
                            dim(),
                        )),
                    ])
                })
                .collect();
            let hint_text = if purge.is_some() {
                " press D again to delete for good · any other key cancels "
            } else {
                " Enter restore · D delete for good · Esc close "
            };
            let block =
                panel("Trash", Color::Red).title_bottom(Line::from(hint_text).right_aligned());
            if items_view.is_empty() {
                frame.render_widget(message("The trash is empty.", block), area);
            } else {
                let list = List::new(items_view)
                    .block(block)
                    .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                    .highlight_symbol("▶ ")
                    .highlight_spacing(HighlightSpacing::Always);
                frame.render_stateful_widget(list, area, state);
            }
        }
        Some(Popup::Cleanup { preset }) => {
            let chosen = CLEANUP_PRESETS[*preset];
            let area = centered(
                frame.area(),
                Constraint::Percentage(80),
                Constraint::Percentage(70),
            );
            frame.render_widget(Clear, area);
            let candidates = app.cleanup_candidates(chosen);
            let bytes: u64 = candidates.iter().map(|s| s.size).sum();
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("◀ ", dim()),
                    Span::styled(format!("Sessions {}", chosen.label()), Style::new().bold()),
                    Span::styled(" ▶", dim()),
                ]),
                Line::from(Span::styled(
                    format!(
                        "{} session(s), {:.1} MB. Open and starred sessions are never included.",
                        candidates.len(),
                        bytes as f64 / 1e6
                    ),
                    dim(),
                )),
                Line::default(),
            ];
            lines.extend(
                candidates
                    .iter()
                    .take(area.height.saturating_sub(8) as usize)
                    .map(|s| {
                        Line::from(vec![
                            Span::raw(format!("  {}", s.title)),
                            Span::styled(
                                format!(
                                    "  {} · {} · {:.1} MB",
                                    s.project_path,
                                    sessions::relative_age(s.modified),
                                    s.size as f64 / 1e6
                                ),
                                dim(),
                            ),
                        ])
                    }),
            );
            let block = panel("Clean up", Color::Yellow).title_bottom(
                Line::from(" ←/→ criteria · Enter move them to the trash · Esc cancel ")
                    .right_aligned(),
            );
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Some(Popup::Prompts {
            query,
            matches,
            state,
        }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(85),
                Constraint::Percentage(80),
            );
            frame.render_widget(Clear, area);
            let items: Vec<ListItem> = matches
                .iter()
                .take(500)
                .map(|&i| {
                    let p = &app.prompts[i];
                    let when =
                        p.at.map(|t| t.format("%b %d %H:%M").to_string())
                            .unwrap_or_default();
                    let first = p.text.lines().next().unwrap_or_default().to_string();
                    let lines = p.text.lines().count();
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::raw(first),
                            Span::styled(
                                if lines > 1 {
                                    format!("  (+{} lines)", lines - 1)
                                } else {
                                    String::new()
                                },
                                dim(),
                            ),
                        ]),
                        Line::from(Span::styled(format!("  {when} · {}", p.project), dim())),
                    ])
                })
                .collect();
            let block = panel("", Color::Magenta)
                .title(
                    Line::from(format!(" Prompt history · {} of {} ", matches.len(), app.prompts.len()))
                        .bold()
                        .fg(Color::Magenta),
                )
                .title(
                    Line::from(format!(" search: {query}▌ "))
                        .fg(Color::Yellow)
                        .right_aligned(),
                )
                .title_bottom(
                    Line::from(" type to search · Enter use as prompt for the selected session · Tab copy · Esc close ")
                        .right_aligned(),
                );
            if items.is_empty() {
                frame.render_widget(message("No prompts match.", block), area);
            } else {
                let list = List::new(items)
                    .block(block)
                    .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                    .highlight_symbol("▶ ")
                    .highlight_spacing(HighlightSpacing::Always);
                frame.render_stateful_widget(list, area, state);
            }
        }
        Some(Popup::Summary { markdown, scroll }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(85),
                Constraint::Percentage(85),
            );
            frame.render_widget(Clear, area);
            let width = area.width.saturating_sub(4).max(1) as usize;
            let rows: Vec<Line> = markdown
                .lines()
                .flat_map(|l| {
                    let style = if l.starts_with("# ") {
                        Style::new().fg(Color::LightMagenta).bold()
                    } else if l.starts_with("## ") {
                        Style::new().fg(Color::Cyan).bold()
                    } else if l.starts_with("**") || l.starts_with("- **") {
                        Style::new().bold()
                    } else if l.trim_start().starts_with("- `") || l.starts_with("Commits") {
                        Style::new().fg(Color::Green)
                    } else {
                        Style::new()
                    };
                    let text = l
                        .trim_start_matches("# ")
                        .trim_start_matches("## ")
                        .replace("**", "");
                    hard_wrap(&text, width)
                        .into_iter()
                        .map(move |r| Line::from(Span::styled(r, style)))
                })
                .collect();
            let visible = area.height.saturating_sub(2) as usize;
            let last_page = rows.len().saturating_sub(visible).min(u16::MAX as usize) as u16;
            *scroll = (*scroll).min(last_page);
            let block = panel("Today", Color::LightMagenta).title_bottom(
                Line::from(" e export to Markdown · ↑/↓ scroll · Esc close ").right_aligned(),
            );
            frame.render_widget(Paragraph::new(rows).block(block).scroll((*scroll, 0)), area);
        }
        Some(Popup::Branches {
            listing,
            reviewed,
            query,
            matches,
            state,
            repo_name,
            ..
        }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(85),
                Constraint::Percentage(80),
            );
            frame.render_widget(Clear, area);
            let now = chrono::Utc::now().timestamp();
            let items: Vec<ListItem> = matches
                .iter()
                .map(|&i| {
                    let b = &listing.branches[i];
                    let mut first = vec![Span::styled(b.name.clone(), Style::new().bold())];
                    if b.local {
                        first.push(Span::styled(
                            "  your checkout · review before pushing",
                            Style::new().fg(Color::Yellow),
                        ));
                    }
                    if b.ahead > 0 {
                        first.push(Span::styled(
                            format!("  +{} commit(s)", b.ahead),
                            Style::new().fg(Color::Cyan),
                        ));
                    }
                    if let Some(at) = reviewed.get(&b.name) {
                        first.push(Span::styled(
                            format!("  ✓ reviewed {} ago", short_age(now - at)),
                            Style::new().fg(Color::Green),
                        ));
                    }
                    ListItem::new(vec![
                        Line::from(first),
                        Line::from(Span::styled(
                            format!(
                                "  {} · {} · {} ago",
                                b.subject,
                                b.author,
                                short_age(now - b.when)
                            ),
                            dim(),
                        )),
                    ])
                })
                .collect();
            let base = listing
                .base
                .as_deref()
                .map_or("no develop, main or master".to_string(), |b| {
                    format!("against {b}")
                });
            let block = panel("", Color::Magenta)
                .title(
                    Line::from(format!(
                        " Review a branch · {repo_name} · {base} · {} of {} ",
                        matches.len(),
                        listing.branches.len()
                    ))
                    .bold()
                    .fg(Color::Magenta),
                )
                .title(
                    Line::from(format!(" search: {query}▌ "))
                        .fg(Color::Yellow)
                        .right_aligned(),
                )
                .title_bottom(
                    Line::from(" type to search · Enter choose · Esc close ").right_aligned(),
                );
            if items.is_empty() {
                let text = if listing.branches.is_empty() {
                    "The remote has no other branches."
                } else {
                    "No branches match."
                };
                frame.render_widget(message(text, block), area);
            } else {
                let list = List::new(items)
                    .block(block)
                    .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                    .highlight_symbol("▶ ")
                    .highlight_spacing(HighlightSpacing::Always);
                frame.render_stateful_widget(list, area, state);
            }
        }
        Some(Popup::ReviewSetup {
            task,
            stat,
            last,
            choice,
        }) => {
            let area = centered(frame.area(), Constraint::Max(90), Constraint::Max(26));
            frame.render_widget(Clear, area);
            let width = area.width.saturating_sub(8).max(20) as usize;
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(task.branch.clone(), Style::new().bold()),
                    Span::styled(format!("  against {}", task.base), dim()),
                ]),
                match stat {
                    Ok(st) => Line::from(format!(
                        "{} · {} · +{} −{}",
                        plural(st.commits as u64, "commit"),
                        plural(st.files as u64, "file"),
                        st.insertions,
                        st.deletions
                    )),
                    Err(e) => Line::from(Span::styled(e.clone(), Style::new().fg(Color::Red))),
                },
                Line::from(Span::styled(
                    "Checked out in a worktree of its own; your checkout isn't touched.",
                    dim(),
                )),
                Line::from(Span::styled(
                    if task.reference.contains('/') {
                        ""
                    } else {
                        "Your local branch: only committed work is reviewed."
                    },
                    Style::new().fg(Color::Yellow),
                )),
                Line::default(),
            ];
            let mut options: Vec<(&str, &str)> = crate::review::Mode::ALL
                .iter()
                .map(|m| (m.title(), m.detail()))
                .collect();
            let last_label = last.as_ref().map(|r| {
                format!(
                    "Show the last review ({})",
                    plural(r.result.findings.len() as u64, "finding")
                )
            });
            if let Some(label) = &last_label {
                options.push((
                    label.as_str(),
                    "Open the findings Claude gave last time, without reviewing again.",
                ));
            }
            for (i, (title, detail)) in options.iter().enumerate() {
                let selected = i == *choice;
                lines.push(Line::from(vec![
                    Span::raw(if selected { "▶ " } else { "  " }),
                    Span::styled(
                        title.to_string(),
                        if selected {
                            Style::new().fg(Color::Yellow).bold()
                        } else {
                            Style::new().bold()
                        },
                    ),
                ]));
                if selected {
                    for chunk in crate::app::textwrap(detail, width) {
                        lines.push(Line::from(Span::styled(format!("    {chunk}"), dim())));
                    }
                }
            }
            let block = panel("Review", Color::Magenta).title_bottom(
                Line::from(" ↑/↓ choose · Enter start · Esc cancel ").right_aligned(),
            );
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Some(Popup::Findings {
            review,
            state,
            detail,
        }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(90),
                Constraint::Percentage(85),
            );
            frame.render_widget(Clear, area);
            let severity_color = |s: &str| match s {
                "high" => Color::Red,
                "medium" => Color::Yellow,
                _ => Color::Cyan,
            };
            let title = Line::from(format!(
                " Review · {} against {} · {} ",
                review.branch,
                review.base,
                plural(review.result.findings.len() as u64, "finding")
            ))
            .bold()
            .fg(Color::Magenta);
            let block = panel("", Color::Magenta).title(title);
            let width = area.width.saturating_sub(4).max(10) as usize;
            let selected = state.selected().unwrap_or(0);
            match (detail, review.result.findings.get(selected)) {
                (Some(scroll), Some(f)) => {
                    let place = f.line.map_or(f.file.clone(), |l| format!("{}:{l}", f.file));
                    let mut lines = vec![
                        Line::from(vec![
                            Span::styled(
                                format!(" {} ", f.severity),
                                Style::new()
                                    .fg(Color::Black)
                                    .bg(severity_color(&f.severity)),
                            ),
                            Span::styled(format!("  {place}"), Style::new().bold()),
                        ]),
                        Line::default(),
                    ];
                    for chunk in crate::app::textwrap(&f.comment, width) {
                        lines.push(Line::from(chunk));
                    }
                    if let Some(s) = f.suggestion.as_deref().filter(|s| !s.trim().is_empty()) {
                        lines.push(Line::default());
                        lines.push(Line::from(Span::styled(
                            "Suggested change",
                            Style::new().bold(),
                        )));
                        for l in s.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("  {l}"),
                                Style::new().fg(Color::Green),
                            )));
                        }
                    }
                    if let Some(line) = f.line {
                        let context = crate::review::context(&review.worktree, &f.file, line, 6);
                        if !context.is_empty() {
                            lines.push(Line::default());
                            lines.push(Line::from(Span::styled(
                                format!("{place} in the branch"),
                                Style::new().bold(),
                            )));
                            for l in context {
                                let style = if l.starts_with('▶') {
                                    Style::new().fg(Color::Yellow)
                                } else {
                                    dim()
                                };
                                lines.push(Line::from(Span::styled(l, style)));
                            }
                        }
                    }
                    let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
                    *scroll = (*scroll).min(max);
                    let block = block.title_bottom(
                        Line::from(" Tab copy the comment · ↑/↓ scroll · Esc back to the list ")
                            .right_aligned(),
                    );
                    frame.render_widget(
                        Paragraph::new(lines)
                            .block(block)
                            .wrap(Wrap { trim: false })
                            .scroll((*scroll, 0)),
                        area,
                    );
                }
                _ => {
                    let [top, list_area] = Layout::vertical([
                        Constraint::Length(
                            (crate::app::textwrap(&review.result.summary, width).len() as u16 + 2)
                                .min(8),
                        ),
                        Constraint::Min(3),
                    ])
                    .areas(block.inner(area));
                    frame.render_widget(
                        block.title_bottom(
                            Line::from(
                                " Enter details · Tab copy · e export · v read the session · D remove worktree · Esc close ",
                            )
                            .right_aligned(),
                        ),
                        area,
                    );
                    frame.render_widget(
                        Paragraph::new(review.result.summary.clone()).wrap(Wrap { trim: true }),
                        top,
                    );
                    if review.result.findings.is_empty() {
                        frame.render_widget(
                            Paragraph::new("Nothing worth a comment.").style(dim()),
                            list_area,
                        );
                    } else {
                        let items: Vec<ListItem> = review
                            .result
                            .findings
                            .iter()
                            .map(|f| {
                                let place =
                                    f.line.map_or(f.file.clone(), |l| format!("{}:{l}", f.file));
                                let first = f.comment.lines().next().unwrap_or_default();
                                ListItem::new(vec![
                                    Line::from(vec![
                                        Span::styled(
                                            format!("{:<7}", f.severity),
                                            Style::new().fg(severity_color(&f.severity)).bold(),
                                        ),
                                        Span::styled(place, Style::new().bold()),
                                    ]),
                                    Line::from(Span::styled(format!("       {first}"), dim())),
                                ])
                            })
                            .collect();
                        let list = List::new(items)
                            .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                            .highlight_symbol("▶ ")
                            .highlight_spacing(HighlightSpacing::Always);
                        frame.render_stateful_widget(list, list_area, state);
                    }
                }
            }
        }
        Some(Popup::Wrapped { month, redact }) => {
            let (month, redact) = (*month, *redact);
            let stats = app.wrapped_stats(month);
            let lines = crate::wrapped::card(&stats, redact);
            let width = lines.iter().map(|l| l.width()).max().unwrap_or(40) as u16 + 6;
            let height = lines.len() as u16 + 4;
            let area = centered(
                frame.area(),
                Constraint::Max(width),
                Constraint::Max(height),
            );
            frame.render_widget(Clear, area);
            let block = card(&crate::wrapped::title(&stats), ACCENT, true)
                .padding(Padding::new(2, 2, 1, 0))
                .title_bottom(Line::from(" made with claudash ").left_aligned())
                .title_bottom(
                    Line::from(format!(
                        " Tab {} · x {} names · e export · Esc ",
                        if month { "week" } else { "month" },
                        if redact { "show" } else { "hide" }
                    ))
                    .right_aligned(),
                );
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Some(Popup::Palette {
            commands,
            query,
            matches,
            state,
        }) => {
            let height = (matches.len() as u16 + 2).clamp(5, 24);
            let area = centered(
                frame.area(),
                Constraint::Max(96),
                Constraint::Length(height),
            );
            frame.render_widget(Clear, area);
            let width = area.width.saturating_sub(6) as usize;
            let items: Vec<ListItem> = matches
                .iter()
                .map(|&i| {
                    let b = commands[i];
                    let label = format!("{:<13}", b.context.title());
                    let keys = format!(" {} ", b.keys);
                    let room = width
                        .saturating_sub(label.chars().count() + keys.chars().count())
                        .max(1);
                    let mut what: String = b.what.chars().take(room).collect();
                    if b.what.chars().count() > room {
                        what.pop();
                        what.push('…');
                    }
                    let pad = room.saturating_sub(what.chars().count());
                    ListItem::new(Line::from(vec![
                        Span::styled(label, dim()),
                        Span::raw(what),
                        Span::raw(" ".repeat(pad)),
                        Span::styled(keys, Style::new().fg(Color::Yellow).bold()),
                    ]))
                })
                .collect();
            let block = panel("", Color::Magenta)
                .title(Line::from(" Commands ").bold().fg(Color::Magenta))
                .title(
                    Line::from(format!(" {query}▌ "))
                        .fg(Color::Yellow)
                        .right_aligned(),
                )
                .title_bottom(
                    Line::from(" type to filter · Enter run · Esc close ").right_aligned(),
                );
            if items.is_empty() {
                frame.render_widget(message("No command matches.", block), area);
            } else {
                let list = List::new(items)
                    .block(block)
                    .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                    .highlight_symbol("▶ ")
                    .highlight_spacing(HighlightSpacing::Always);
                frame.render_stateful_widget(list, area, state);
            }
        }
        Some(Popup::Results { query, hits, state }) => {
            let area = centered(
                frame.area(),
                Constraint::Percentage(85),
                Constraint::Percentage(80),
            );
            frame.render_widget(Clear, area);
            let items: Vec<ListItem> = hits
                .iter()
                .map(|hit| {
                    let when = hit
                        .at
                        .map(|t| t.format("%b %d %H:%M").to_string())
                        .unwrap_or_default();
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(hit.title.clone(), Style::new().bold()),
                            Span::styled(format!("  {when}"), dim()),
                        ]),
                        Line::from(Span::styled(
                            format!("  {}", hit.snippet),
                            Style::new().fg(Color::Gray),
                        )),
                    ])
                })
                .collect();
            let more = if hits.len() >= 300 { "300+" } else { "" };
            let block = panel("", Color::Magenta)
                .title(
                    Line::from(format!(
                        " \"{query}\" · {}{more} matches ",
                        if more.is_empty() {
                            hits.len().to_string()
                        } else {
                            String::new()
                        }
                    ))
                    .bold()
                    .fg(Color::Magenta),
                )
                .title_bottom(Line::from(" Enter open at the match · Esc close ").right_aligned());
            let list = List::new(items)
                .block(block)
                .highlight_style(Style::new().fg(Color::White).bg(HIGHLIGHT))
                .highlight_symbol("▶ ")
                .highlight_spacing(HighlightSpacing::Always);
            frame.render_stateful_widget(list, area, state);
        }
        Some(Popup::Confirm {
            title, lines, yes, ..
        }) => {
            let height = lines.len() as u16 + 5;
            let area = centered(
                frame.area(),
                Constraint::Length(72),
                Constraint::Length(height),
            );
            frame.render_widget(Clear, area);
            let mut text: Vec<Line> = lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    if i == 0 {
                        Line::from(Span::styled(l.clone(), Style::new().bold()))
                    } else {
                        Line::from(l.clone()).dark_gray()
                    }
                })
                .collect();
            text.push(Line::default());
            text.push(Line::from(vec![
                Span::styled(" y ", Style::new().fg(Color::Black).bg(Color::Red)),
                Span::raw(format!(" {yes}   ")),
                Span::styled(" n ", Style::new().fg(Color::Black).bg(Color::Gray)),
                Span::raw(" cancel"),
            ]));
            let paragraph = Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .block(panel(title, Color::Red));
            frame.render_widget(paragraph, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_wraps_by_characters() {
        assert_eq!(hard_wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(hard_wrap("", 4), [""]);
        assert_eq!(hard_wrap("naïveté", 3), ["naï", "vet", "é"]);
    }

    #[test]
    fn labels_tools_and_tokens() {
        assert_eq!(
            tool_label("mcp__plugin_chrome-devtools-mcp_chrome-devtools__take_screenshot"),
            "chrome-devtools › take_screenshot"
        );
        assert_eq!(
            tool_label("mcp__playwright__browser_click"),
            "playwright › browser_click"
        );
        assert_eq!(tool_label("Bash"), "Bash");
        assert_eq!(parse_tokens("~1,067"), 1067);
        assert_eq!(plural(1, "use"), "1 use");
        assert_eq!(plural(3, "use"), "3 uses");
    }
}
