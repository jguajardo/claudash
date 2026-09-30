//! Reads the real Claude Code sessions from `~/.claude/projects`.
//!
//! On-disk layout: `~/.claude/projects/<encoded-path>/<session-id>.jsonl`,
//! where each line of the `.jsonl` is a JSON record. The directory name
//! replaces `/` with `-` (and can't be reversed reliably), so the real project
//! path is taken from the records' `cwd` field.
//!
//! Token usage comes from `message.usage` of each assistant response. A single
//! response is written over several lines (one per content block) with the
//! same `requestId`, so usage is deduplicated by that field.

use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::Deserialize;

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    /// Display path (with `~`).
    pub project_path: String,
    /// Real directory the session ran in; needed to resume it.
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub modified: SystemTime,
    size: u64,
    pub tokens: SessionTokens,
}

#[derive(Clone, Default)]
pub struct SessionTokens {
    /// Sum over all responses in the main session (subagents excluded).
    pub total: Usage,
    /// Input tokens of the last response = current context usage.
    pub context_used: u64,
    pub model: Option<String>,
    /// Accumulated cost from the last `cost-state` record, if any.
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Copy, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl Usage {
    /// Everything that takes up context in a request.
    pub fn context(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Only the fields we care about; serde ignores the rest.
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<Message>,
    #[serde(rename = "totalCostUSD")]
    total_cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<Usage>,
}

pub fn projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("projects"))
}

/// Loads every session, newest first.
///
/// `previous` acts as a cache: if a file hasn't changed (same mtime and size)
/// its previous result is reused instead of reading it again.
pub fn load_sessions(root: &Path, previous: &[Session]) -> io::Result<Vec<Session>> {
    let cache: HashMap<&Path, &Session> = previous.iter().map(|s| (s.path.as_path(), s)).collect();
    let mut sessions = Vec::new();
    for project in fs::read_dir(root)?.flatten() {
        let Ok(entries) = fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let (Ok(modified), size) = (meta.modified(), meta.len()) else {
                continue;
            };
            let session = match cache.get(path.as_path()) {
                Some(s) if s.modified == modified && s.size == size => Some((*s).clone()),
                _ => parse_session(
                    &path,
                    &project.file_name().to_string_lossy(),
                    modified,
                    size,
                ),
            };
            sessions.extend(session);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(sessions)
}

fn parse_session(path: &Path, dir_name: &str, modified: SystemTime, size: u64) -> Option<Session> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let reader = BufReader::new(fs::File::open(path).ok()?);

    let mut ai_title = None;
    let mut last_prompt = None;
    let mut cwd = None;
    let mut git_branch = None;
    // requestId -> usage; arrival order tells us which response came last.
    let mut usage_by_request: HashMap<String, Usage> = HashMap::new();
    let mut last_usage: Option<Usage> = None;
    let mut model = None;
    let mut cost_usd = None;

    for line in reader.lines().map_while(Result::ok) {
        // Cheap filter before deserializing: skips user messages, attachments,
        // snapshots, etc.
        let is_assistant = line.contains("\"type\":\"assistant\"");
        let wanted = is_assistant
            || line.contains("\"ai-title\"")
            || line.contains("\"last-prompt\"")
            || line.contains("\"cost-state\"")
            || (cwd.is_none() && line.contains("\"cwd\""));
        if !wanted {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match record.kind.as_deref() {
            Some("ai-title") if record.ai_title.is_some() => ai_title = record.ai_title,
            Some("last-prompt") if record.last_prompt.is_some() => last_prompt = record.last_prompt,
            Some("cost-state") => cost_usd = record.total_cost_usd.or(cost_usd),
            Some("assistant") if !record.is_sidechain => {
                if let Some(msg) = record.message
                    && let Some(usage) = msg.usage
                    // "<synthetic>" messages are generated locally, without an API call.
                    && msg.model.as_deref() != Some("<synthetic>")
                {
                    if let Some(req) = record.request_id {
                        usage_by_request.insert(req, usage);
                    }
                    last_usage = Some(usage);
                    model = msg.model;
                }
            }
            _ => {}
        }
        if cwd.is_none() && record.cwd.is_some() {
            cwd = record.cwd;
            git_branch = record.git_branch.filter(|b| !b.is_empty() && b != "HEAD");
        }
    }

    let mut total = Usage::default();
    for usage in usage_by_request.values() {
        total.add(usage);
    }
    let context_used = last_usage.map(|u| u.context()).unwrap_or(0);

    let title = ai_title
        .or_else(|| last_prompt.map(|p| p.lines().next().unwrap_or_default().to_string()))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("(untitled) {}", &id[..id.len().min(8)]));
    // Without `cwd`, approximate it by decoding the directory name.
    let project_path = cwd.clone().unwrap_or_else(|| dir_name.replace('-', "/"));

    Some(Session {
        id,
        path: path.to_path_buf(),
        title,
        project_path: shorten_home(&project_path),
        cwd: cwd.map(PathBuf::from),
        git_branch,
        modified,
        size,
        tokens: SessionTokens {
            total,
            context_used,
            model,
            cost_usd,
        },
    })
}

fn shorten_home(path: &str) -> String {
    match dirs::home_dir().and_then(|h| h.to_str().map(str::to_owned)) {
        Some(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// "5 min ago", "3 h ago", "yesterday", "4 days ago"...
pub fn relative_age(time: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(time)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match secs {
        0..60 => "just now".into(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "yesterday".into(),
        172_800..2_592_000 => format!("{} days ago", secs / 86_400),
        _ => format!("{} months ago", secs / 2_592_000),
    }
}

/// 1234 -> "1.2k", 2_500_000 -> "2.5M".
pub fn human_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_usage_by_request_and_tracks_context() {
        let dir = std::env::temp_dir().join(format!("claudash-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.jsonl");
        let usage = |i, cr, o| {
            format!(
                r#"{{"input_tokens":{i},"cache_creation_input_tokens":0,"cache_read_input_tokens":{cr},"output_tokens":{o}}}"#
            )
        };
        let lines = [
            r#"{"type":"user","cwd":"/tmp/proj","gitBranch":"main","message":{"content":"hola"}}"#
                .to_string(),
            // Same request written twice (two content blocks).
            format!(
                r#"{{"type":"assistant","requestId":"r1","message":{{"model":"m","usage":{}}}}}"#,
                usage(10, 100, 5)
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r1","message":{{"model":"m","usage":{}}}}}"#,
                usage(10, 100, 5)
            ),
            // Subagent: doesn't count.
            format!(
                r#"{{"type":"assistant","isSidechain":true,"requestId":"s1","message":{{"model":"m","usage":{}}}}}"#,
                usage(999, 0, 999)
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r2","message":{{"model":"m","usage":{}}}}}"#,
                usage(20, 300, 7)
            ),
            r#"{"type":"cost-state","totalCostUSD":1.5}"#.to_string(),
        ];
        fs::write(&file, lines.join("\n")).unwrap();

        let meta = fs::metadata(&file).unwrap();
        let s = parse_session(&file, "x", meta.modified().unwrap(), meta.len()).unwrap();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(s.git_branch.as_deref(), Some("main"));
        assert_eq!(s.tokens.total.input_tokens, 30);
        assert_eq!(s.tokens.total.cache_read_input_tokens, 400);
        assert_eq!(s.tokens.total.output_tokens, 12);
        assert_eq!(s.tokens.context_used, 320);
        assert_eq!(s.tokens.cost_usd, Some(1.5));
    }
}
