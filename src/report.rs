//! One-shot commands that print and exit: `claudash status` for status bars
//! (tmux, Waybar, polybar) and `claudash summary` for stand-ups.

use std::io::{self, Write};

use crate::{
    claude_cli, hooks,
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
                parts.push(format!("{label} {pct:.0}%"));
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
    if let Some((limits, _)) = statusline::load().rate_limits {
        let pct = |w: Option<Window>| w.filter(Window::is_current).map(|w| w.used_percentage);
        status.five_hour = pct(limits.five_hour);
        status.seven_day = pct(limits.seven_day);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_leaves_out_what_is_zero() {
        let status = Status {
            needs_you: 1,
            working: 2,
            five_hour: Some(64.4),
            ..Default::default()
        };
        assert_eq!(status.line(), "▲ 1 needs you · 2 working · 5h 64%");
        assert_eq!(Status::default().line(), "no open sessions");
        assert!(status.json().contains("\"needs_you\":1"));
    }
}
