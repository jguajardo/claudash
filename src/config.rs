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
        let config =
            parse("view = \"Insights\"\ncontext_limit = \"200k\"\nnotify = false\n").unwrap();
        assert_eq!(view_index(config.view.as_deref().unwrap()), Ok(3));
        assert_eq!(config.context_limit.unwrap().as_text(), "200k");
        assert_eq!(config.notify, Some(false));
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
}
