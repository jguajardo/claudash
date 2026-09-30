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
        ListItem, Padding, Paragraph, Row, Table, Tabs, Wrap,
    },
};

use crate::{
    app::{
        App, CLEANUP_PRESETS, EcoTab, Focus, Input, McpSnapshot, Popup, TranscriptRow,
        TranscriptView, View,
    },
    ecosystem::Item,
    hooks::Activity,
    mcp::McpStatus,
    paths,
    sessions::{self, Usage, human_tokens},
    statusline::{self, Forecast, Window},
    transcript::{self, Kind},
};

const HIGHLIGHT: Color = Color::Rgb(60, 40, 70);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);
    match app.view {
        View::Dashboard => draw_dashboard(frame, app, body),
        View::Ecosystem => draw_ecosystem(frame, app, body),
        View::Usage => draw_usage(frame, app, body),
        View::Transcript => draw_transcript(frame, app, body),
        View::Help => draw_help(frame, app, body),
    }
    draw_footer(frame, app, footer);
    draw_popup(frame, app);
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
        .border_style(Style::new().fg(accent))
        .padding(Padding::horizontal(1))
}

fn focused(block: Block<'_>, is_focused: bool) -> Block<'_> {
    if is_focused {
        block.border_type(BorderType::Thick)
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
    let tabs = [
        (View::Dashboard, "1 Dashboard"),
        (View::Ecosystem, "2 Ecosystem"),
        (View::Usage, "3 Usage"),
    ];
    let mut spans = vec![
        Span::styled(
            " ◆ claudash ",
            Style::new().fg(Color::Black).bg(Color::LightMagenta).bold(),
        ),
        Span::raw("  "),
    ];
    for (view, label) in tabs {
        let style = if app.view == view {
            Style::new().fg(Color::White).bg(HIGHLIGHT).bold()
        } else {
            dim()
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    if app.view == View::Help {
        spans.push(Span::styled(
            " help ",
            Style::new().fg(Color::White).bg(HIGHLIGHT).bold(),
        ));
    }
    if app.view == View::Transcript {
        spans.push(Span::styled(
            " conversation ",
            Style::new().fg(Color::White).bg(HIGHLIGHT).bold(),
        ));
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
    let project_lines = project_lines(app);
    // Borders + content, but never more than half of the column.
    let project_height = (project_lines.len() as u16 + 2)
        .min(right.height / 2)
        .max(3);
    let [project, mcp] =
        Layout::vertical([Constraint::Length(project_height), Constraint::Min(3)]).areas(right);

    draw_sessions(frame, app, sessions);
    draw_project(frame, app, project, project_lines);
    draw_mcp(frame, app, mcp);
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
    block = focused(block, app.focus == Focus::Sessions);
    if !app.hooks_configured || !app.statusline.configured {
        block = block.title_bottom(
            Line::from(" run `claudash setup` for alerts and plan usage ").dark_gray(),
        );
    }
    if !app.filter.is_empty() {
        block = block.title(Line::from(format!(" filter: {} ", app.filter)).fg(Color::Yellow));
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
            let mut lines = vec![Line::from(title), Line::from(detail)];
            if let Some(note) = meta.map(|m| m.note.as_str()).filter(|n| !n.is_empty()) {
                lines.push(Line::from(Span::styled(
                    format!("  ✎ {note}"),
                    dim().italic(),
                )));
            }
            ListItem::new(lines)
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(HIGHLIGHT).add_modifier(Modifier::BOLD))
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

fn draw_mcp(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = focused(panel("MCP Status", Color::Cyan), app.focus == Focus::Mcp);
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
    if app.focus == Focus::Mcp {
        list = list
            .highlight_style(Style::new().bg(HIGHLIGHT))
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
        .gauge_style(
            Style::new()
                .fg(level_color(ratio))
                .bg(Color::Rgb(40, 40, 40)),
        )
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
    if let Some(cost) = t.cost_usd {
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
    let Some(cache) = snapshot_cache else {
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
        Some(p) => format!(" user + {} ", paths::display(&p.cwd)),
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
                if let Some(Ok(details)) = app.plugin_details.get(&p.id)
                    && let Some(tokens) = crate::ecosystem::always_on_tokens(details)
                {
                    spans.push(Span::styled(
                        format!("  {tokens} tok/session"),
                        Style::new().fg(Color::Yellow),
                    ));
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
    let items: Vec<ListItem> = list
        .iter()
        .map(|item| {
            ListItem::new(Line::from(vec![
                Span::styled(item.name.clone(), Style::new().bold()),
                Span::styled(format!("  {}", item.source), dim()),
            ]))
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
        .highlight_style(Style::new().bg(HIGHLIGHT))
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
            *months.entry((day.year(), day.month())).or_default() += usage.processed();
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
                let tokens = days.get(&day).map_or(0, Usage::processed);
                bar(day.format("%d").to_string(), tokens, day == today)
            })
            .collect();
        (bars, format!("last {n} days"), peak)
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
    let chart = BarChart::default()
        .block(
            panel(
                if app.monthly {
                    "Tokens per month"
                } else {
                    "Tokens per day"
                },
                Color::Cyan,
            )
            .title_bottom(
                Line::from(format!(
                    " input + cache write + output · {range} · peak {}{since} · m days/months ",
                    human_tokens(peak)
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

    let header = Row::new(["", "input", "c.write", "c.read", "output"]).style(dim());
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
        .map(|(project, tokens)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>7}  ", human_tokens(tokens)),
                    Style::new().bold(),
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
            .gauge_style(
                Style::new()
                    .fg(level_color(ratio))
                    .bg(Color::Rgb(40, 40, 40)),
            )
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
    let key = |k: &'static str| Span::styled(k, Style::new().fg(Color::Black).bg(Color::Gray));
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
            } else if app.prompt_running() {
                Line::from(Span::styled(
                    format!(" {} waiting for Claude's reply…", app.spinner()),
                    Style::new().fg(Color::Cyan),
                ))
            } else {
                let mut spans = match (app.view, app.focus) {
                    (View::Dashboard, Focus::Sessions) => vec![
                        key(" Enter "),
                        hint(" resume  "),
                        key(" / "),
                        hint(" search  "),
                        key(" p "),
                        hint(" prompt  "),
                        key(" v "),
                        hint(" read  "),
                        key(" f "),
                        hint(" find  "),
                        key(" d "),
                        hint(" delete  "),
                        key(" Tab "),
                        hint(" MCP  "),
                    ],
                    (View::Dashboard, Focus::Mcp) => vec![
                        key(" Enter "),
                        hint(" server log  "),
                        key(" Tab "),
                        hint(" sessions  "),
                    ],
                    (View::Ecosystem, _) => vec![
                        key(" ←/→ "),
                        hint(" tab  "),
                        key(" Enter "),
                        hint(" details  "),
                        key(" Space "),
                        hint(" toggle plugin  "),
                    ],
                    (View::Usage, _) => vec![key(" Esc "), hint(" dashboard  ")],
                    (View::Help, _) => vec![key(" Esc "), hint(" back  ")],
                    (View::Transcript, _) => vec![
                        key(" / "),
                        hint(" search  "),
                        key(" n/N "),
                        hint(" next/prev  "),
                        key(" o "),
                        hint(" tool output  "),
                        key(" e "),
                        hint(" export .md  "),
                        key(" Esc "),
                        hint(" back  "),
                    ],
                };
                spans.extend([key(" r "), hint(" reload  "), key(" q "), hint(" quit")]);
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
                " press x again to delete for good · any other key cancels "
            } else {
                " u restore · x delete for good · Esc close "
            };
            let block =
                panel("Trash", Color::Red).title_bottom(Line::from(hint_text).right_aligned());
            if items_view.is_empty() {
                frame.render_widget(message("The trash is empty.", block), area);
            } else {
                let list = List::new(items_view)
                    .block(block)
                    .highlight_style(Style::new().bg(HIGHLIGHT))
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
                .highlight_style(Style::new().bg(HIGHLIGHT))
                .highlight_symbol("▶ ")
                .highlight_spacing(HighlightSpacing::Always);
            frame.render_stateful_widget(list, area, state);
        }
        Some(Popup::ConfirmDelete { title, .. }) => {
            let area = centered(frame.area(), Constraint::Length(64), Constraint::Length(8));
            frame.render_widget(Clear, area);
            let lines = vec![
                Line::from(vec![
                    Span::raw("Delete "),
                    Span::styled(format!("\"{title}\""), Style::new().bold()),
                    Span::raw("?"),
                ]),
                Line::default(),
                Line::from("It moves to the trash with its subagents and checkpoints;").dark_gray(),
                Line::from("press T to restore it within 30 days.").dark_gray(),
                Line::default(),
                Line::from(vec![
                    Span::styled(" y ", Style::new().fg(Color::Black).bg(Color::Red)),
                    Span::raw(" move to trash   "),
                    Span::styled(" n ", Style::new().fg(Color::Black).bg(Color::Gray)),
                    Span::raw(" cancel"),
                ]),
            ];
            let paragraph = Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(panel("Move to trash", Color::Red));
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
}
