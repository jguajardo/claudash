//! "Wrapped": a week or a month of Claude Code on one card, made to be shared.
//! Everything comes from local transcripts; `redact` hides project and
//! session names before you post it.

use std::collections::HashMap;

use chrono::{Datelike, Local, NaiveDate, Timelike};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    analysis::Analysis,
    pricing::format_usd,
    sessions::{Session, human_tokens},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Week,
    Month,
}

impl Period {
    /// First and last day covered, ending today.
    pub fn range(self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        let days = match self {
            Period::Week => 6,
            Period::Month => 29,
        };
        (today - chrono::Days::new(days), today)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub sessions: usize,
    pub prompts: usize,
    pub requests: usize,
    /// Input, cache writes and output: what was actually processed.
    pub tokens: u64,
    pub cost: f64,
    pub active_days: usize,
    /// Consecutive active days ending on the last day.
    pub streak: usize,
    pub busiest_day: Option<(NaiveDate, f64)>,
    /// Session with the most requests: title, requests, cost.
    pub top_session: Option<(String, usize, f64)>,
    pub projects: Vec<(String, f64)>,
    pub models: Vec<(String, f64)>,
    pub tools: Vec<(String, u32)>,
    pub commands: Vec<(String, u32)>,
    /// Requests per hour of the day, local time.
    pub hours: [u32; 24],
    pub secrets: usize,
    pub risky: usize,
}

/// Gathers a period's numbers. `analysis` gives a session's transcript
/// analysis, when there is one.
pub fn stats<'a>(
    sessions: &'a [Session],
    analysis: impl Fn(&Session) -> Option<&'a Analysis>,
    from: NaiveDate,
    to: NaiveDate,
) -> Stats {
    let mut s = Stats {
        from,
        to,
        ..Default::default()
    };
    let in_range = |day: NaiveDate| day >= from && day <= to;
    let mut days: HashMap<NaiveDate, f64> = HashMap::new();
    let mut projects: HashMap<String, f64> = HashMap::new();
    let mut models: HashMap<String, f64> = HashMap::new();
    let mut tools: HashMap<String, u32> = HashMap::new();
    let mut commands: HashMap<String, u32> = HashMap::new();
    for session in sessions {
        let mut requests = 0;
        let mut cost = 0.0;
        for (at, usage) in &session.tokens.timeline {
            let Some(time) = chrono::DateTime::from_timestamp(*at, 0) else {
                continue;
            };
            let time = time.with_timezone(&Local);
            if !in_range(time.date_naive()) {
                continue;
            }
            requests += 1;
            cost += usage.cost;
            s.tokens += usage.processed();
            s.hours[time.hour() as usize] += 1;
            *days.entry(time.date_naive()).or_default() += usage.cost;
        }
        if requests == 0 {
            continue;
        }
        s.sessions += 1;
        s.requests += requests;
        s.cost += cost;
        let project = project_name(&session.project_path);
        *projects.entry(project).or_default() += cost;
        for (day, per_model) in &session.tokens.daily {
            if in_range(*day) {
                for (model, usage) in per_model {
                    if !model.is_empty() {
                        *models.entry(short_model(model)).or_default() += usage.cost;
                    }
                }
            }
        }
        if s.top_session.as_ref().is_none_or(|(_, r, _)| requests > *r) {
            s.top_session = Some((session.title.clone(), requests, cost));
        }
        let Some(a) = analysis(session) else {
            continue;
        };
        s.prompts += a
            .prompts
            .iter()
            .filter(|p| p.at.is_some_and(|t| in_range(t.date_naive())))
            .count();
        for ((day, label), stat) in &a.tool_output {
            if !in_range(*day) {
                continue;
            }
            match label.strip_prefix("Bash: ") {
                Some(command) => {
                    *tools.entry("Bash".into()).or_default() += stat.calls;
                    if !matches!(command, "shell loop or condition" | "other") {
                        *commands.entry(command.to_string()).or_default() += stat.calls;
                    }
                }
                None => *tools.entry(label.clone()).or_default() += stat.calls,
            }
        }
        s.secrets += a.secrets.len();
        s.risky += a
            .audit
            .iter()
            .filter(|e| {
                e.severity == crate::audit::Severity::High
                    && e.at.is_some_and(|t| in_range(t.date_naive()))
            })
            .count();
    }
    s.active_days = days.len();
    let mut day = to;
    while days.contains_key(&day) {
        s.streak += 1;
        day = day.pred_opt().unwrap_or(day);
        if day < from {
            break;
        }
    }
    s.busiest_day = days.into_iter().max_by(|a, b| a.1.total_cmp(&b.1));
    s.projects = sorted(projects);
    s.models = sorted(models);
    s.tools = sorted_counts(tools);
    s.commands = sorted_counts(commands);
    s
}

