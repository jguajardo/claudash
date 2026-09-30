//! Real MCP server status, obtained from `claude mcp list`.
//!
//! That command already resolves every configuration source (`~/.claude.json`,
//! the project's `.mcp.json`, plugins and claude.ai connectors) and health-checks
//! each server. It takes several seconds, so it runs on a separate thread and
//! the result comes back over a channel.
//!
//! Line format:
//!   `<name>: <command or URL> - <symbol> <status>[ — <detail>]`
//! Only name and status are kept: the command/URL may contain credentials.

use std::{
    sync::mpsc::{self, Receiver},
    thread,
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
    /// Display name, without the source prefix (`plugin:x:`, `claude.ai `).
    pub name: String,
    /// Where it comes from: "local", "claude.ai" or the plugin name.
    pub source: String,
    pub status: McpStatus,
}

pub type McpResult = Result<Vec<McpServer>, String>;

/// Runs `claude mcp list` in the background.
pub fn spawn_check() -> Receiver<McpResult> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(run_check());
    });
    rx
}

fn run_check() -> McpResult {
    let output = crate::claude_cli::command()
        .args(["mcp", "list"])
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
    let (source, name) = split_source(full_name.trim());
    Some(McpServer {
        name,
        source,
        status: parse_status(status_text.trim()),
    })
}

fn split_source(full_name: &str) -> (String, String) {
    if let Some(rest) = full_name.strip_prefix("plugin:") {
        // plugin:<plugin>:<servidor>
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

        assert_eq!(servers[4].source, "local");
        assert_eq!(servers[4].name, "postgres");
    }
}
