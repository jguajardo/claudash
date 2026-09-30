//! Health checks for claudash's connection to Claude Code, shown in the Help
//! view and printed by `claudash doctor`.

use crate::{claude_cli, hooks, paths, setup, statusline};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Works, but something could be better.
    Warn,
    /// Not set up or not found.
    Missing,
    /// Just information.
    Info,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    /// What to run or do to fix it.
    pub fix: Option<String>,
}

/// Minimum Claude Code version that reads AGENTS.md on its own.
const AGENTS_MD_VERSION: &str = "2.1.277";

/// Runs every check. Calls `claude --version` and `claude agents --json`, so
/// it takes a moment: run it off the UI thread.
pub fn run() -> Vec<Check> {
    let mut checks = Vec::new();
    let check = |name, level, detail: String, fix: Option<&str>| Check {
        name,
        level,
        detail,
        fix: fix.map(str::to_owned),
    };

    // Claude Code itself.
    let version = claude_cli::version();
    match (claude_cli::path(), &version) {
        (Some(path), Some(v)) => checks.push(check(
            "Claude Code",
            Level::Ok,
            format!("{v} at {}", paths::display(path)),
            None,
        )),
        (Some(path), None) => checks.push(check(
            "Claude Code",
            Level::Warn,
            format!(
                "found at {} but `claude --version` failed",
                paths::display(path)
            ),
            None,
        )),
        (None, _) => checks.push(check(
            "Claude Code",
            Level::Missing,
            "`claude` isn't on PATH, so MCP checks, resuming and plugins won't work".into(),
            Some("install it: https://code.claude.com/docs"),
        )),
    }

    let sessions = paths::projects_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|dirs| {
            dirs.flatten()
                .filter_map(|d| std::fs::read_dir(d.path()).ok())
                .flat_map(|e| e.flatten())
                .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                .count()
        });
    checks.push(match sessions {
        Some(n) => check(
            "Session data",
            Level::Ok,
            format!(
                "{n} sessions in {}",
                paths::projects_dir()
                    .map(|p| paths::display(&p))
                    .unwrap_or_default()
            ),
            None,
        ),
        None => check(
            "Session data",
            Level::Missing,
            "no sessions yet; use Claude Code once".into(),
            None,
        ),
    });

    // What `claudash setup` connects.
    let (statusline_installed, hook_events) = setup::installed();
    let statusline_data = statusline::load().configured;
    checks.push(match (statusline_installed, statusline_data) {
        (true, true) => check(
            "Status line",
            Level::Ok,
            "connected; plan usage, context window and cache data arriving".into(),
            None,
        ),
        (true, false) => check(
            "Status line",
            Level::Warn,
            "connected, no data yet: it arrives with the next reply in a Claude Code session started after setup".into(),
            None,
        ),
        (false, _) => check(
            "Status line",
            Level::Missing,
            "not connected: no plan usage, real context window or cache diagnostics".into(),
            Some("claudash setup --apply"),
        ),
    });
    let (_, hooks_seen) = hooks::load();
    let total = hooks::EVENTS.len();
    checks.push(match hook_events {
        n if n == total => check(
            "Hooks",
            if hooks_seen { Level::Ok } else { Level::Warn },
            if hooks_seen {
                "connected; sessions report needs-you / working / waiting".into()
            } else {
                "connected, no events yet: restart open Claude Code sessions".into()
            },
            None,
        ),
        0 => check(
            "Hooks",
            Level::Missing,
            "not connected: \"needs you\" alerts rely on Claude Code's own status only".into(),
            Some("claudash setup --apply"),
        ),
        n => check(
            "Hooks",
            Level::Warn,
            format!("{n} of {total} events connected"),
            Some("claudash setup --apply"),
        ),
    });

    checks.push(match claude_cli::live_sessions(false) {
        Ok(live) => check(
            "Open sessions",
            Level::Ok,
            format!("`claude agents --json` reports {} open", live.len()),
            None,
        ),
        Err(_) => check(
            "Open sessions",
            Level::Warn,
            "`claude agents --json` failed: live markers are off".into(),
            Some("update Claude Code"),
        ),
    });

    if let Some(v) = &version {
        checks.push(if claude_cli::at_least(v, AGENTS_MD_VERSION) {
            check("AGENTS.md", Level::Ok, "read by Claude Code on its own".into(), None)
        } else {
            check(
                "AGENTS.md",
                Level::Info,
                format!("needs Claude Code {AGENTS_MD_VERSION} or later; import it from CLAUDE.md meanwhile"),
                Some("claude update"),
            )
        });
    }

    let notifier = if cfg!(target_os = "macos") {
        Some("osascript")
    } else if cfg!(unix) {
        Some("notify-send")
    } else {
        None
    };
    checks.push(match notifier {
        Some(tool) if claude_cli::which(tool).is_some() => check(
            "Notifications",
            Level::Ok,
            format!("desktop notifications through {tool}, plus the terminal bell"),
            None,
        ),
        Some(tool) => check(
            "Notifications",
            Level::Warn,
            format!("`{tool}` not found: only the terminal bell rings"),
            Some("install libnotify (notify-send)"),
        ),
        None => check(
            "Notifications",
            Level::Info,
            "terminal bell only on this platform".into(),
            None,
        ),
    });

    checks.push(check(
        "claudash files",
        Level::Info,
        format!(
            "data {} · cache {}",
            crate::library::data_dir()
                .map(|p| paths::display(&p))
                .unwrap_or_else(|| "—".into()),
            paths::claudash_cache()
                .map(|p| paths::display(&p))
                .unwrap_or_else(|| "—".into()),
        ),
        None,
    ));
    checks
}

pub fn symbol(level: Level) -> &'static str {
    match level {
        Level::Ok => "✓",
        Level::Warn => "!",
        Level::Missing => "✗",
        Level::Info => "·",
    }
}

/// `claudash doctor`.
pub fn print() {
    println!("claudash doctor\n");
    for c in run() {
        println!("  {} {:<16} {}", symbol(c.level), c.name, c.detail);
        if let Some(fix) = c.fix {
            println!("  {:<18} → {fix}", "");
        }
    }
}
