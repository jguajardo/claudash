//! Sessions a plan limit stopped, and continuing them once it resets.
//!
//! When a plan limit stops a session, Claude Code writes a reply with
//! `"error": "rate_limit"` and the time the limit resets. Once that time has
//! passed, `claude --bg --resume <id> <prompt>` continues the same
//! conversation in the background, under the same ID.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    claude_cli::{self, LiveSession},
    sessions::{LimitStop, Session},
};

/// What a continued session is told.
pub const PROMPT: &str = "The usage limit has reset. Continue where you left off.";

/// Stops older than this, counted from their reset, are left alone: the work
/// has likely moved on.
const RECENT: i64 = 24 * 3600;

/// A session stopped at a limit that reset less than a day ago or hasn't yet.
pub struct Stopped<'a> {
    pub session: &'a Session,
    pub stop: &'a LimitStop,
    /// Open in Claude Code: it can only be continued from its own terminal.
    pub open: bool,
}

impl Stopped<'_> {
    pub fn reset(&self, now: i64) -> bool {
        now >= self.stop.resets_at
    }
}

/// Sessions stopped at a plan limit, soonest reset first.
pub fn stopped<'a>(
    sessions: &'a [Session],
    live: &HashMap<String, LiveSession>,
    now: i64,
) -> Vec<Stopped<'a>> {
    let mut list: Vec<Stopped> = sessions
        .iter()
        .filter_map(|session| {
            let stop = session.tokens.limit_stop.as_ref()?;
            (stop.resets_at + RECENT > now).then(|| Stopped {
                session,
                stop,
                open: live.contains_key(&session.id),
            })
        })
        .collect();
    list.sort_by_key(|s| (s.stop.resets_at, std::cmp::Reverse(s.stop.at)));
    list
}

/// What `continue_all` needs for each session that can be continued now:
/// reset, not open, and its folder still there.
pub fn ready(list: &[Stopped], now: i64) -> Vec<(String, PathBuf, String)> {
    list.iter()
        .filter(|s| s.reset(now) && !s.open)
        .filter_map(|s| {
            let cwd = s.session.cwd.clone().filter(|d| d.is_dir())?;
            Some((s.session.id.clone(), cwd, s.session.title.clone()))
        })
        .collect()
}

fn run(id: &str, cwd: &Path) -> Result<String, String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("unexpected session ID".into());
    }
    let output = claude_cli::command()
        .args(["--bg", "--resume", id, PROMPT])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Could not run `claude --bg`: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let mut err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if err.is_empty() {
            err = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
        // Background sessions only start in folders you've trusted.
        if err.contains("not trusted") {
            return Err(format!(
                "Claude Code doesn't trust {} yet: run `claude` there once and accept",
                crate::paths::display(cwd)
            ));
        }
        Err(if err.is_empty() {
            "`claude --bg --resume` failed".into()
        } else {
            err
        })
    }
}

/// Continues every session in `sessions` (ID, folder, title) one after
/// another, and says how it went.
pub fn continue_all(sessions: &[(String, PathBuf, String)]) -> Result<String, String> {
    let mut done = 0;
    let mut errors = Vec::new();
    for (id, cwd, title) in sessions {
        match run(id, cwd) {
            Ok(_) => done += 1,
            Err(e) => errors.push(format!("{title}: {e}")),
        }
    }
    let msg = format!(
        "Continued {done} session{} in the background",
        if done == 1 { "" } else { "s" }
    );
    if errors.is_empty() {
        Ok(msg)
    } else {
        Err(format!("{msg}; {}", errors.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionTokens;
    use std::time::SystemTime;

    fn session(id: &str, stop: Option<(i64, i64)>) -> Session {
        Session {
            id: id.into(),
            path: PathBuf::new(),
            title: id.into(),
            project_path: String::new(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::UNIX_EPOCH,
            size: 0,
            tokens: SessionTokens {
                limit_stop: stop.map(|(at, resets_at)| LimitStop {
                    at,
                    resets_at,
                    window: "five_hour".into(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn lists_recent_stops_and_marks_open_ones() {
        let now = 100_000;
        let sessions = [
            session("fine", None),
            session("waits", Some((now - 600, now + 3600))),
            session("reset", Some((now - 9000, now - 60))),
            session("old", Some((now - 200_000, now - 100_000))),
        ];
        let live = HashMap::from([("reset".to_string(), LiveSession::default())]);
        let list = stopped(&sessions, &live, now);
        let ids: Vec<&str> = list.iter().map(|s| s.session.id.as_str()).collect();
        assert_eq!(ids, ["reset", "waits"]);
        assert!(list[0].open && list[0].reset(now));
        assert!(!list[1].open && !list[1].reset(now));
    }
}
