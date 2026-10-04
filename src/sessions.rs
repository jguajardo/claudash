//! Reads the real Claude Code sessions from `~/.claude/projects`.
//!
//! On-disk layout: `~/.claude/projects/<encoded-path>/<session-id>.jsonl`,
//! where each line of the `.jsonl` is a JSON record. The directory name
//! replaces `/` with `-` (and can't be reversed reliably), so the real project
//! path is taken from the records' `cwd` field.
//!
//! Token usage comes from `message.usage` of each assistant response. A single
//! response is written over several lines (one per content block) with the
//! same `requestId`, so usage is deduplicated by that field. Subagents write
//! their own transcripts to `<session-id>/subagents/*.jsonl`; they count toward
//! daily usage but not toward the main conversation's context.
//!
//! Claude Code documents this format as internal, so parsing is lenient: lines
//! that don't match are skipped rather than treated as errors.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

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
    /// Transcript size in bytes.
    pub size: u64,
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
    /// Usage per local calendar day and model, subagents included.
    pub daily: Daily,
    /// Subagent transcripts, and their combined usage.
    pub subagents: usize,
    pub subagent_total: Usage,
    /// Every response with its time (epoch seconds), subagents included,
    /// oldest first: what a plan window's usage is attributed from.
    pub timeline: Vec<(i64, Usage)>,
    /// The session's last reply was a plan-limit error: it stopped there.
    pub limit_stop: Option<LimitStop>,
    /// Every time a plan limit stopped the main conversation, oldest first.
    pub limit_hits: Vec<LimitStop>,
}

/// Where a session stopped because it hit a plan limit.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitStop {
    /// When it stopped (epoch seconds).
    pub at: i64,
    /// When the limit resets (epoch seconds), as the API reported it.
    pub resets_at: i64,
    /// "five_hour", "seven_day" or another window name.
    pub window: String,
}

impl LimitStop {
    /// "5-hour", "7-day" or the API's name for the window.
    pub fn window_label(&self) -> String {
        match self.window.as_str() {
            "five_hour" => "5-hour".into(),
            "seven_day" | "seven_day_opus" | "seven_day_sonnet" => "7-day".into(),
            other => other.replace('_', " "),
        }
    }
}

/// Usage per local calendar day, then per model.
pub type Daily = BTreeMap<NaiveDate, BTreeMap<String, Usage>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// API-equivalent cost in USD (see `pricing`).
    #[serde(default)]
    pub cost: f64,
}

impl Usage {
    /// Everything that takes up context in a request.
    pub fn context(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    /// Tokens actually processed, leaving out cache reads (which dwarf the rest).
    pub fn processed(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.output_tokens
    }

    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost += other.cost;
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
    timestamp: Option<String>,
    /// "rate_limit" on the reply Claude Code writes when a plan limit stops it.
    error: Option<String>,
    #[serde(rename = "quotaLimits")]
    quota_limits: Option<QuotaLimits>,
}

#[derive(Deserialize)]
struct QuotaLimits {
    #[serde(rename = "resetsAt")]
    resets_at: Option<i64>,
    #[serde(rename = "rateLimitType", default)]
    rate_limit_type: String,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<crate::pricing::RawUsage>,
}

impl Message {
    /// The response's usage, priced, unless it was generated locally
    /// ("<synthetic>" messages make no API call).
    fn usage(&self) -> Option<Usage> {
        let model = self.model.as_deref().unwrap_or_default();
        if model == "<synthetic>" {
            return None;
        }
        self.usage.as_ref().map(|u| u.priced(model))
    }
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
    // Main conversation only.
    let mut usage_by_request: HashMap<String, RequestUsage> = HashMap::new();
    let mut last_usage: Option<Usage> = None;
    let mut model = None;
    let mut cost_usd = None;
    let mut limit_stop = None;
    let mut limit_hits = Vec::new();

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
                // Any later reply means it went on after the limit.
                limit_stop = None;
                if record.error.as_deref() == Some("rate_limit")
                    && let Some(quota) = &record.quota_limits
                    && let Some(resets_at) = quota.resets_at
                {
                    limit_stop = Some(LimitStop {
                        at: record
                            .timestamp
                            .as_deref()
                            .and_then(parse_time)
                            .map_or(resets_at, |t| t.timestamp()),
                        resets_at,
                        window: quota.rate_limit_type.clone(),
                    });
                    limit_hits.extend(limit_stop.clone());
                }
                if let Some(msg) = record.message
                    && let Some(usage) = msg.usage()
                {
                    if let Some(req) = record.request_id {
                        let at = record.timestamp.as_deref().and_then(parse_time);
                        let model_name = msg.model.clone().unwrap_or_default();
                        usage_by_request.insert(req, (usage, at, model_name));
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
    for (usage, _, _) in usage_by_request.values() {
        total.add(usage);
    }
    // Daily usage adds the subagents, deduplicated against the main file.
    let (sub_requests, subagents) = subagent_usage(&path.with_extension(""));
    let mut subagent_total = Usage::default();
    let mut all_requests = usage_by_request;
    for (req, entry) in sub_requests {
        if let std::collections::hash_map::Entry::Vacant(slot) = all_requests.entry(req) {
            subagent_total.add(&entry.0);
            slot.insert(entry);
        }
    }
    let mut daily = Daily::new();
    let mut timeline = Vec::with_capacity(all_requests.len());
    for (usage, at, model) in all_requests.values() {
        if let Some(at) = at {
            daily
                .entry(at.date_naive())
                .or_default()
                .entry(model.clone())
                .or_default()
                .add(usage);
            timeline.push((at.timestamp(), *usage));
        }
    }
    timeline.sort_by_key(|(at, _)| *at);
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
        project_path: crate::paths::display(Path::new(&project_path)),
        cwd: cwd.map(PathBuf::from),
        git_branch,
        modified,
        size,
        tokens: SessionTokens {
            total,
            context_used,
            model,
            cost_usd,
            daily,
            subagents,
            subagent_total,
            timeline,
            limit_stop,
            limit_hits,
        },
    })
}

/// One response's usage, its local time and its model.
type RequestUsage = (Usage, Option<DateTime<Local>>, String);

/// Usage of every response in `<session-dir>/subagents/*.jsonl`, by
/// requestId, and how many subagent transcripts there are.
fn subagent_usage(session_dir: &Path) -> (HashMap<String, RequestUsage>, usize) {
    let mut usage = HashMap::new();
    let mut files = 0;
    let Ok(entries) = fs::read_dir(session_dir.join("subagents")) else {
        return (usage, files);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        files += 1;
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"type\":\"assistant\"") {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(&line) else {
                continue;
            };
            if let (Some(req), Some(msg)) = (record.request_id, record.message)
                && let Some(u) = msg.usage()
            {
                let at = record.timestamp.as_deref().and_then(parse_time);
                usage.insert(req, (u, at, msg.model.unwrap_or_default()));
            }
        }
    }
    (usage, files)
}

