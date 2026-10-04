//! One-shot commands that print and exit: `claudash status` for status bars
//! (tmux, Waybar, polybar) and `claudash summary` for stand-ups.

use std::io::{self, Write};

use crate::{
    claude_cli,
    history::History,
    hooks,
    hooks::Activity,
    paths, sessions,
    statusline::{self, Window},
};

#[derive(Debug, Default, PartialEq)]
pub struct Status {
    pub needs_you: usize,
    pub working: usize,
    pub waiting: usize,
    /// Plan usage percentages, when the status line has reported them.
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    /// When the 5-hour limit is reached at the current pace (local "HH:MM").
    pub limit_at: Option<String>,
}

impl Status {
    /// `▲ 1 needs you · 2 working · 1 waiting · 5h 64% · 7d 31%`, leaving out
    /// what's zero or unknown.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if self.needs_you > 0 {
            parts.push(format!("▲ {} needs you", self.needs_you));
        }
        if self.working > 0 {
            parts.push(format!("{} working", self.working));
        }
        if self.waiting > 0 {
            parts.push(format!("{} waiting", self.waiting));
        }
        if parts.is_empty() {
            parts.push("no open sessions".into());
        }
        for (label, pct) in [("5h", self.five_hour), ("7d", self.seven_day)] {
            if let Some(pct) = pct {
                match (&self.limit_at, label) {
                    (Some(at), "5h") => parts.push(format!("{label} {pct:.0}% → limit {at}")),
                    _ => parts.push(format!("{label} {pct:.0}%")),
                }
            }
        }
        parts.join(" · ")
    }

    pub fn json(&self) -> String {
        serde_json::json!({
            "needs_you": self.needs_you,
            "working": self.working,
            "waiting": self.waiting,
            "five_hour": self.five_hour,
            "seven_day": self.seven_day,
            "five_hour_limit_at": self.limit_at,
        })
        .to_string()
    }
}

/// Open sessions by what they're doing, and plan usage.
fn current() -> Result<Status, String> {
    let live = claude_cli::live_sessions(false)?;
    let (states, _) = hooks::load();
    let mut status = Status::default();
    for session in live.values().filter(|s| s.is_running()) {
        match hooks::combine(session, states.get(&session.session_id)) {
            Activity::NeedsYou => status.needs_you += 1,
            Activity::Working => status.working += 1,
            Activity::Waiting | Activity::Ended => status.waiting += 1,
        }
    }
    let store = statusline::load();
    if let Some((limits, _)) = &store.rate_limits {
        let pct = |w: Option<Window>| w.filter(Window::is_current).map(|w| w.used_percentage);
        status.five_hour = pct(limits.five_hour);
        status.seven_day = pct(limits.seven_day);
    }
    let now = chrono::Utc::now().timestamp();
    if let Some(statusline::Forecast::LimitAt(at)) =
        statusline::forecast(&store.samples, |s| s.five_hour, 3_600, now)
    {
        status.limit_at = chrono::DateTime::from_timestamp(at, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string());
    }
    Ok(status)
}

/// `claudash status [--json]`.
pub fn status(json: bool) -> io::Result<()> {
    let status = current().map_err(io::Error::other)?;
    let text = if json { status.json() } else { status.line() };
    println!("{text}");
    Ok(())
}

/// `claudash summary [-o FILE]`: today's summary as Markdown.
pub fn summary(output: Option<&str>) -> io::Result<()> {
    let dir = paths::projects_dir()
        .ok_or_else(|| io::Error::other("could not find Claude Code's config directory"))?;
    let all = sessions::load_sessions(&dir, &[])?;
    let today = chrono::Local::now().date_naive();
    let markdown = crate::summary::build(crate::summary::inputs(&all, today), today);
    match output {
        Some(file) => {
            std::fs::write(file, markdown)?;
            eprintln!("Wrote {file}");
            Ok(())
        }
        None => io::stdout().write_all(markdown.as_bytes()),
    }
}

