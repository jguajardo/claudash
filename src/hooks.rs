//! `claudash hook`: a Claude Code hook command that records what each session
//! is doing, so the dashboard can tell which sessions need you.
//!
//! Claude Code runs it for the events `claudash setup` registers and pipes the
//! documented hook JSON to stdin (<https://code.claude.com/docs/en/hooks>).
//! Only the event, its notification type and the time are kept, in
//! `<cache dir>/claudash/state/<session_id>.json`; prompts and replies are not.
//! It prints nothing: `UserPromptSubmit` output would be added to Claude's context.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

/// Events `claudash setup` registers.
pub const EVENTS: [&str; 5] = [
    "UserPromptSubmit",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    hook_event_name: Option<String>,
    notification_type: Option<String>,
    cwd: Option<String>,
}

/// What a session last reported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub event: String,
    pub notification_type: Option<String>,
    /// Epoch seconds.
    pub at: i64,
}

/// What the dashboard shows for a session, derived from its last hook event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Waiting for a permission decision or other input only you can give.
    NeedsYou,
    /// Claude is working on a prompt.
    Working,
    /// Claude finished and is waiting for your next prompt.
    Waiting,
    /// The session ended.
    Ended,
}

impl State {
    pub fn activity(&self) -> Activity {
        match (self.event.as_str(), self.notification_type.as_deref()) {
            ("Notification", Some("idle_prompt" | "agent_completed" | "auth_success")) => {
                Activity::Waiting
            }
            ("Notification", Some(t)) if t.starts_with("quota_auto_resume") => Activity::Waiting,
            // permission_prompt, elicitation_*, agent_needs_input and unknown types.
            ("Notification", _) => Activity::NeedsYou,
            ("Stop", _) => Activity::Waiting,
            ("SessionEnd", _) => Activity::Ended,
            _ => Activity::Working,
        }
    }
}

/// What an open session is doing: Claude Code's own "waiting on a permission"
/// wins, the session's last hook event refines the rest.
pub fn combine(live: &crate::claude_cli::LiveSession, state: Option<&State>) -> Activity {
    if live.needs_you() {
        return Activity::NeedsYou;
    }
    match state.map(State::activity) {
        Some(Activity::Ended) | None => match live.status.as_str() {
            "busy" => Activity::Working,
            _ => Activity::Waiting,
        },
        Some(activity) => activity,
    }
}

fn state_dir() -> Option<PathBuf> {
    crate::paths::claudash_cache().map(|dir| dir.join("state"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Entry point for `claudash hook`. Always exits successfully and silently so
/// it can never block or disturb Claude Code.
pub fn run() -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    if let Err(e) = record(&input) {
        eprintln!("claudash hook: {e}");
    }
    Ok(())
}

fn record(input: &[u8]) -> io::Result<()> {
    let hook: HookInput = serde_json::from_slice(input).map_err(io::Error::other)?;
    let (Some(id), Some(event), Some(dir)) = (hook.session_id, hook.hook_event_name, state_dir())
    else {
        return Ok(());
    };
    if !valid_id(&id) {
        return Ok(());
    }
    // The opt-in safety net: snapshot the project around each turn.
    let label = match event.as_str() {
        "UserPromptSubmit" => Some("before prompt"),
        "Stop" => Some("after reply"),
        _ => None,
    };
    if let (Some(label), Some(cwd)) = (label, hook.cwd.as_deref())
        && crate::snapshots::enabled()
        && let Some(root) = crate::snapshots::project_root(std::path::Path::new(cwd))
        && let Err(e) = crate::snapshots::take(&root, &id, label)
    {
        eprintln!("claudash hook: snapshot failed: {e}");
    }
    let state = State {
        event,
        notification_type: hook.notification_type,
        at: chrono::Utc::now().timestamp(),
    };
    fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{id}.{}.tmp", std::process::id()));
    fs::write(&tmp, serde_json::to_vec(&state).map_err(io::Error::other)?)?;
    fs::rename(tmp, dir.join(format!("{id}.json")))
}

/// Last reported state per session ID, and whether any hook has ever run.
pub fn load() -> (HashMap<String, State>, bool) {
    let mut states = HashMap::new();
    let Some(dir) = state_dir() else {
        return (states, false);
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return (states, false);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(id), Some("json")) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|e| e.to_str()),
        ) else {
            continue;
        };
        if let Some(state) = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<State>(&raw).ok())
        {
            states.insert(id.to_string(), state);
        }
    }
    (states, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(event: &str, kind: Option<&str>) -> Activity {
        State {
            event: event.into(),
            notification_type: kind.map(Into::into),
            at: 0,
        }
        .activity()
    }

    #[test]
    fn maps_events_to_activity() {
        assert_eq!(
            state("Notification", Some("permission_prompt")),
            Activity::NeedsYou
        );
        assert_eq!(
            state("Notification", Some("elicitation_dialog")),
            Activity::NeedsYou
        );
        assert_eq!(
            state("Notification", Some("idle_prompt")),
            Activity::Waiting
        );
        assert_eq!(state("Stop", None), Activity::Waiting);
        assert_eq!(state("UserPromptSubmit", None), Activity::Working);
        // A tool ran, so a pending permission prompt was answered.
        assert_eq!(state("PostToolUse", None), Activity::Working);
        assert_eq!(state("SessionEnd", None), Activity::Ended);
    }

    #[test]
    fn parses_hook_input_without_keeping_content() {
        let hook: HookInput = serde_json::from_str(
            r#"{"session_id":"abc","hook_event_name":"UserPromptSubmit","user_prompt":"secret"}"#,
        )
        .unwrap();
        assert_eq!(hook.hook_event_name.as_deref(), Some("UserPromptSubmit"));
        assert!(valid_id("0fb00c9a-2bf4"));
        assert!(!valid_id("../etc"));
    }
}
