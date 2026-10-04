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
        InsightsSection::Plan => draw_plan(frame, app, content),
        InsightsSection::Tokens => draw_token_report(frame, app, content),
        InsightsSection::Security => draw_security(frame, app, content),
    }
}

/// Is your plan worth it: your use at API prices against the plan's price,
/// and how often each plan would have stopped you.
fn draw_plan(frame: &mut Frame, app: &App, area: Rect) {
    let now = chrono::Utc::now().timestamp();
    let fit = crate::plan::fit(&app.sessions, &app.statusline, app.plan, now);
    let report = crate::plan::report(&fit);
    let yours = fit.plan.map(|(p, _)| p.name());
    let mut lines: Vec<Line> = Vec::new();
    let mut in_table = false;
    for (i, text) in report.into_iter().enumerate() {
        let line = if i < 2 {
            Line::from(Span::styled(text, Style::new().bold()))
        } else if text.starts_with("If you had") {
            in_table = true;
            Line::from(Span::styled(text, Style::new().fg(Color::Cyan).bold()))
        } else if text.is_empty() {
            in_table = false;
            Line::default()
        } else if in_table && yours.is_some_and(|y| text.starts_with(&format!("{y} (yours)"))) {
            Line::from(Span::styled(
                text,
                Style::new().fg(Color::LightMagenta).bold(),
            ))
        } else if in_table {
            Line::from(text)
        } else if text.starts_with("How it's estimated") {
            Line::from(Span::styled(text, dim()))
        } else {
            Line::from(Span::styled(text, Style::new().fg(Color::Yellow)))
        };
        lines.push(line);
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.insights_scroll, 0))
            .block(card(
                "Is your plan worth it · last 30 days",
                Color::LightMagenta,
                false,
            )),
        area,
    );
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
    // Names are cut to the room left after the bar, share and cost.
    // Borders and padding take 4 columns.
    let room = (area.width as usize)
        .saturating_sub(4 + 2 + bar_width + 1 + 13 + 10)
        .max(8);
    let cut = |text: &str| -> String {
        if text.chars().count() <= room {
            text.to_string()
        } else {
            let mut s: String = text.chars().take(room - 1).collect();
            s.push('…');
            s
        }
    };
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
            Span::raw(cut(&share.title)),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "{}{}",
                " ".repeat(bar_width + 3),
                cut(&crate::wrapped::project_name(&share.project))
            ),
            dim(),
        )));
    }
    for (title, rows) in [("Projects", &ledger.projects), ("Models", &ledger.models)] {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            title,
            Style::new().fg(Color::LightMagenta).bold(),
        )));
        let models = title == "Models";
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
                Span::styled(
                    format!("{:>3.0}%{:<9}", fraction * 100.0, ""),
                    Style::new().bold(),
                ),
                Span::styled(
                    format!("{:>8}  ", crate::pricing::format_usd(usage.cost)),
                    Style::new().fg(Color::Green),
                ),
                Span::raw(if models {
                    cut(&crate::wrapped::short_model(name))
                } else {
                    cut(&crate::wrapped::project_name(name))
                }),
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