// ---- claudash usage --------------------------------------------------------------

/// How `claudash usage` groups its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Daily,
    Monthly,
    Projects,
    Models,
    Sessions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageArgs {
    pub group: Group,
    /// First day included; defaults to 30 days ago (everything for monthly).
    pub since: Option<chrono::NaiveDate>,
    pub json: bool,
}

impl UsageArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = UsageArgs {
            group: Group::Daily,
            since: None,
            json: false,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "daily" => out.group = Group::Daily,
                "monthly" => out.group = Group::Monthly,
                "projects" => out.group = Group::Projects,
                "models" => out.group = Group::Models,
                "sessions" => out.group = Group::Sessions,
                "--json" => out.json = true,
                "--since" => {
                    let value = args.next().ok_or("missing value for --since")?;
                    out.since = Some(parse_day(value)?);
                }
                other => match other.strip_prefix("--since=") {
                    Some(value) => out.since = Some(parse_day(value)?),
                    None => {
                        return Err(format!(
                            "usage: claudash usage [daily|monthly|projects|models|sessions] \
                             [--since YYYY-MM-DD] [--json] (unexpected '{other}')"
                        ));
                    }
                },
            }
        }
        Ok(out)
    }
}

fn parse_day(text: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| format!("invalid date '{text}' (use YYYY-MM-DD)"))
}

/// One row of the report.
struct UsageRow {
    label: String,
    usage: sessions::Usage,
}

fn usage_rows(
    args: &UsageArgs,
    sessions: &[sessions::Session],
    history: &History,
) -> Vec<UsageRow> {
    let today = chrono::Local::now().date_naive();
    let since = args.since.unwrap_or(match args.group {
        Group::Monthly => chrono::NaiveDate::MIN,
        _ => today - chrono::Days::new(29),
    });
    let row = |label: String, usage| UsageRow { label, usage };
    match args.group {
        Group::Daily => history
            .per_day()
            .range(since..)
            .map(|(day, u)| row(day.to_string(), *u))
            .collect(),
        Group::Monthly => {
            let mut months: std::collections::BTreeMap<String, sessions::Usage> =
                std::collections::BTreeMap::new();
            for (day, u) in history.per_day().range(since..) {
                months
                    .entry(day.format("%Y-%m").to_string())
                    .or_default()
                    .add(u);
            }
            months.into_iter().map(|(m, u)| row(m, u)).collect()
        }
        Group::Projects => history
            .per_project(since)
            .into_iter()
            .map(|(p, u)| row(p, u))
            .collect(),
        Group::Models => history
            .per_model(since)
            .into_iter()
            .map(|(m, u)| row(m, u))
            .collect(),
        Group::Sessions => {
            let mut rows: Vec<UsageRow> = sessions
                .iter()
                .filter_map(|s| {
                    let mut u = sessions::Usage::default();
                    for (_, models) in s.tokens.daily.range(since..) {
                        for usage in models.values() {
                            u.add(usage);
                        }
                    }
                    (u.processed() > 0).then(|| {
                        row(
                            format!(
                                "{} · {} · {}",
                                &s.id[..s.id.len().min(8)],
                                s.project_path,
                                s.title
                            ),
                            u,
                        )
                    })
                })
                .collect();
            rows.sort_by(|a, b| b.usage.cost.total_cmp(&a.usage.cost));
            rows
        }
    }
}

