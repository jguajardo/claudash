//! `claudash statusline`: a Claude Code status line command that records what
//! Claude Code reports, so the dashboard can show it.
//!
//! Claude Code runs the configured status line command after each assistant
//! message and pipes a documented JSON object to its stdin
//! (<https://code.claude.com/docs/en/statusline>). That JSON carries data that
//! isn't available anywhere else: plan usage (`rate_limits`) and the real
//! context window size. This command saves it to
//! `<cache dir>/claudash/statusline/<session_id>.json` and prints a short status
//! line, or the output of another status line command given after `--`.

use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use serde::Deserialize;

/// Files older than this are deleted when a new one is written.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

pub const SETUP_HELP: &str = r#"Add this to ~/.claude/settings.json to let claudash show plan usage and
the real context window size:

  "statusLine": {
    "type": "command",
    "command": "claudash statusline"
  }

To keep an existing status line, pass its command after `--`; claudash
records the data and prints that command's output instead of its own:

  "command": "claudash statusline -- ~/.claude/my-statusline.sh"

Claude Code only sends plan usage (rate_limits) to Pro and Max subscribers,
after the first response of a session."#;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Model,
    #[serde(default)]
    pub context_window: ContextWindow,
    #[serde(default)]
    pub cost: Cost,
    pub rate_limits: Option<RateLimits>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Model {
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ContextWindow {
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Window {
    pub used_percentage: f64,
    /// Unix epoch seconds.
    pub resets_at: i64,
}

impl Window {
    /// A window whose reset time has passed no longer applies.
    pub fn is_current(&self) -> bool {
        self.resets_at > chrono::Utc::now().timestamp()
    }
}

fn store_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("claudash").join("statusline"))
}

/// Entry point for `claudash statusline [-- <command> [args...]]`.
///
/// Never fails loudly: a broken status line would clutter Claude Code's UI, so
/// errors only reach stderr.
pub fn run(wrapped: &[String]) -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;

    let snapshot: Snapshot = serde_json::from_slice(&input).unwrap_or_default();
    if let Err(e) = save(&snapshot, &input) {
        eprintln!("claudash statusline: {e}");
    }

    let mut out = io::stdout().lock();
    match wrapped.split_first() {
        Some((program, args)) => {
            let output = run_wrapped(program, args, &input)?;
            out.write_all(&output)
        }
        None => writeln!(out, "{}", render(&snapshot)),
    }
}

fn save(snapshot: &Snapshot, raw: &[u8]) -> io::Result<()> {
    let (Some(dir), Some(id)) = (store_dir(), snapshot.session_id.as_deref()) else {
        return Ok(());
    };
    // Session IDs are UUIDs; refuse anything that could escape the directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    // Write then rename, so the dashboard never reads a half-written file.
    let tmp = dir.join(format!(".{id}.{}.tmp", std::process::id()));
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    prune(&dir);
    Ok(())
}

fn prune(dir: &PathBuf) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > MAX_AGE);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn run_wrapped(program: &str, args: &[String], input: &[u8]) -> io::Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // The command may exit without reading everything; that's fine.
        let _ = stdin.write_all(input);
    }
    Ok(child.wait_with_output()?.stdout)
}

/// The default one-line status: model, context and plan usage.
fn render(s: &Snapshot) -> String {
    let mut parts = Vec::new();
    if let Some(name) = &s.model.display_name {
        parts.push(name.clone());
    }
    if let Some(pct) = s.context_window.used_percentage {
        parts.push(format!("ctx {pct:.0}%"));
    }
    if let Some(limits) = &s.rate_limits {
        for (label, window) in [("5h", limits.five_hour), ("7d", limits.seven_day)] {
            if let Some(w) = window.filter(Window::is_current) {
                parts.push(format!("{label} {:.0}%", w.used_percentage));
            }
        }
    }
    if let Some(cost) = s.cost.total_cost_usd {
        parts.push(format!("${cost:.2}"));
    }
    parts.join(" · ")
}

/// What the dashboard knows from the status line.
#[derive(Default)]
pub struct Store {
    /// Latest snapshot per session ID.
    pub sessions: std::collections::HashMap<String, Snapshot>,
    /// Plan usage from the most recently written snapshot that has it.
    pub rate_limits: Option<(RateLimits, SystemTime)>,
    /// `true` once any snapshot has been found, i.e. the status line is set up.
    pub configured: bool,
}

/// Reads every saved snapshot. Cheap: one small JSON file per recent session.
pub fn load() -> Store {
    let mut store = Store::default();
    let Some(dir) = store_dir() else {
        return store;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return store;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(snapshot) = fs::read(&path)
            .and_then(|raw| serde_json::from_slice::<Snapshot>(&raw).map_err(io::Error::other))
        else {
            continue;
        };
        store.configured = true;
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(limits) = &snapshot.rate_limits
            && store
                .rate_limits
                .as_ref()
                .is_none_or(|(_, t)| modified > *t)
        {
            store.rate_limits = Some((limits.clone(), modified));
        }
        if let Some(id) = snapshot.session_id.clone() {
            store.sessions.insert(id, snapshot);
        }
    }
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "session_id": "abc-123",
        "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
        "context_window": {"context_window_size": 1000000, "used_percentage": 23.4},
        "cost": {"total_cost_usd": 1.5},
        "rate_limits": {
            "five_hour": {"used_percentage": 41.2, "resets_at": 4102444800},
            "seven_day": {"used_percentage": 12.0, "resets_at": 1000}
        }
    }"#;

    #[test]
    fn parses_the_documented_fields_and_renders_a_line() {
        let s: Snapshot = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(s.context_window.context_window_size, Some(1_000_000));
        let limits = s.rate_limits.clone().unwrap();
        assert!(limits.five_hour.unwrap().is_current());
        // resets_at in the past: the window no longer applies and isn't shown.
        assert!(!limits.seven_day.unwrap().is_current());
        assert_eq!(render(&s), "Opus · ctx 23% · 5h 41% · $1.50");
    }

    #[test]
    fn tolerates_missing_fields() {
        let s: Snapshot = serde_json::from_str(r#"{"session_id":"x"}"#).unwrap();
        assert!(s.rate_limits.is_none());
        assert_eq!(render(&s), "");
    }
}
