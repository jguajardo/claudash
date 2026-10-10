//! `claudash setup`: registers `claudash statusline` and `claudash hook` in
//! Claude Code's user settings (`~/.claude/settings.json`).
//!
//! Without `--apply` it only prints the changes. With `--apply` it backs the
//! file up and writes them; running it again changes nothing. `--remove`
//! undoes it. An existing status line is kept by wrapping it:
//! `claudash statusline -- <your command>`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use crate::hooks;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Show,
    Apply,
    Remove,
}

/// The command Claude Code should run: this binary by absolute path, so it
/// works even when Claude Code's shell doesn't have ~/.cargo/bin on PATH.
fn exe() -> String {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "claudash".into());
    shell_quote(&path)
}

/// Quotes a word for the POSIX shell Claude Code runs commands with.
fn shell_quote(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~:".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

fn is_ours(command: &str, sub: &str) -> bool {
    command.contains("claudash") && command.contains(sub)
}

/// What `claudash setup --apply` has registered in the user settings:
/// (status line, number of hook events out of [`hooks::EVENTS`]).
pub fn installed() -> (bool, usize) {
    let settings: Option<Value> = crate::paths::claude_home()
        .and_then(|home| fs::read_to_string(home.join("settings.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    let Some(settings) = settings else {
        return (false, 0);
    };
    let statusline = settings["statusLine"]["command"]
        .as_str()
        .is_some_and(|c| is_ours(c, " statusline"));
    let hooks = hooks::EVENTS
        .iter()
        .filter(|event| {
            settings["hooks"][**event]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
                .any(|h| h["command"].as_str().is_some_and(|c| is_ours(c, " hook")))
        })
        .count();
    (statusline, hooks)
}

/// Applies (or removes) claudash's entries. Returns what changed, in words.
fn update(settings: &mut Map<String, Value>, exe: &str, mode: Mode) -> Vec<String> {
    let mut changes = Vec::new();

    // Status line.
    let current = settings
        .get("statusLine")
        .and_then(|s| s["command"].as_str())
        .map(str::to_owned);
    match (mode, current) {
        (Mode::Remove, Some(cmd)) if is_ours(&cmd, " statusline") => {
            // Restore a wrapped command, or drop ours.
            match cmd.split_once(" statusline -- sh -c ") {
                Some((_, quoted)) => {
                    let original = unquote(quoted);
                    settings["statusLine"]["command"] = Value::String(original.clone());
                    changes.push(format!("status line restored to `{original}`"));
                }
                None => {
                    settings.remove("statusLine");
                    changes.push("status line removed".into());
                }
            }
        }
        (Mode::Remove, _) => {}
        (_, Some(cmd)) if is_ours(&cmd, " statusline") => {}
        (_, Some(cmd)) => {
            let wrapped = format!("{exe} statusline -- sh -c {}", shell_quote(&cmd));
            settings["statusLine"]["command"] = Value::String(wrapped);
            changes.push(format!(
                "status line: your `{cmd}` keeps running, wrapped by claudash statusline"
            ));
        }
        (_, None) => {
            settings.insert(
                "statusLine".into(),
                json!({"type": "command", "command": format!("{exe} statusline")}),
            );
            changes.push("status line: claudash statusline".into());
        }
    }

    // Hooks.
    let hooks_obj = settings
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks_map) = hooks_obj.as_object_mut() else {
        changes.push("skipped hooks: `hooks` in settings.json isn't an object".into());
        return changes;
    };
    for event in hooks::EVENTS {
        let groups = hooks_map
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let ours = |h: &Value| h["command"].as_str().is_some_and(|c| is_ours(c, " hook"));
        let present = groups
            .iter()
            .any(|g| g["hooks"].as_array().is_some_and(|hs| hs.iter().any(ours)));
        match mode {
            Mode::Remove if present => {
                for group in groups.iter_mut() {
                    if let Some(hs) = group["hooks"].as_array_mut() {
                        hs.retain(|h| !ours(h));
                    }
                }
                groups.retain(|g| g["hooks"].as_array().is_none_or(|hs| !hs.is_empty()));
                changes.push(format!("hook removed: {event}"));
            }
            Mode::Show | Mode::Apply if !present => {
                groups.push(json!({
                    "hooks": [{"type": "command", "command": format!("{exe} hook"), "async": true}]
                }));
                changes.push(format!("hook: {event} → claudash hook (async)"));
            }
            _ => {}
        }
    }
    // Leave no empty leftovers behind on removal.
    if mode == Mode::Remove {
        hooks_map.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
        if hooks_map.is_empty() {
            settings.remove("hooks");
        }
    }
    changes
}

/// Reverses `shell_quote` for a single-quoted word.
fn unquote(word: &str) -> String {
    match word.strip_prefix('\'').and_then(|w| w.strip_suffix('\'')) {
        Some(inner) => inner.replace(r"'\''", "'"),
        None => word.to_string(),
    }
}

/// What applying a mode to the settings file did.
pub struct Applied {
    pub file: PathBuf,
    /// What changed, or would change with `Mode::Show`, one line each.
    pub changes: Vec<String>,
    /// The copy made of the file before writing it.
    pub backup: Option<PathBuf>,
}

/// Updates the settings file at `file` so Claude Code runs `exe` for the
/// status line and the hooks (or stops doing it). `Mode::Show` only reports.
fn apply_to(file: &Path, exe: &str, mode: Mode) -> io::Result<Applied> {
    let mut settings: Map<String, Value> = match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            io::Error::other(format!(
                "{} isn't valid JSON ({e}); fix it first",
                file.display()
            ))
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e),
    };
    let changes = update(&mut settings, exe, mode);
    let backup = if changes.is_empty() || mode == Mode::Show {
        None
    } else {
        write_with_backup(file, &settings)?
    };
    Ok(Applied {
        file: file.to_path_buf(),
        changes,
        backup,
    })
}

