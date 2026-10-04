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
    ACCENT, GAUGE_TRACK, HIGHLIGHT, card, dim, draw_activity, forecast_span, level_color,
    plan_windows, problem_line, reset_time,
};
use crate::{
    app::{ActivityFocus, Alert, App},
    paths,
};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .spacing(1)
            .areas(area);
    draw_activity(frame, app, left);

    let alerts = app.alerts();
    let plan_height = if plan_windows(app).is_empty() { 5 } else { 8 };
    let [alerts_area, plan_area] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(plan_height)])
            .spacing(1)
            .areas(right);
    let focused = app.activity_focus == ActivityFocus::Alerts;
    if alerts.is_empty() {
        let lines = vec![
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
        ];
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(card(
                "All clear",
                Color::Green,
                false,
            )),
            alerts_area,
        );
    } else {
        let cursor = app.alert_cursor.min(alerts.len() - 1);
        let width = alerts_area.width.saturating_sub(4).max(10) as usize;
        let mut lines = Vec::new();
        let mut selected_row = 0;
        let mut rows = 0;
        for (i, alert) in alerts.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
                rows += 1;
            }
            let mut line = alert_line(alert);
            let chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            if focused && i == cursor {
                selected_row = rows;
                line.spans
                    .insert(0, Span::styled("▶ ", Style::new().fg(ACCENT).bold()));
                if app.alert_target(alert).is_some() {
                    line.spans
                        .push(Span::styled("  → Enter", Style::new().fg(ACCENT)));
                }
                line = line.style(Style::new().fg(Color::White).bg(HIGHLIGHT));
            }
            rows += chars.div_ceil(width).max(1);
            lines.push(line);
        }
        // Keep the selected alert in view.
        let height = alerts_area.height.saturating_sub(2) as usize;
        let scroll = selected_row.saturating_sub(height.saturating_sub(4)) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .block(card(
                    &format!("Needs a look · {}", alerts.len()),
                    Color::Red,
                    focused,
                )),
            alerts_area,
        );
    }
    draw_plan_card(frame, app, plan_area);
}

/// One alert as a line.
fn alert_line(alert: &Alert) -> Line<'static> {
    match alert {
        Alert::Problem(p) => problem_line(p),
        Alert::Mcp {
            dir, name, failed, ..
        } => {
            let (mark, color, what) = if *failed {
                ("✗ ", Color::Red, "failed to connect")
            } else {
                ("◐ ", Color::Yellow, "needs you to sign in")
            };
            Line::from(vec![
                Span::styled(mark, Style::new().fg(color)),
                Span::styled(format!("MCP {name}"), Style::new().fg(color)),
                Span::styled(format!(" {what} · {}", paths::display(dir)), dim()),
            ])
        }
    }
}

fn draw_plan_card(frame: &mut Frame, app: &App, area: Rect) {
    // What today and the last 7 days would have cost at API prices.
    let days = app.history.per_day();
    let today = chrono::Local::now().date_naive();
    let week: f64 = days
        .range(today - chrono::Days::new(6)..)
        .map(|(_, u)| u.cost)
        .sum();
    let today_cost = days.get(&today).map_or(0.0, |u| u.cost);
    let title = format!(
        "Plan · ≈{} today · ≈{} this week",
        crate::pricing::format_usd(today_cost),
        crate::pricing::format_usd(week)
    );
    let block = card(&title, Color::LightMagenta, false);
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
                .gauge_style(Style::new().fg(level_color(ratio)).bg(GAUGE_TRACK))
                .ratio(ratio.clamp(0.0, 1.0))
                .label(format!("{:.0}%", w.used_percentage).bold())
                .use_unicode(true),
            bar,
        );
    }
}