/// `claudash usage`: tokens and API-equivalent dollars by day, month,
/// project, model or session.
pub fn usage(args: &UsageArgs) -> io::Result<()> {
    let dir = paths::projects_dir()
        .ok_or_else(|| io::Error::other("could not find Claude Code's config directory"))?;
    let all = sessions::load_sessions(&dir, &[])?;
    let mut history = History::load();
    // Best effort: without a data directory the report still covers current transcripts.
    let _ = history.merge(&all);
    let rows = usage_rows(args, &all, &history);
    let mut total = sessions::Usage::default();
    for r in &rows {
        total.add(&r.usage);
    }
    let mut out = io::stdout().lock();
    if args.json {
        let json_row = |label: &str, u: &sessions::Usage| {
            serde_json::json!({
                "label": label,
                "input_tokens": u.input_tokens,
                "cache_creation_tokens": u.cache_creation_input_tokens,
                "cache_read_tokens": u.cache_read_input_tokens,
                "output_tokens": u.output_tokens,
                "total_tokens": u.context() + u.output_tokens,
                "cost_usd": (u.cost * 100.0).round() / 100.0,
            })
        };
        let value = serde_json::json!({
            "group": format!("{:?}", args.group).to_lowercase(),
            "cost": "API-equivalent USD at Claude API prices",
            "rows": rows.iter().map(|r| json_row(&r.label, &r.usage)).collect::<Vec<_>>(),
            "total": json_row("total", &total),
        });
        return writeln!(out, "{value}");
    }
    let head = match args.group {
        Group::Daily => "Day",
        Group::Monthly => "Month",
        Group::Projects => "Project",
        Group::Models => "Model",
        Group::Sessions => "Session",
    };
    let width = rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(head.len(), 60);
    writeln!(
        out,
        "{head:<width$}  {:>9}  {:>9}  {:>9}  {:>9}  {:>10}",
        "input", "c.write", "c.read", "output", "≈ USD"
    )?;
    let line = |label: &str, u: &sessions::Usage| {
        let label: String = label.chars().take(width).collect();
        format!(
            "{label:<width$}  {:>9}  {:>9}  {:>9}  {:>9}  {:>10}",
            sessions::human_tokens(u.input_tokens),
            sessions::human_tokens(u.cache_creation_input_tokens),
            sessions::human_tokens(u.cache_read_input_tokens),
            sessions::human_tokens(u.output_tokens),
            crate::pricing::format_usd(u.cost)
        )
    };
    for r in &rows {
        writeln!(out, "{}", line(&r.label, &r.usage))?;
    }
    writeln!(out, "{}", "─".repeat(width + 59))?;
    writeln!(out, "{}", line("Total", &total))?;
    writeln!(
        out,
        "\nDollars are API-equivalent (what this usage would cost at Claude API prices), not what a Pro or Max plan charges."
    )
}

/// `claudash wrapped`: the last week or month on one card.
pub fn wrapped(month: bool, redact: bool, plain: bool) -> io::Result<()> {
    let dir = paths::projects_dir()
        .ok_or_else(|| io::Error::other("could not find Claude Code's config directory"))?;
    let all = sessions::load_sessions(&dir, &[])?;
    let period = if month {
        crate::wrapped::Period::Month
    } else {
        crate::wrapped::Period::Week
    };
    let (from, to) = period.range(chrono::Local::now().date_naive());
    // Only sessions active in the period need their transcripts analyzed.
    let start = from
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).single())
        .map_or(0, |t| t.timestamp());
    let analyses: std::collections::HashMap<String, crate::analysis::Analysis> = all
        .iter()
        .filter(|s| s.tokens.timeline.last().is_some_and(|(at, _)| *at >= start))
        .map(|s| (s.id.clone(), crate::analysis::analyze(&s.path)))
        .collect();
    let stats = crate::wrapped::stats(&all, |s| analyses.get(&s.id), from, to);
    let text = if plain {
        crate::wrapped::plain(&stats, redact)
    } else {
        crate::wrapped::ansi(&stats, redact)
    };
    io::stdout().lock().write_all(text.as_bytes())
}