/// 1292 → "1,292".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn sorted(map: HashMap<String, f64>) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = map.into_iter().filter(|(_, c)| *c > 0.0).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

fn sorted_counts(map: HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = map.into_iter().collect();
    v.sort_by_key(|(name, n)| (std::cmp::Reverse(*n), name.clone()));
    v
}

/// `~/code/api-server` → `api-server`.
pub fn project_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty() && *s != "~")
        .unwrap_or(path)
        .to_string()
}

/// `claude-haiku-4-5-20251001` → `haiku-4-5`.
pub fn short_model(model: &str) -> String {
    let name = model.strip_prefix("claude-").unwrap_or(model);
    let parts: Vec<&str> = name.split('-').collect();
    match parts.last() {
        Some(last) if last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()) => {
            parts[..parts.len() - 1].join("-")
        }
        _ => name.to_string(),
    }
}

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn sparkline(values: &[u32]) -> String {
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    values
        .iter()
        .map(|&v| {
            if v == 0 {
                ' '
            } else {
                SPARK[((v as usize * 7) / max as usize).min(7)]
            }
        })
        .collect()
}

fn bar(fraction: f64, width: usize) -> String {
    let filled = ((fraction * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// The card's lines, without a border. `redact` replaces project and
/// session names with neutral ones.
pub fn card(s: &Stats, redact: bool) -> Vec<Line<'static>> {
    let label = |text: &str| Span::styled(format!("{text:<14}"), Style::new().fg(Color::DarkGray));
    let accent = Style::new().fg(Color::LightMagenta).bold();
    let green = Style::new().fg(Color::Green).bold();
    let bold = Style::new().bold();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(thousands(s.prompts), accent),
            Span::raw(" prompts · "),
            Span::styled(thousands(s.requests), accent),
            Span::raw(" requests · "),
            Span::styled(thousands(s.sessions), accent),
            Span::raw(" sessions · "),
            Span::styled(human_tokens(s.tokens), accent),
            Span::raw(" tokens"),
        ]),
        Line::from(vec![
            Span::styled(format!("≈{}", format_usd(s.cost)), green),
            Span::styled(" at API prices", Style::new().fg(Color::DarkGray)),
        ]),
        Line::default(),
    ];
    if s.requests == 0 {
        lines.push(Line::from(Span::styled(
            "No Claude Code activity in this period.",
            Style::new().fg(Color::DarkGray),
        )));
        return lines;
    }
    lines.push(Line::from(vec![
        label("Active days"),
        Span::styled(format!("{}", s.active_days), bold),
        Span::raw(if s.streak > 1 {
            format!(" · {} days in a row", s.streak)
        } else {
            String::new()
        }),
    ]));
    if let Some((day, cost)) = s.busiest_day {
        lines.push(Line::from(vec![
            label("Busiest day"),
            Span::styled(day.format("%a %b %-d").to_string(), bold),
            Span::raw(format!(" · ≈{}", format_usd(cost))),
        ]));
    }
    if let Some((title, requests, cost)) = &s.top_session {
        let title = if redact {
            "a session".to_string()
        } else {
            title.chars().take(44).collect()
        };
        lines.push(Line::from(vec![
            label("Top session"),
            Span::styled(title, bold),
            Span::raw(format!(
                " · {} requests · ≈{}",
                thousands(*requests),
                format_usd(*cost)
            )),
        ]));
    }
    lines.push(Line::default());
    let total = s.cost.max(f64::MIN_POSITIVE);
    let shares = |rows: &[(String, f64)], name_redacted: Option<&str>| -> Vec<Line<'static>> {
        rows.iter()
            .take(4)
            .enumerate()
            .map(|(i, (name, cost))| {
                let name = match name_redacted {
                    Some(prefix) => format!("{prefix} {}", i + 1),
                    None => name.clone(),
                };
                Line::from(vec![
                    Span::raw(" ".repeat(14)),
                    Span::styled(
                        format!("{} ", bar(cost / total, 10)),
                        Style::new().fg(Color::Cyan),
                    ),
                    Span::styled(format!("{:>3.0}%  ", cost / total * 100.0), bold),
                    Span::raw(name),
                ])
            })
            .collect()
    };
    let mut project_lines = shares(&s.projects, redact.then_some("project"));
    if let Some(first) = project_lines.first_mut() {
        first.spans[0] = label("Projects");
    }
    lines.extend(project_lines);
    let mut model_lines = shares(&s.models, None);
    if let Some(first) = model_lines.first_mut() {
        first.spans[0] = label("Models");
    }
    lines.extend(model_lines);
    lines.push(Line::default());
    if !s.tools.is_empty() {
        let tools: Vec<String> = s
            .tools
            .iter()
            .take(4)
            .map(|(t, n)| format!("{} {}", tool_name(t), thousands(*n as usize)))
            .collect();
        lines.push(Line::from(vec![
            label("Tools"),
            Span::raw(tools.join(" · ")),
        ]));
    }
    if !s.commands.is_empty() {
        let commands: Vec<String> = s
            .commands
            .iter()
            .take(4)
            .map(|(c, n)| format!("{c} {}", thousands(*n as usize)))
            .collect();
        lines.push(Line::from(vec![
            label("Commands"),
            Span::raw(commands.join(" · ")),
        ]));
    }
    let peak = s
        .hours
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(h, _)| h)
        .unwrap_or(0);
    lines.push(Line::from(vec![
        label("Hours"),
        Span::styled(
            format!("│{}│", sparkline(&s.hours)),
            Style::new().fg(Color::LightMagenta),
        ),
        Span::styled(
            format!("  peak {peak:02}:00"),
            Style::new().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(15)),
        Span::styled(
            "0     6     12    18   23",
            Style::new().fg(Color::DarkGray),
        ),
    ]));
    if s.secrets + s.risky > 0 {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            label("Security"),
            Span::styled(
                format!(
                    "{} secret(s) in transcripts · {} risky command(s)",
                    s.secrets, s.risky
                ),
                Style::new().fg(Color::Red),
            ),
        ]));
    }
    lines
}

