//! Real MCP server status, obtained from `claude mcp list`.
//!
//! That command already resolves every configuration source (`~/.claude.json`,
//! the project's `.mcp.json`, plugins and claude.ai connectors) and health-checks
//! each server. Project-scoped servers depend on the directory it runs in, so
//! it runs in the selected session's project folder. It takes several seconds,
//! so the dashboard runs it on a background thread.
//!
//! Line format:
//!   `<name>: <command or URL> - <symbol> <status>[ — <detail>]`
//! Only name and status are kept: the command/URL may contain credentials.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpStatus {
    Connected,
    NeedsAuth,
    Failed(String),
    NotConfigured,
    Unknown(String),
}

pub struct McpServer {
    /// Name as `claude mcp list` prints it, e.g. `plugin:context7:context7`.
    pub full_name: String,
    /// Display name, without the source prefix (`plugin:x:`, `claude.ai `).
    pub name: String,
    /// Where it comes from: "local", "claude.ai" or the plugin name.
    pub source: String,
    pub status: McpStatus,
}

pub type McpResult = Result<Vec<McpServer>, String>;

/// Runs `claude mcp list` in `cwd`. Takes several seconds; call it off the UI thread.
pub fn check(cwd: &Path) -> McpResult {
    let output = crate::claude_cli::command()
        .args(["mcp", "list"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Could not run `claude mcp list`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`claude mcp list` failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(parse_list(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_list(stdout: &str) -> Vec<McpServer> {
    stdout.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<McpServer> {
    // The name ends at the first ": " (plugin names use ':' without a space)
    // and the status starts after the last " - ".
    let (full_name, rest) = line.split_once(": ")?;
    let (_target, status_text) = rest.rsplit_once(" - ")?;
    let full_name = full_name.trim();
    let (source, name) = split_source(full_name);
    Some(McpServer {
        full_name: full_name.to_string(),
        name,
        source,
        status: parse_status(status_text.trim()),
    })
}

fn split_source(full_name: &str) -> (String, String) {
    if let Some(rest) = full_name.strip_prefix("plugin:") {
        // plugin:<plugin>:<server>
        return match rest.split_once(':') {
            Some((plugin, server)) => (plugin.to_string(), server.to_string()),
            None => ("plugin".to_string(), rest.to_string()),
        };
    }
    if let Some(name) = full_name.strip_prefix("claude.ai ") {
        return ("claude.ai".to_string(), name.to_string());
    }
    ("local".to_string(), full_name.to_string())
}

fn parse_status(text: &str) -> McpStatus {
    let (head, detail) = match text.split_once(" — ") {
        Some((head, detail)) => (head, detail.trim()),
        None => (text, ""),
    };
    let lower = head.to_lowercase();
    if lower.contains("connected") && !lower.contains("failed") {
        McpStatus::Connected
    } else if lower.contains("needs authentication") {
        McpStatus::NeedsAuth
    } else if lower.contains("failed") {
        McpStatus::Failed(detail.to_string())
    } else if lower.contains("not configured") {
        McpStatus::NotConfigured
    } else {
        McpStatus::Unknown(text.to_string())
    }
}

/// How many log lines to show.
const LOG_LINES: usize = 200;

/// Where Claude Code writes MCP server logs for a project (observed, not
/// documented): `<cache>/claude-cli-nodejs/<project>/mcp-logs-<server>/`, one
/// JSONL file per connection, where `<project>` and `<server>` have every
/// non-alphanumeric character replaced by `-`.
pub fn log_dir(cwd: &Path, full_name: &str) -> Option<PathBuf> {
    let mut root = dirs::cache_dir()?.join("claude-cli-nodejs");
    if cfg!(windows) {
        root.push("Cache");
    }
    let project = sanitize(&cwd.to_string_lossy());
    Some(
        root.join(project)
            .join(format!("mcp-logs-{}", sanitize(full_name))),
    )
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Every MCP server with logs for project `cwd`: (server as named on disk, directory).
pub fn log_dirs(cwd: &Path) -> Vec<(String, PathBuf)> {
    let Some(project) = log_dir(cwd, "").and_then(|d| d.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(project)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let server = name.strip_prefix("mcp-logs-")?.to_string();
            Some((server, e.path()))
        })
        .collect();
    dirs.sort();
    dirs
}

/// The last lines of the newest log for `full_name` in project `cwd`,
/// formatted as `HH:MM:SS  level  message`. Returns the log file and its lines.
pub fn read_log(cwd: &Path, full_name: &str) -> Result<(PathBuf, Vec<String>), String> {
    let dir = log_dir(cwd, full_name).ok_or("Could not find the cache directory")?;
    read_log_dir(&dir)
}

/// Like [`read_log`], for a server's log directory.
pub fn read_log_dir(dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let dir = dir.to_path_buf();
    let newest = fs::read_dir(&dir)
        .map_err(|_| {
            format!(
                "No logs yet for this server in {}",
                crate::paths::display(&dir)
            )
        })?
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "jsonl" || ext == "txt")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
        .ok_or_else(|| {
            format!(
                "No logs yet for this server in {}",
                crate::paths::display(&dir)
            )
        })?;
    let text = fs::read_to_string(&newest).map_err(|e| e.to_string())?;

    let mut lines = Vec::new();
    for raw in text.lines() {
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(entry) => {
                let time = entry["timestamp"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                let (level, message) = match (entry["error"].as_str(), entry["debug"].as_str()) {
                    (Some(e), _) => ("error", e),
                    (None, Some(d)) => ("debug", d),
                    _ => ("", raw),
                };
                let mut parts = message.lines();
                lines.push(format!(
                    "{time:8}  {level:5}  {}",
                    parts.next().unwrap_or("")
                ));
                lines.extend(parts.map(|p| format!("{:17}{p}", "")));
            }
            Err(_) => lines.push(raw.to_string()),
        }
    }
    let skip = lines.len().saturating_sub(LOG_LINES);
    Ok((newest, lines.split_off(skip)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_line_kinds() {
        let out = "Checking MCP server health…

claude.ai Gmail: https://gmailmcp.googleapis.com/mcp/v1 - ✔ Connected
plugin:vercel:vercel: https://mcp.vercel.com (HTTP) - ! Needs authentication
plugin:engineering:github: https://api.githubcopilot.com/mcp/ (HTTP) - ✘ Failed to connect — Incompatible auth server
plugin:engineering:google calendar:  (HTTP) - - Not configured
postgres: npx -y server-postgres postgresql://u:p@host/db - ✔ Connected
";
        let servers = parse_list(out);
        assert_eq!(servers.len(), 5);

        assert_eq!(servers[0].source, "claude.ai");
        assert_eq!(servers[0].name, "Gmail");
        assert_eq!(servers[0].status, McpStatus::Connected);

        assert_eq!(servers[1].source, "vercel");
        assert_eq!(servers[1].status, McpStatus::NeedsAuth);

        assert_eq!(servers[2].name, "github");
        assert_eq!(
            servers[2].status,
            McpStatus::Failed("Incompatible auth server".into())
        );

        assert_eq!(servers[3].name, "google calendar");
        assert_eq!(servers[3].status, McpStatus::NotConfigured);

        assert_eq!(servers[1].full_name, "plugin:vercel:vercel");
        assert_eq!(servers[4].source, "local");
        assert_eq!(servers[4].name, "postgres");
    }

    #[test]
    fn log_dir_follows_claude_codes_naming() {
        let dir = log_dir(Path::new("/home/me/my_app"), "claude.ai Google Drive").unwrap();
        assert!(dir.ends_with("-home-me-my-app/mcp-logs-claude-ai-Google-Drive"));
        let dir = log_dir(Path::new("/p"), "plugin:context7:context7").unwrap();
        assert!(dir.ends_with("mcp-logs-plugin-context7-context7"));
    }
}
