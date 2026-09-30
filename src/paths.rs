//! Where Claude Code keeps its files.
//!
//! See <https://code.claude.com/docs/en/sessions> and
//! <https://code.claude.com/docs/en/memory> for the documented locations.

use std::{
    env,
    path::{Path, PathBuf},
};

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
pub fn claude_home() -> Option<PathBuf> {
    match env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".claude")),
    }
}

/// Directory with one sub-directory of session transcripts per project.
pub fn projects_dir() -> Option<PathBuf> {
    claude_home().map(|dir| dir.join("projects"))
}

/// Directory that holds managed-policy files (`CLAUDE.md`, `managed-settings.json`).
pub fn managed_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

/// claudash's own cache (status line snapshots, hook state): safe to delete.
pub fn claudash_cache() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("claudash"))
}

/// Path for display, with the home directory shortened to `~`.
pub fn display(path: &Path) -> String {
    match dirs::home_dir() {
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}