fn tool_name(label: &str) -> String {
    match label.strip_prefix("mcp__") {
        Some(rest) => rest.split("__").next().unwrap_or(rest).to_string(),
        None => label.to_string(),
    }
}

/// The card's title: "claudash wrapped · Sep 28 – Oct 4".
pub fn title(s: &Stats) -> String {
    let same_year = s.from.year() == s.to.year();
    format!(
        "claudash wrapped · {} – {}",
        s.from
            .format(if same_year { "%b %-d" } else { "%b %-d %Y" }),
        s.to.format("%b %-d %Y")
    )
}

/// The card as plain text with a border, for terminals and Markdown.
pub fn plain(s: &Stats, redact: bool) -> String {
    boxed_plain(&title(s), &card(s, redact))
}

/// The card with ANSI colors, for a terminal you'll screenshot.
pub fn ansi(s: &Stats, redact: bool) -> String {
    boxed_ansi(&title(s), &card(s, redact))
}

/// `lines` in a box titled `title`, signed "made with claudash".
pub fn boxed_plain(title: &str, lines: &[Line]) -> String {
    let lines: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
        .collect();
    let width = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 4);
    let mut out = format!(
        "╭─ {title} {}╮\n",
        "─".repeat(width - title.chars().count() - 1)
    );
    for line in &lines {
        out.push_str(&format!(
            "│ {line}{} │\n",
            " ".repeat(width - line.chars().count())
        ));
    }
    let footer = " made with claudash ";
    out.push_str(&format!(
        "╰{}{footer}─╯\n",
        "─".repeat(width + 1 - footer.chars().count())
    ));
    out
}

