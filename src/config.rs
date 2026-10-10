//! Settings from `config.toml` in claudash's config directory
//! (`~/.config/claudash/` on Linux). Command-line flags and environment
//! variables override them; every setting is optional.

use std::{fs, io, path::PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// View claudash opens on.
    pub view: Option<String>,
    /// Context window used when the status line hasn't reported one.
    pub context_limit: Option<Limit>,
    /// Desktop notifications and bell.
    pub notify: Option<bool>,
    /// `false` draws without colors, like the `NO_COLOR` environment variable.
    pub colors: Option<bool>,
    /// Snapshot each git project's work tree before every prompt and after
    /// every reply, to undo what `/rewind` can't (see `snapshots`).
    pub snapshots: Option<bool>,
    /// Continue sessions a plan limit stopped, in the background, as soon as
    /// it resets (while claudash is open).
    pub auto_continue: Option<bool>,
    /// Your Claude plan: "pro", "max5x", "max20x" or "api". claudash reads it
    /// from Claude Code's account info when this isn't set.
    pub plan: Option<String>,
    /// Where sessions open inside Zellij or tmux: "tab", "pane" or "here".
    pub open_in: Option<String>,
}

/// `context_limit = 1000000` or `context_limit = "1M"`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Limit {
    Tokens(u64),
    Text(String),
}

impl Limit {
    pub fn as_text(&self) -> String {
        match self {
            Limit::Tokens(n) => n.to_string(),
            Limit::Text(t) => t.clone(),
        }
    }
}

/// Names for the `view` setting and `--view`, in number-key order.
pub const VIEW_NAMES: [&str; 4] = ["now", "sessions", "projects", "insights"];

/// Index into [`crate::app::VIEW_KEYS`] for a view name.
pub fn view_index(name: &str) -> Result<usize, String> {
    // Views from before 0.8 open where they went.
    let name = match name.trim().to_lowercase().as_str() {
        "activity" | "logs" => "now".to_string(),
        "usage" => "insights".to_string(),
        "ecosystem" => "projects".to_string(),
        other => other.to_string(),
    };
    VIEW_NAMES
        .iter()
        .position(|v| v.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            format!(
                "unknown view '{name}' (use one of: {})",
                VIEW_NAMES.join(", ")
            )
        })
}

pub const TEMPLATE: &str = "\
# claudash settings. Every setting is optional; command-line flags and
# environment variables win over this file.

# View to open on: now, sessions, projects or insights.
# view = \"now\"

# Context window used when Claude Code's status line hasn't reported one:
# 1M, 200k or a number of tokens. CLAUDASH_CONTEXT_LIMIT and --context-limit win.
# context_limit = \"1M\"

# Desktop notifications and bell when a session needs you, finishes a reply or
# your plan usage crosses 80% and 95%. --no-notify wins.
# notify = true

# Colors. false draws without them, like setting NO_COLOR.
# colors = true

# Safety net for what /rewind can't undo (Bash and subagent changes): before
# each prompt and after each reply, commit the project's files to a separate
# repository in claudash's data directory. Needs `claudash setup --apply`.
# snapshots = false

# Continue sessions a plan limit stopped, in the background, as soon as the
# limit resets, while claudash is open. Off: Enter on the alert in Now does it.
# auto_continue = false

# Your Claude plan, for Insights › Is your plan worth it: \"pro\", \"max5x\",
# \"max20x\" or \"api\". Read from Claude Code's account info when not set.
# plan = \"max5x\"

# Inside Zellij or tmux, where a session you resume, attach to or start from
# claudash opens: \"tab\" (a new tab; a window in tmux), \"pane\" (next to
# claudash) or \"here\" (claudash's own terminal, which then waits for it).
# Outside them it is always here.
# open_in = \"tab\"
";

pub fn path() -> Option<PathBuf> {
    // Tests never read the real settings.
    if cfg!(test) {
        return None;
    }
    dirs::config_dir().map(|dir| dir.join("claudash").join("config.toml"))
}

