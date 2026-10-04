//! Insights: a menu of sections on the left (plan & usage, where tokens go,
//! security), the section on the right.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::{card, dim, draw_menu, draw_token_report, draw_usage};
use crate::{
    app::{App, InsightsSection},
    audit::Severity,
};

const MENU_WIDTH: u16 = 30;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [menu, content] = Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
        .spacing(1)
        .areas(area);
    let entries: Vec<(String, String)> = InsightsSection::ALL
        .iter()
        .map(|s| (s.icon().to_string(), s.title().to_string()))
        .collect();
    let selected = InsightsSection::ALL
        .iter()
        .position(|s| *s == app.insights)
        .unwrap_or(0);
    draw_menu(frame, menu, "Insights", &entries, selected, true);
    match app.insights {
        InsightsSection::Usage => draw_usage(frame, app, content),
        InsightsSection::Quota => draw_quota(frame, app, content),
        InsightsSection::Tokens => draw_token_report(frame, app, content),
        InsightsSection::Security => draw_security(frame, app, content),
    }
}

/// Where the current 5-hour and 7-day windows went, side by side.
fn draw_quota(frame: &mut Frame, app: &App, area: Rect) {
    let now = chrono::Utc::now().timestamp();
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .areas(area);
    let five = crate::quota::five_hour(&app.sessions, &app.statusline, now);
    draw_window(frame, app, left, "5-hour window", five);
    let seven = crate::quota::seven_day(&app.statusline, now);
    draw_window(frame, app, right, "7-day window", Some(seven));
}

fn draw_window(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    name: &str,
    window: Option<crate::quota::Window>,
) {
    let Some(window) = window else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No request in the last five hours: nothing counts against this window.",
                dim(),
            ))
            .wrap(Wrap { trim: false })
            .block(card(name, Color::LightMagenta, false)),
            area,
        );
        return;
    };
    let ledger = crate::quota::ledger(&app.sessions, &window);
    let local = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%a %H:%M")
                    .to_string()
            })
            .unwrap_or_default()
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Since ", dim()),
            Span::styled(local(window.start), Style::new().bold()),
            Span::styled(
                format!(
                    "  ·  ≈{} API-equivalent",
                    crate::pricing::format_usd(ledger.total.cost)
                ),
                Style::new().fg(Color::Green),
            ),
        ]),
        Line::from(match window.used {
            Some(used) => Span::styled(
                format!("{used:.0}% of the limit used, split below by each session's cost"),
                Style::new().fg(Color::Yellow),
            ),
            None => Span::styled(
                if window.reported {
                    "Claude Code didn't report how much is used".to_string()
                } else {
                    "Estimated window: `claudash setup` gives the exact one and the % used"
                        .to_string()
                },
                dim(),
            ),
        }),
        Line::default(),
    ];
    let bar_width = 12usize;
    let bar = |fraction: f64| {
        let filled = ((fraction * bar_width as f64).round() as usize).min(bar_width);
        format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled))
    };
    lines.push(Line::from(Span::styled(
        "Sessions",
        Style::new().fg(Color::LightMagenta).bold(),
    )));
    if ledger.sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no requests in this window",
            dim(),
        )));
    }
    for share in ledger.sessions.iter().take(8) {
        let points = share
            .limit_points(&window)
            .map(|p| format!(" ≈{p:.0} pts"))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", bar(share.fraction)),
                Style::new().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{:>3.0}%{points:<9}", share.fraction * 100.0),
                Style::new().bold(),
            ),
            Span::styled(
                format!("{:>8}  ", crate::pricing::format_usd(share.usage.cost)),
                Style::new().fg(Color::Green),
            ),
            Span::raw(share.title.chars().take(40).collect::<String>()),
        ]));
        lines.push(Line::from(Span::styled(
            format!("{}{}", " ".repeat(bar_width + 3), share.project),
            dim(),
        )));
    }
    for (title, rows) in [("Projects", &ledger.projects), ("Models", &ledger.models)] {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            title,
            Style::new().fg(Color::LightMagenta).bold(),
        )));
        for (name, usage) in rows.iter().take(5) {
            let fraction = if ledger.total.cost > 0.0 {
                usage.cost / ledger.total.cost
            } else {
                0.0
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", bar(fraction)),
                    Style::new().fg(Color::Cyan),
                ),
                Span::styled(format!("{:>3.0}%  ", fraction * 100.0), Style::new().bold()),
                Span::styled(
                    format!("{:>8}  ", crate::pricing::format_usd(usage.cost)),
                    Style::new().fg(Color::Green),
                ),
                Span::raw(name.clone()),
            ]));
        }
    }
    let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
    let scroll = app.insights_scroll.min(max);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(card(name, Color::LightMagenta, false)),
        area,
    );
}

fn severity_color(s: Severity) -> Color {
    match s {
        Severity::High => Color::Red,
        Severity::Medium => Color::Yellow,
        Severity::Low => Color::Cyan,
    }
}

/// Credentials in transcripts and the prompt history, then what Claude did
/// that deserves a look, worst first, across every session.
fn draw_security(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from(Span::styled(
        "Credentials in transcripts",
        Style::new().fg(Color::LightMagenta).bold(),
    ))];
    let mut found = 0;
    for s in &app.sessions {
        let Some(a) = app.analysis(s) else {
            continue;
        };
        for secret in &a.secrets {
            found += 1;
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
    for secret in &app.history_secrets {
        found += 1;
        lines.push(Line::from(vec![
            Span::styled("  🔑 ", Style::new().fg(Color::Red)),
            Span::styled(
                format!("{} {}", secret.kind, secret.masked),
                Style::new().fg(Color::Red),
            ),
            Span::styled("  in your prompt history", dim()),
        ]));
    }
    if found == 0 {
        lines.push(Line::from(Span::styled("  none found", dim())));
    } else {
        lines.push(Line::from(Span::styled(
            "  Rotate these keys; D in Sessions moves a session (and its transcript) to the trash.",
            dim().italic(),
        )));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "What Claude did that deserves a look",
        Style::new().fg(Color::LightMagenta).bold(),
    )));
    let mut events: Vec<(&crate::audit::Event, &str)> = app
        .sessions
        .iter()
        .filter_map(|s| app.analysis(s).map(|a| (s, a)))
        .flat_map(|(s, a)| a.audit.iter().map(move |e| (e, s.title.as_str())))
        .collect();
    events.sort_by_key(|(e, _)| (e.severity, std::cmp::Reverse(e.at)));
    if events.is_empty() {
        lines.push(Line::from(Span::styled("  nothing risky", dim())));
    }
    let detail_width = area.width.saturating_sub(60).max(20) as usize;
    for (e, title) in &events {
        let when =
            e.at.map(|t| t.format("%b %d").to_string())
                .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<7}", e.severity.label()),
                Style::new().fg(severity_color(e.severity)),
            ),
            Span::styled(format!("{:<34}", e.what), Style::new().bold()),
            Span::styled(format!("{:<7}", when), dim()),
            Span::styled(
                e.detail.chars().take(detail_width).collect::<String>(),
                Style::new().fg(Color::Gray),
            ),
            Span::styled(format!("  · {title}"), dim()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Permission rules are per project: Projects › a project › Skills, plugins & rules.",
        dim().italic(),
    )));
    let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
    let scroll = app.insights_scroll.min(max);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(card(
                &format!("Security · {} risky action(s)", events.len()),
                Color::Red,
                false,
            )),
        area,
    );
}