/// Local time of an RFC 3339 timestamp such as `2026-09-29T23:25:05.989Z`.
fn parse_time(timestamp: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|t| t.with_timezone(&Local))
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
    fn notices_a_session_that_stopped_at_a_plan_limit() {
        let dir = std::env::temp_dir().join(format!("claudash-limit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lim.jsonl");
        let reply = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-10-04T07:50:00Z","message":{"model":"m","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let stop = r#"{"type":"assistant","timestamp":"2026-10-04T07:56:57.594Z","error":"rate_limit","isApiErrorMessage":true,"quotaLimits":{"status":"rejected","resetsAt":1791111600,"rateLimitType":"five_hour"},"message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your session limit · resets 1pm"}]}}"#;
        let parse = |lines: &[&str]| {
            fs::write(&file, lines.join("\n")).unwrap();
            let meta = fs::metadata(&file).unwrap();
            parse_session(&file, "x", meta.modified().unwrap(), meta.len()).unwrap()
        };
        let stopped = parse(&[reply, stop]).tokens.limit_stop.unwrap();
        assert_eq!(stopped.resets_at, 1_791_111_600);
        assert_eq!(stopped.at, 1_791_100_617);
        assert_eq!(stopped.window_label(), "5-hour");
        // It went on later: not stopped any more.
        let went_on = parse(&[stop, reply]).tokens;
        assert_eq!(went_on.limit_stop, None);
        assert_eq!(went_on.limit_hits.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

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
                r#"{{"type":"assistant","requestId":"r1","timestamp":"2026-01-02T12:00:00Z","message":{{"model":"m","usage":{}}}}}"#,
                usage(10, 100, 5)
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r1","timestamp":"2026-01-02T12:00:00Z","message":{{"model":"m","usage":{}}}}}"#,
                usage(10, 100, 5)
            ),
            // Subagent: doesn't count.
            format!(
                r#"{{"type":"assistant","isSidechain":true,"requestId":"s1","message":{{"model":"m","usage":{}}}}}"#,
                usage(999, 0, 999)
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r2","timestamp":"2026-01-02T12:00:00Z","message":{{"model":"m","usage":{}}}}}"#,
                usage(20, 300, 7)
            ),
            r#"{"type":"cost-state","totalCostUSD":1.5}"#.to_string(),
        ];
        fs::write(&file, lines.join("\n")).unwrap();
        // A subagent transcript: counts toward daily usage only.
        fs::create_dir_all(dir.join("abc/subagents")).unwrap();
        fs::write(
            dir.join("abc/subagents/agent-1.jsonl"),
            format!(
                r#"{{"type":"assistant","isSidechain":true,"requestId":"s2","timestamp":"2026-01-02T12:00:00Z","message":{{"model":"m","usage":{}}}}}"#,
                usage(1, 0, 2)
            ),
        )
        .unwrap();

        let meta = fs::metadata(&file).unwrap();
        let s = parse_session(&file, "x", meta.modified().unwrap(), meta.len()).unwrap();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(s.git_branch.as_deref(), Some("main"));
        assert_eq!(s.tokens.total.input_tokens, 30);
        assert_eq!(s.tokens.total.cache_read_input_tokens, 400);
        assert_eq!(s.tokens.total.output_tokens, 12);
        assert_eq!(s.tokens.context_used, 320);
        assert_eq!(s.tokens.cost_usd, Some(1.5));

        let day = parse_time("2026-01-02T12:00:00Z").unwrap().date_naive();
        let daily = s.tokens.daily[&day]["m"];
        assert_eq!(daily.input_tokens, 31);
        assert_eq!(daily.output_tokens, 14);
        assert_eq!(s.tokens.subagents, 1);
        assert_eq!(s.tokens.subagent_total.output_tokens, 2);
    }
}