/// `lines` in a box with ANSI colors.
pub fn boxed_ansi(title: &str, lines: &[Line]) -> String {
    let width = lines
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 4);
    let border = |text: &str| format!("\x1b[38;5;213m{text}\x1b[0m");
    let mut out = border(&format!(
        "╭─ {title} {}╮",
        "─".repeat(width - title.chars().count() - 1)
    ));
    out.push('\n');
    for line in lines {
        out.push_str(&border("│ "));
        for span in &line.spans {
            out.push_str(&styled(&span.content, span.style));
        }
        out.push_str(&" ".repeat(width - line.width()));
        out.push_str(&border(" │"));
        out.push('\n');
    }
    let footer = " made with claudash ";
    out.push_str(&border(&format!(
        "╰{}{footer}─╯",
        "─".repeat(width + 1 - footer.chars().count())
    )));
    out.push('\n');
    out
}

fn styled(text: &str, style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    if style.add_modifier.contains(ratatui::style::Modifier::BOLD) {
        codes.push("1".into());
    }
    if let Some(fg) = style.fg {
        let code = match fg {
            Color::DarkGray => "90",
            Color::Red => "91",
            Color::Green => "92",
            Color::Yellow => "93",
            Color::LightMagenta => "95",
            Color::Cyan => "96",
            Color::White => "97",
            _ => "",
        };
        if !code.is_empty() {
            codes.push(code.into());
        }
    }
    if codes.is_empty() {
        text.to_string()
    } else {
        format!("\x1b[{}m{text}\x1b[0m", codes.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{SessionTokens, Usage};
    use std::{path::PathBuf, time::SystemTime};

    fn at(day: u32, hour: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, 10, day, hour, 0, 0)
            .unwrap()
            .timestamp()
    }

    use chrono::TimeZone;

    fn session(project: &str, title: &str, requests: Vec<(i64, f64)>) -> Session {
        Session {
            id: title.into(),
            path: PathBuf::new(),
            title: title.into(),
            project_path: project.into(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::now(),
            size: 0,
            tokens: SessionTokens {
                timeline: requests
                    .into_iter()
                    .map(|(t, cost)| {
                        (
                            t,
                            Usage {
                                output_tokens: 1000,
                                cost,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn sums_a_week_and_finds_the_streak() {
        let sessions = [
            session(
                "~/code/api",
                "Fix login",
                vec![(at(2, 10), 1.0), (at(3, 11), 2.0), (at(4, 11), 4.0)],
            ),
            session("~/code/web", "Pricing page", vec![(at(4, 15), 1.0)]),
            session("~/old", "Old", vec![(at(1, 9), 9.0)]),
        ];
        let from = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let s = stats(&sessions, |_| None, from, to);
        assert_eq!(
            (s.sessions, s.requests, s.active_days, s.streak),
            (2, 4, 3, 3)
        );
        assert!((s.cost - 8.0).abs() < 1e-9);
        assert_eq!(s.busiest_day.unwrap().0, to);
        assert_eq!(s.top_session.as_ref().unwrap().0, "Fix login");
        assert_eq!(s.projects[0].0, "api");
        assert_eq!(s.hours[11], 2);

        let text = plain(&s, true);
        assert!(text.contains("project 1") && !text.contains("api") && !text.contains("Fix login"));
        assert!(text.contains("≈$8.00"));
        assert!(plain(&s, false).contains("Fix login"));
        // Every line of the box is as wide as the others.
        let widths: Vec<usize> = text.lines().map(|l| l.chars().count()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    }

    #[test]
    fn names_models_and_projects_briefly() {
        assert_eq!(short_model("claude-haiku-4-5-20251001"), "haiku-4-5");
        assert_eq!(short_model("claude-opus-5-5"), "opus-5-5");
        assert_eq!(
            project_name("~/Documentos/proyectos/claude-dash"),
            "claude-dash"
        );
        assert_eq!(sparkline(&[0, 1, 2, 4]), " ▂▄█");
    }
}
