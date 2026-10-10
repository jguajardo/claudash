//! Locates the Claude Code executable portably.
//!
//! On Windows, `Command::new("claude")` only tries `claude.exe`, but an npm
//! install creates `claude.cmd` (next to an extensionless `claude` script that
//! Windows can't run). So we search `PATH` trying each `PATHEXT` extension and
//! launch the full path; since Rust 1.77 the standard library runs `.cmd`/`.bat`
//! through `cmd.exe` with safely escaped arguments.

use serde::Deserialize;
use std::{
    collections::HashMap,
    env,
    ffi::{OsStr, OsString},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

const PROGRAM: &str = "claude";

/// A `Command` ready to run Claude Code.
///
/// If it isn't found on `PATH`, the bare name is used so that spawning fails
/// with the usual "not found" error.
pub fn command() -> Command {
    // Tests never run the real Claude Code: this name isn't on any PATH.
    if cfg!(test) {
        return Command::new("claude-is-not-run-in-tests");
    }
    match path() {
        Some(path) => Command::new(path),
        None => Command::new(PROGRAM),
    }
}

/// Where `claude` was found on `PATH`, if it was.
pub fn path() -> Option<&'static PathBuf> {
    static RESOLVED: OnceLock<Option<PathBuf>> = OnceLock::new();
    RESOLVED.get_or_init(|| which(PROGRAM)).as_ref()
}

/// Finds a program on `PATH` (honoring `PATHEXT` on Windows).
pub fn which(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    find_executable(program, &path, &executable_extensions())
}

/// A session Claude Code reports as open, from `claude agents --json`
/// (<https://code.claude.com/docs/en/agent-view>).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct LiveSession {
    #[serde(rename = "sessionId", default)]
    pub session_id: String,
    /// Short ID used by `claude attach/logs/stop/respawn` (background sessions).
    pub id: Option<String>,
    /// "interactive" or "background".
    #[serde(default)]
    pub kind: String,
    /// "busy", "waiting" (on a permission or other input) or "idle".
    #[serde(default)]
    pub status: String,
    /// "working", "blocked", "done", "failed" or "stopped" (background sessions).
    pub state: Option<String>,
    /// Why a blocked session waits, such as "permission prompt".
    #[serde(rename = "waitingFor")]
    pub waiting_for: Option<String>,
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub cwd: Option<String>,
}

impl LiveSession {
    /// Waiting for a permission decision or other input only you can give.
    pub fn needs_you(&self) -> bool {
        self.status == "waiting" || self.state.as_deref() == Some("blocked")
    }

    pub fn is_background(&self) -> bool {
        self.kind == "background"
    }

    /// A process is running for it; finished background sessions have none.
    pub fn is_running(&self) -> bool {
        self.pid.is_some() || matches!(self.state.as_deref(), Some("working") | Some("blocked"))
    }
}