/// `claudash quota`: each session's, project's and model's part of the current
/// 5-hour and 7-day plan windows.
pub fn quota(json: bool) -> io::Result<()> {
    let dir = paths::projects_dir()
        .ok_or_else(|| io::Error::other("could not find Claude Code's config directory"))?;
    let all = sessions::load_sessions(&dir, &[])?;
    let store = statusline::load();
    let now = chrono::Utc::now().timestamp();
    let windows = [
        ("5-hour", crate::quota::five_hour(&all, &store, now)),
        ("7-day", Some(crate::quota::seven_day(&store, now))),
    ];
    let mut out = io::stdout().lock();
    if json {
        let value: Vec<serde_json::Value> = windows
            .iter()
            .filter_map(|(name, w)| w.map(|w| (name, w)))
            .map(|(name, w)| {
                let ledger = crate::quota::ledger(&all, &w);
                serde_json::json!({
                    "window": name,
                    "start": w.start,
                    "reported": w.reported,
                    "used_percentage": w.used,
                    "cost_usd": (ledger.total.cost * 100.0).round() / 100.0,
                    "sessions": ledger.sessions.iter().map(|s| serde_json::json!({
                        "session_id": s.session_id,
                        "title": s.title,
                        "project": s.project,
                        "share": (s.fraction * 1000.0).round() / 1000.0,
                        "limit_points": s.limit_points(&w),
                        "cost_usd": (s.usage.cost * 100.0).round() / 100.0,
                    })).collect::<Vec<_>>(),
                    "projects": ledger.projects.iter().map(|(p, u)| serde_json::json!({
                        "project": p, "cost_usd": (u.cost * 100.0).round() / 100.0,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        return writeln!(out, "{}", serde_json::Value::Array(value));
    }
    for (name, window) in windows {
        let Some(window) = window else {
            writeln!(out, "{name} window: no requests in the last five hours\n")?;
            continue;
        };
        let ledger = crate::quota::ledger(&all, &window);
        let start = chrono::DateTime::from_timestamp(window.start, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%a %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let used = match window.used {
            Some(u) => format!("{u:.0}% of the limit used"),
            None if window.reported => "use not reported".into(),
            None => "estimated window (run `claudash setup` for the exact one)".into(),
        };
        writeln!(
            out,
            "{name} window since {start} · {used} · ≈{} API-equivalent",
            crate::pricing::format_usd(ledger.total.cost)
        )?;
        for s in ledger.sessions.iter().take(10) {
            let points = s
                .limit_points(&window)
                .map(|p| format!(" ≈{p:>3.0} pts"))
                .unwrap_or_default();
            writeln!(
                out,
                "  {:>4.0}%{points}  {:>9}  {}  ({})",
                s.fraction * 100.0,
                crate::pricing::format_usd(s.usage.cost),
                s.title,
                s.project
            )?;
        }
        writeln!(out)?;
    }
    writeln!(
        out,
        "Shares are by API-equivalent cost; how requests count against plan limits isn't published."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_arguments() {
        let parse = |list: &[&str]| {
            UsageArgs::parse(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            parse(&[]),
            Ok(UsageArgs {
                group: Group::Daily,
                since: None,
                json: false
            })
        );
        let args = parse(&["models", "--since", "2026-09-01", "--json"]).unwrap();
        assert_eq!(args.group, Group::Models);
        assert_eq!(args.since, chrono::NaiveDate::from_ymd_opt(2026, 9, 1));
        assert!(args.json);
        assert!(parse(&["--since", "yesterday"]).is_err());
        assert!(parse(&["weekly"]).is_err());
    }

    #[test]
    fn status_line_leaves_out_what_is_zero() {
        let status = Status {
            needs_you: 1,
            working: 2,
            five_hour: Some(64.4),
            ..Default::default()
        };
        assert_eq!(status.line(), "▲ 1 needs you · 2 working · 5h 64%");
        let pacing = Status {
            five_hour: Some(80.0),
            seven_day: Some(31.0),
            limit_at: Some("13:10".into()),
            ..Default::default()
        };
        assert_eq!(
            pacing.line(),
            "no open sessions · 5h 80% → limit 13:10 · 7d 31%"
        );
        assert_eq!(Status::default().line(), "no open sessions");
        assert!(status.json().contains("\"needs_you\":1"));
    }
}