/// `apply_to` on Claude Code's user settings, without printing anything.
pub fn apply(mode: Mode) -> io::Result<Applied> {
    // Tests never write Claude Code's real settings; they use `apply_to`.
    if cfg!(test) {
        return Err(io::Error::other("no settings file in tests"));
    }
    let home = crate::paths::claude_home()
        .ok_or_else(|| io::Error::other("could not find Claude Code's config directory"))?;
    apply_to(&home.join("settings.json"), &exe(), mode)
}

pub fn run(mode: Mode) -> io::Result<()> {
    let done = apply(mode)?;
    let file = done.file.display();
    if done.changes.is_empty() {
        println!("Nothing to do: {file} is already up to date.");
        return Ok(());
    }
    println!("Changes to {file}:");
    for change in &done.changes {
        println!("  • {change}");
    }
    if mode == Mode::Show {
        println!(
            "\nRun `claudash setup --apply` to write them (a backup is made first).\n\
             This lets claudash show plan usage, the real context window and which\n\
             sessions need you. Hooks only record the event type and time."
        );
        return Ok(());
    }
    if let Some(backup) = &done.backup {
        println!("Backup: {}", backup.display());
    }
    println!("\nDone. New Claude Code sessions pick it up; restart open ones.");
    Ok(())
}

/// Writes the settings, after copying the file there was; returns the copy.
fn write_with_backup(file: &Path, settings: &Map<String, Value>) -> io::Result<Option<PathBuf>> {
    let backup = if file.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup = file.with_extension(format!("json.claudash-{stamp}.bak"));
        fs::copy(file, &backup)?;
        Some(backup)
    } else {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        None
    };
    let mut text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    text.push('\n');
    let tmp = file.with_extension("json.claudash-tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, file)?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(json: &str, mode: Mode) -> (Value, Vec<String>) {
        let mut map: Map<String, Value> = serde_json::from_str(json).unwrap();
        let changes = update(&mut map, "/bin/claudash", mode);
        (Value::Object(map), changes)
    }

    #[test]
    fn adds_statusline_and_hooks_once() {
        let (settings, changes) = apply(r#"{"theme":"dark"}"#, Mode::Apply);
        assert_eq!(changes.len(), 1 + hooks::EVENTS.len());
        assert_eq!(
            settings["statusLine"]["command"],
            "/bin/claudash statusline"
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "/bin/claudash hook"
        );
        assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["async"], true);
        // Existing keys keep their place.
        assert_eq!(
            settings.as_object().unwrap().keys().next().unwrap(),
            "theme"
        );

        let (_, again) = apply(&settings.to_string(), Mode::Apply);
        assert!(again.is_empty(), "second run changed: {again:?}");
    }

    #[test]
    fn wraps_an_existing_statusline_and_restores_it() {
        let original = r#"{"statusLine":{"type":"command","command":"~/sl.sh --it's","padding":1},
            "hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#;
        let (settings, _) = apply(original, Mode::Apply);
        assert_eq!(
            settings["statusLine"]["command"],
            r#"/bin/claudash statusline -- sh -c '~/sl.sh --it'\''s'"#
        );
        assert_eq!(settings["statusLine"]["padding"], 1);
        // The user's own Stop hook is kept next to ours.
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 2);

        let (restored, _) = apply(&settings.to_string(), Mode::Remove);
        assert_eq!(restored["statusLine"]["command"], "~/sl.sh --it's");
        assert_eq!(
            restored["hooks"]["Stop"][0]["hooks"][0]["command"],
            "say done"
        );
        assert!(restored["hooks"].get("Notification").is_none());
    }

    #[test]
    fn remove_leaves_no_trace_when_only_ours() {
        let (settings, _) = apply("{}", Mode::Apply);
        let (removed, _) = apply(&settings.to_string(), Mode::Remove);
        assert_eq!(removed, json!({}));
    }
    #[test]
    fn applies_to_a_file_quietly_and_backs_it_up() {
        let dir = std::env::temp_dir().join(format!("claudash-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        fs::write(&file, r#"{"model": "opus"}"#).unwrap();

        let done = apply_to(&file, "/bin/claudash", Mode::Apply).unwrap();
        assert!(!done.changes.is_empty());
        let backup = done.backup.expect("the file existed, so it's backed up");
        assert_eq!(fs::read_to_string(backup).unwrap(), r#"{"model": "opus"}"#);
        let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["model"], "opus");
        assert!(written["hooks"]["Stop"].is_array());

        // Nothing left to change: nothing is written or backed up again.
        let again = apply_to(&file, "/bin/claudash", Mode::Apply).unwrap();
        assert!(again.changes.is_empty() && again.backup.is_none());
        // Showing never writes.
        fs::write(&file, "{}").unwrap();
        let shown = apply_to(&file, "/bin/claudash", Mode::Show).unwrap();
        assert!(!shown.changes.is_empty() && shown.backup.is_none());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