/// Sessions Claude Code reports as open (and, with `all`, finished background
/// sessions), keyed by session ID.
pub fn live_sessions(all: bool) -> Result<HashMap<String, LiveSession>, String> {
    let mut args = vec!["agents", "--json"];
    if all {
        args.push("--all");
    }
    let output = command()
        .args(&args)
        .output()
        .map_err(|e| format!("Could not run `claude agents`: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    parse_live(&output.stdout)
}

fn parse_live(json: &[u8]) -> Result<HashMap<String, LiveSession>, String> {
    let list: Vec<LiveSession> = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    Ok(list
        .into_iter()
        .filter(|s| !s.session_id.is_empty())
        .map(|s| (s.session_id.clone(), s))
        .collect())
}

/// `claude mcp logout <server>` in `cwd`: clears the server's stored OAuth credentials.
pub fn mcp_logout(server: &str, cwd: &Path) -> Result<String, String> {
    let output = self::command()
        .args(["mcp", "logout", server])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Could not run `claude mcp logout`: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Runs `claude <command> <id>` for a background session (`logs`, `stop`,
/// `respawn`) and returns its output.
pub fn background(command: &str, id: &str) -> Result<String, String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("unexpected session ID".into());
    }
    let output = self::command()
        .args([command, id])
        .output()
        .map_err(|e| format!("Could not run `claude {command}`: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("`claude {command}` failed")
        } else {
            err
        })
    }
}

/// `claude --version`, e.g. "2.1.285".
pub fn version() -> Option<String> {
    let output = command().arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

/// Compares dotted versions: `at_least("2.1.285", "2.1.277")` is true.
pub fn at_least(version: &str, minimum: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(version) >= parse(minimum)
}

/// Result of a one-off prompt sent to an existing session.
pub struct PromptReply {
    pub text: String,
    pub is_error: bool,
    pub cost_usd: Option<f64>,
    /// Tool calls Claude wanted to make but wasn't allowed to.
    pub permission_denials: usize,
}

/// Sends `prompt` to session `id` with `claude -p --resume`, in `cwd`.
///
/// The prompt goes through stdin so it can't be mistaken for a flag. Headless
/// runs can't ask for permission: tools that need approval are denied and
/// reported in `permission_denials`.
pub fn run_prompt(id: &str, cwd: &Path, prompt: &str) -> Result<PromptReply, String> {
    let mut child = command()
        .args(["-p", "--resume", id, "--output-format", "json"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run `claude -p`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("Could not send the prompt: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!("`claude -p` failed: {}", stderr.trim())
    })?;
    Ok(PromptReply {
        text: json["result"].as_str().unwrap_or_default().to_string(),
        is_error: json["is_error"]
            .as_bool()
            .unwrap_or(!output.status.success()),
        cost_usd: json["total_cost_usd"].as_f64(),
        permission_denials: json["permission_denials"].as_array().map_or(0, Vec::len),
    })
}

/// Extensions to try: those in `PATHEXT` on Windows; none elsewhere.
fn executable_extensions() -> Vec<OsString> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    pathext
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(OsString::from)
        .collect()
}

/// Searches each directory in `path`, in order, for `name`.
///
/// With empty `extensions` (Unix) it looks for the exact name; otherwise it
/// tries `name` + each extension and never the bare name.
fn find_executable(name: &str, path: &OsStr, extensions: &[OsString]) -> Option<PathBuf> {
    env::split_paths(path).find_map(|dir| {
        if extensions.is_empty() {
            let candidate = dir.join(name);
            return candidate.is_file().then_some(candidate);
        }
        extensions.iter().find_map(|ext| {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(file);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("claudash-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prefers_extension_over_bare_npm_shim() {
        let dir = temp_dir("pathext");
        // npm creates all three; the extensionless script can't run on Windows.
        for file in ["claude", "claude.cmd", "claude.ps1"] {
            fs::write(dir.join(file), "").unwrap();
        }
        let path = env::join_paths([&dir]).unwrap();
        // Lowercase: Windows file names are case-insensitive, Linux ones are not.
        let exts: Vec<OsString> = [".exe", ".cmd"].map(OsString::from).to_vec();

        let found = find_executable("claude", &path, &exts);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, Some(dir.join("claude.cmd")));
    }

    #[test]
    fn respects_path_order_and_missing_dirs() {
        let first = temp_dir("order-a");
        let second = temp_dir("order-b");
        fs::write(second.join("claude"), "").unwrap();
        let path = env::join_paths([first.join("missing"), first.clone(), second.clone()]).unwrap();

        let found = find_executable("claude", &path, &[]);
        let missing = find_executable("otro", &path, &[]);
        fs::remove_dir_all(&first).unwrap();
        fs::remove_dir_all(&second).unwrap();
        assert_eq!(found, Some(second.join("claude")));
        assert_eq!(missing, None);
    }

    /// Actually runs a fake `claude.cmd`: checks that Rust launches it through
    /// `cmd.exe` and that arguments arrive intact. Windows only (runs in CI).
    #[cfg(windows)]
    #[test]
    fn runs_cmd_shim_with_arguments() {
        let dir = temp_dir("cmd-shim");
        fs::write(dir.join("claude.cmd"), "@echo off\r\necho args=%1 %2\r\n").unwrap();
        let path = env::join_paths([&dir]).unwrap();
        let exe = find_executable("claude", &path, &executable_extensions()).unwrap();

        let output = Command::new(&exe)
            .args(["--resume", "0fb00c9a-2bf4-4096-ba7e-c46880a43b21"])
            .output()
            .unwrap();
        fs::remove_dir_all(&dir).unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            stdout.trim(),
            "args=--resume 0fb00c9a-2bf4-4096-ba7e-c46880a43b21"
        );
    }

    #[test]
    fn parses_agents_json_and_versions() {
        let live = parse_live(
            br#"[{"pid":1,"sessionId":"a","kind":"interactive","status":"busy"},
                 {"id":"7c5d","sessionId":"b","kind":"background","state":"blocked",
                  "status":"waiting","waitingFor":"permission prompt"},
                 {"status":"idle"}]"#,
        )
        .unwrap();
        assert_eq!(live.len(), 2);
        assert!(!live["a"].needs_you());
        assert!(live["b"].needs_you() && live["b"].is_background());
        assert_eq!(live["b"].id.as_deref(), Some("7c5d"));
        assert!(at_least("2.1.285", "2.1.277"));
        assert!(!at_least("2.1.99", "2.1.277"));
        assert!(at_least("3.0.0", "2.1.277"));
    }
}
