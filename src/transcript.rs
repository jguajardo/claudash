//! Reads a session transcript as a conversation: prompts, replies, tool calls
//! and their results, and compaction points. Also renders it as Markdown.
//!
//! The transcript format is internal to Claude Code
//! (<https://code.claude.com/docs/en/sessions#where-transcripts-are-stored>),
//! so parsing is lenient: unknown records and blocks are skipped.

use std::{
    fs,
    io::{self, BufRead, BufReader},
    path::Path,
};

use chrono::{DateTime, Local};
use serde_json::Value;

/// Longest tool result kept per call; the rest is summarized as "… N more lines".
const MAX_RESULT_LINES: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// What you typed.
    User,
    /// Claude's reply text.
    Assistant,
    /// Claude reasoned here (the content isn't stored in readable form).
    Thinking,
    /// A tool call: the tool name and a one-line summary of its input.
    ToolUse { name: String },
    /// The output of a tool call.
    ToolResult { is_error: bool },
    /// The conversation was compacted; tokens before compaction if known.
    Compaction { pre_tokens: Option<u64> },
    /// The summary Claude Code wrote when compacting.
    CompactSummary,
    /// A notice from Claude Code (recaps, warnings).
    Notice,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    pub at: Option<DateTime<Local>>,
    pub text: String,
}

pub fn load(path: &Path) -> io::Result<Vec<Entry>> {
    let file = fs::File::open(path)?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(record) = serde_json::from_str::<Value>(&line) {
            parse_record(&record, &mut entries);
        }
    }
    Ok(entries)
}

