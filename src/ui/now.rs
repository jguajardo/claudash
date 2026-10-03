//! Now: what needs you. Open sessions and the live feed on the left; alerts
//! and plan usage on the right.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Gauge, Paragraph, Wrap},
};

use super::{
    card, dim, draw_activity, forecast_span, level_color, plan_windows, problem_line, reset_time,
};
use crate::{app::App, mcp::McpStatus, paths};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .spacing(1)
            .areas(area);
    draw_activity(frame, app, left);

    let alerts = alert_lines(app);
    let plan_height = if plan_windows(app).is_empty() { 5 } else { 8 };
    let [alerts_area, plan_area] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(plan_height)])
            .spacing(1)
            .areas(right);
    let (title, color) = if alerts.is_empty() {
        ("All clear", Color::Green)
    } else {
        ("Needs a look", Color::Red)
    };
    let lines = if alerts.is_empty() {
        vec![
            Line::default(),
            Line::from(Span::styled(
                "✓ Nothing needs you.",
                Style::new().fg(Color::Green).bold(),
            )),
            Line::default(),
            Line::from(Span::styled(
                "No secrets in transcripts, no risky commands this week, MCP servers fine, \
                 no forgotten worktrees or stale specs.",
                dim(),
            )),
        ]
    } else {
        alerts
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(card(title, color, false))
            .scroll((0, 0)),
        alerts_area,
    );
    draw_plan_card(frame, app, plan_area);
}

/// Alerts across projects, one blank line between them.
fn alert_lines(app: &App) -> Vec<Line<'static>> {
    let mut items: Vec<Line<'static>> = app.problems().iter().map(problem_line).collect();
    // MCP servers checked so far that failed or need you to sign in.
    for (dir, snapshot) in &app.mcp_cache {
        let Ok(servers) = &snapshot.result else {
            continue;
        };
        for s in servers {
            let (mark, color, what) = match &s.status {
                McpStatus::Failed(_) => ("✗ ", Color::Red, "failed to connect"),
                McpStatus::NeedsAuth => ("◐ ", Color::Yellow, "needs you to sign in"),
                _ => continue,
            };
            items.push(Line::from(vec![
                Span::styled(mark, Style::new().fg(color)),
                Span::styled(format!("MCP {}", s.name), Style::new().fg(color)),
                Span::styled(format!(" {what} · {}", paths::display(dir)), dim()),
            ]));
        }
    }
    let mut out = Vec::new();
    for (i, line) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        out.push(line);
    }
    out
}

fn draw_plan_card(frame: &mut Frame, app: &App, area: Rect) {
    let block = card("Plan", Color::LightMagenta, false);
    let windows = plan_windows(app);
    if windows.is_empty() {
        let text = if app.statusline.configured {
            "No plan usage reported yet (Pro and Max only, after a session's first reply)."
        } else {
            "Plan usage needs Claude Code's status line: run `claudash setup`."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(text, dim()))
                .wrap(Wrap { trim: false })
                .block(block),
            area,
        );
        return;
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical(vec![Constraint::Length(3); windows.len()]).split(inner);
    for ((label, w), row) in windows.iter().zip(rows.iter()) {
        let [head, bar, _] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(*row);
        let ratio = w.used_percentage / 100.0;
        let name = if *label == "5h" { "5-hour" } else { "7-day" };
        let mut spans = vec![
            Span::styled(format!("{name}  "), Style::new().bold()),
            Span::styled(format!("resets {}", reset_time(w)), dim()),
        ];
        if let Some(forecast) = forecast_span(app, label) {
            spans.push(Span::raw("  "));
            spans.push(forecast);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), head);
        frame.render_widget(
            Gauge::default()
                .gauge_style(
                    Style::new()
                        .fg(level_color(ratio))
                        .bg(Color::Rgb(40, 40, 40)),
                )
                .ratio(ratio.clamp(0.0, 1.0))
                .label(format!("{:.0}%", w.used_percentage).bold())
                .use_unicode(true),
            bar,
        );
    }
}