/// The settings, or the defaults when there's no file.
pub fn load() -> Result<Config, String> {
    let Some(path) = path() else {
        return Ok(Config::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if let Some(view) = &config.view {
        view_index(view)?;
    }
    Ok(config)
}

/// `text` with `key = value`: on the line that sets it or has it commented
/// out, or added at the end. Everything else stays as it is.
pub fn with_setting(text: &str, key: &str, value: &str) -> String {
    let sets_it = |line: &str| {
        line.trim_start()
            .strip_prefix(key)
            .is_some_and(|after| after.trim_start().starts_with('='))
    };
    let shows_it = |line: &str| {
        let line = line.trim_start();
        line.starts_with('#') && sets_it(line.trim_start_matches('#'))
    };
    let setting = format!("{key} = {value}");
    let mut lines: Vec<&str> = text.lines().collect();
    // The line that sets it wins over a commented example of it: turning
    // the example on as well would give the file the key twice.
    let at = lines
        .iter()
        .position(|line| sets_it(line))
        .or_else(|| lines.iter().position(|line| shows_it(line)));
    match at {
        Some(at) => lines[at] = &setting,
        None => lines.push(&setting),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Writes `key = value` to the settings file at `path`; a file that doesn't
/// exist starts from the commented template.
fn set_in(path: &std::path::Path, key: &str, value: &str) -> io::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => return Err(e),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, with_setting(&text, key, value))
}

/// Writes `key = value` to the settings file and returns where that is.
pub fn set(key: &str, value: &str) -> io::Result<PathBuf> {
    let path = path().ok_or_else(|| io::Error::other("could not find the config directory"))?;
    set_in(&path, key, value)?;
    Ok(path)
}

/// Writes the commented template, unless a file is already there.
pub fn init() -> io::Result<PathBuf> {
    let path = path().ok_or_else(|| io::Error::other("could not find the config directory"))?;
    if path.exists() {
        return Err(io::Error::other(format!(
            "{} already exists",
            path.display()
        )));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, TEMPLATE)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_and_rejects_mistakes() {
        assert_eq!(parse(TEMPLATE), Ok(Config::default()));
        assert_eq!(
            parse("open_in = \"pane\"\n").unwrap().open_in.as_deref(),
            Some("pane")
        );
        let config =
            parse("view = \"Insights\"\ncontext_limit = \"200k\"\nnotify = false\n").unwrap();
        assert_eq!(view_index(config.view.as_deref().unwrap()), Ok(3));
        assert_eq!(config.context_limit.unwrap().as_text(), "200k");
        assert_eq!(config.notify, Some(false));
        assert_eq!(parse("colors = false").unwrap().colors, Some(false));
        assert_eq!(
            parse("context_limit = 500000").unwrap().context_limit,
            Some(Limit::Tokens(500_000))
        );
        assert!(parse("view = \"nope\"").is_err());
        // Names from before 0.8 still work.
        assert_eq!(view_index("activity"), Ok(0));
        assert_eq!(view_index("Usage"), Ok(3));
        assert_eq!(view_index("ecosystem"), Ok(2));
        assert!(parse("colour = \"red\"").is_err());
    }

    #[test]
    fn turns_a_setting_on_where_the_file_mentions_it() {
        // The template has it commented out: that line becomes the setting.
        let text = with_setting(TEMPLATE, "snapshots", "true");
        assert_eq!(parse(&text).unwrap().snapshots, Some(true));
        assert_eq!(text.lines().count(), TEMPLATE.lines().count());
        assert!(text.contains("\nsnapshots = true\n") && !text.contains("# snapshots"));

        // Already set: the value changes in place, and nothing else does.
        let off = with_setting(&text, "snapshots", "false");
        assert_eq!(parse(&off).unwrap().snapshots, Some(false));
        assert_eq!(off.matches("snapshots =").count(), 1);
        assert_eq!(with_setting(&off, "snapshots", "true"), text);
    }

    #[test]
    fn changes_the_line_that_sets_it_not_a_commented_example() {
        // The template's example stays a comment when the setting is given below.
        let text = "# snapshots = false\nview = \"now\"\nsnapshots = true\n";
        let off = with_setting(text, "snapshots", "false");
        assert_eq!(
            off,
            "# snapshots = false\nview = \"now\"\nsnapshots = false\n"
        );
        assert_eq!(parse(&off).unwrap().snapshots, Some(false));
    }

    #[test]
    fn adds_a_setting_the_file_doesnt_mention() {
        let text = with_setting("view = \"now\"", "snapshots", "true");
        assert_eq!(text, "view = \"now\"\nsnapshots = true\n");
        // A setting whose name starts the same is another setting.
        let text = with_setting("plan_note = 1\n", "plan", "\"pro\"");
        assert_eq!(text, "plan_note = 1\nplan = \"pro\"\n");
    }

    #[test]
    fn writes_a_setting_to_a_file_that_may_not_exist() {
        let dir = std::env::temp_dir().join(format!("claudash-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("claudash").join("config.toml");
        // No file: the commented template, with that setting on.
        set_in(&file, "snapshots", "true").unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# claudash settings") && text.contains("\nsnapshots = true\n"));
        // An existing file keeps everything else.
        fs::write(&file, "view = \"sessions\"\nsnapshots = true\n").unwrap();
        set_in(&file, "snapshots", "false").unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "view = \"sessions\"\nsnapshots = false\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