fn parse_record(record: &Value, out: &mut Vec<Entry>) {
    // Subagent turns live in their own files; meta messages are Claude Code's
    // own injections (skill bodies, command wrappers).
    if record["isSidechain"].as_bool() == Some(true) || record["isMeta"].as_bool() == Some(true) {
        return;
    }
    let at = record["timestamp"]
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Local));
    let push = |out: &mut Vec<Entry>, kind: Kind, text: String| out.push(Entry { kind, at, text });

    match record["type"].as_str() {
        Some("user") => {
            let content = &record["message"]["content"];
            if record["isCompactSummary"].as_bool() == Some(true) {
                push(out, Kind::CompactSummary, plain_text(content));
                return;
            }
            match content {
                Value::String(text) => {
                    if let Some(text) = user_text(text) {
                        push(out, Kind::User, text);
                    }
                }
                Value::Array(blocks) => {
                    for block in blocks {
                        match block["type"].as_str() {
                            Some("text") => {
                                if let Some(text) = block["text"].as_str().and_then(user_text) {
                                    push(out, Kind::User, text);
                                }
                            }
                            Some("image") => push(out, Kind::User, "[image]".into()),
                            Some("tool_result") => {
                                let is_error = block["is_error"].as_bool() == Some(true);
                                push(
                                    out,
                                    Kind::ToolResult { is_error },
                                    plain_text(&block["content"]),
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Some("assistant") => {
            for block in record["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
            {
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default().trim();
                        if !text.is_empty() {
                            push(out, Kind::Assistant, text.to_string());
                        }
                    }
                    Some("thinking") | Some("redacted_thinking") => {
                        // Consecutive thinking blocks read as one.
                        if out.last().map(|e| &e.kind) != Some(&Kind::Thinking) {
                            push(out, Kind::Thinking, String::new());
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool").to_string();
                        push(out, Kind::ToolUse { name }, tool_summary(&block["input"]));
                    }
                    _ => {}
                }
            }
        }
        Some("system") => match record["subtype"].as_str() {
            Some("compact_boundary") => {
                let pre_tokens = record["compactMetadata"]["preTokens"].as_u64();
                push(out, Kind::Compaction { pre_tokens }, String::new());
            }
            Some("away_summary") | Some("model_refusal_fallback") => {
                push(out, Kind::Notice, plain_text(&record["content"]));
            }
            _ => {}
        },
        _ => {}
    }
}

/// A user message worth showing: slash commands are shown as the command,
/// Claude Code's own wrappers (command output, reminders) are dropped.
fn user_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix("<command-name>") {
        let command = rest.split("</command-name>").next().unwrap_or_default();
        let args = text
            .split("<command-args>")
            .nth(1)
            .and_then(|a| a.split("</command-args>").next())
            .unwrap_or_default()
            .trim();
        return Some(format!("{command} {args}").trim().to_string());
    }
    if text.starts_with("<local-command-")
        || text.starts_with("<system-reminder>")
        || text.starts_with("<command-message>")
        || text.starts_with("[Request interrupted")
    {
        return None;
    }
    Some(text.to_string())
}

/// Text of a string or an array of text blocks.
fn plain_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().map(str::to_owned),
                Some("image") => Some("[image]".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// One line describing what a tool call does, from its most telling input.
fn tool_summary(input: &Value) -> String {
    const KEYS: [&str; 10] = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "skill",
        "description",
        "prompt",
        "subject",
    ];
    let value = KEYS
        .iter()
        .find_map(|k| input[*k].as_str())
        .map(str::to_owned)
        .or_else(|| {
            input
                .as_object()
                .filter(|o| !o.is_empty())
                .map(|_| input.to_string())
        })
        .unwrap_or_default();
    let first_line = value.lines().next().unwrap_or_default();
    truncate(first_line, 140)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Tool results can be huge; keep the first lines.
pub fn clip_lines(text: &str) -> (Vec<&str>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let extra = lines.len().saturating_sub(MAX_RESULT_LINES);
    (lines.into_iter().take(MAX_RESULT_LINES).collect(), extra)
}

/// A Markdown document for the conversation.
pub fn to_markdown(title: &str, meta: &[(&str, String)], entries: &[Entry]) -> String {
    let mut md = format!("# {title}\n\n");
    for (key, value) in meta {
        md.push_str(&format!("- **{key}:** {value}\n"));
    }
    md.push('\n');
    let time = |e: &Entry| {
        e.at.map(|t| format!(" · {}", t.format("%Y-%m-%d %H:%M")))
            .unwrap_or_default()
    };
    for entry in entries {
        match &entry.kind {
            Kind::User => md.push_str(&format!("## You{}\n\n{}\n\n", time(entry), entry.text)),
            Kind::Assistant => {
                md.push_str(&format!("## Claude{}\n\n{}\n\n", time(entry), entry.text))
            }
            Kind::Thinking => {}
            Kind::ToolUse { name } => md.push_str(&format!(
                "> **{name}** `{}`\n\n",
                entry.text.replace('`', "'")
            )),
            Kind::ToolResult { is_error } => {
                if entry.text.is_empty() {
                    continue;
                }
                let (lines, extra) = clip_lines(&entry.text);
                let label = if *is_error { "Error" } else { "Output" };
                md.push_str(&format!("<details><summary>{label}</summary>\n\n```\n"));
                for line in lines {
                    // Keep a result from closing the fence early.
                    md.push_str(&line.replace("```", "'''"));
                    md.push('\n');
                }
                if extra > 0 {
                    md.push_str(&format!("… {extra} more lines\n"));
                }
                md.push_str("```\n\n</details>\n\n");
            }
            Kind::Compaction { pre_tokens } => {
                let size = pre_tokens
                    .map(|t| format!(" ({} tokens before)", crate::sessions::human_tokens(t)))
                    .unwrap_or_default();
                md.push_str(&format!(
                    "---\n\n*Conversation compacted{size}.*\n\n---\n\n"
                ));
            }
            Kind::CompactSummary => md.push_str(&format!(
                "<details><summary>Compaction summary</summary>\n\n{}\n\n</details>\n\n",
                entry.text
            )),
            Kind::Notice => md.push_str(&format!("> *{}*\n\n", entry.text)),
        }
    }
    md
}

/// Text that search looks at: prompts, replies, tool calls and tool output.
pub fn searchable(entry: &Entry) -> Option<&str> {
    match entry.kind {
        Kind::Thinking | Kind::Compaction { .. } => None,
        _ => Some(entry.text.as_str()),
    }
}

/// A search match in some session's conversation.
#[derive(Clone, Debug)]
pub struct Hit {
    pub session_id: String,
    pub title: String,
    /// Index into that session's entries.
    pub entry: usize,
    pub snippet: String,
    pub at: Option<DateTime<Local>>,
}

/// Most matches reported per search.
const MAX_HITS: usize = 300;

/// Case-insensitive search through every given transcript, newest first.
pub fn search(sessions: &[(String, String, std::path::PathBuf)], query: &str) -> Vec<Hit> {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for (id, title, path) in sessions {
        let Ok(entries) = load(path) else {
            continue;
        };
        for (i, entry) in entries.iter().enumerate() {
            let Some(text) = searchable(entry) else {
                continue;
            };
            if let Some(snippet) = snippet(text, &needle) {
                hits.push(Hit {
                    session_id: id.clone(),
                    title: title.clone(),
                    entry: i,
                    snippet,
                    at: entry.at,
                });
                if hits.len() >= MAX_HITS {
                    return hits;
                }
            }
        }
    }
    hits
}

/// The matching line, trimmed around the match to about 90 characters.
pub fn snippet(text: &str, needle_lower: &str) -> Option<String> {
    if needle_lower.is_empty() {
        return None;
    }
    let line = text
        .lines()
        .find(|l| l.to_lowercase().contains(needle_lower))?;
    let chars: Vec<char> = line.trim().chars().collect();
    let lower: Vec<char> = line.trim().to_lowercase().chars().collect();
    // Position in characters (lowercasing can change byte lengths).
    let needle: Vec<char> = needle_lower.chars().collect();
    let pos = lower
        .windows(needle.len().max(1))
        .position(|w| w == needle.as_slice())
        .unwrap_or(0)
        .min(chars.len());
    let start = pos.saturating_sub(30);
    let end = (start + 90).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(records: &[Value]) -> Vec<Entry> {
        let mut out = Vec::new();
        for r in records {
            parse_record(r, &mut out);
        }
        out
    }

    #[test]
    fn reads_a_conversation() {
        let entries = parse(&[
            json!({"type":"user","timestamp":"2026-01-02T10:00:00Z","message":{"content":"Fix the tests"}}),
            json!({"type":"user","isMeta":true,"message":{"content":"Base directory for this skill"}}),
            json!({"type":"user","message":{"content":"<command-name>/compact</command-name><command-args>keep tests</command-args>"}}),
            json!({"type":"user","message":{"content":"<local-command-stdout>ok</local-command-stdout>"}}),
            json!({"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":""},
                {"type":"thinking","thinking":""},
                {"type":"text","text":"Running them."},
                {"type":"tool_use","name":"Bash","input":{"command":"cargo test\n--quiet","description":"Run tests"}}
            ]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","content":"1 failed","is_error":true}]}}),
            json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"preTokens":412530}}),
            json!({"type":"user","isCompactSummary":true,"message":{"content":"Summary of earlier work"}}),
            json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}),
            json!({"type":"system","subtype":"turn_duration"}),
        ]);
        let kinds: Vec<&Kind> = entries.iter().map(|e| &e.kind).collect();
        assert_eq!(
            kinds,
            [
                &Kind::User,
                &Kind::User,
                &Kind::Thinking,
                &Kind::Assistant,
                &Kind::ToolUse {
                    name: "Bash".into()
                },
                &Kind::ToolResult { is_error: true },
                &Kind::Compaction {
                    pre_tokens: Some(412530)
                },
                &Kind::CompactSummary,
            ]
        );
        assert_eq!(entries[1].text, "/compact keep tests");
        assert_eq!(entries[4].text, "cargo test");
        assert!(entries[0].at.is_some());
    }

    #[test]
    fn markdown_keeps_fences_intact_and_clips_output() {
        let long: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let entries = vec![
            Entry {
                kind: Kind::User,
                at: None,
                text: "hello".into(),
            },
            Entry {
                kind: Kind::ToolResult { is_error: false },
                at: None,
                text: format!("```\n{long}"),
            },
        ];
        let md = to_markdown("Title", &[("Project", "~/app".into())], &entries);
        assert!(md.starts_with("# Title\n\n- **Project:** ~/app\n"));
        assert!(md.contains("## You\n\nhello"));
        assert!(md.contains("'''"));
        assert!(md.contains("… 11 more lines"));
        assert_eq!(md.matches("```").count(), 2);
    }

    #[test]
    fn snippets_center_on_the_match() {
        let text = format!(
            "first line\n{}needle here{}",
            "a".repeat(60),
            "b".repeat(60)
        );
        let snip = snippet(&text, "needle").unwrap();
        assert!(snip.starts_with('…') && snip.ends_with('…'));
        assert!(snip.contains("needle here"));
        assert_eq!(snippet("Ångström Ok", "ok").unwrap(), "Ångström Ok");
        assert!(snippet("nothing", "zzz").is_none());
    }
}
