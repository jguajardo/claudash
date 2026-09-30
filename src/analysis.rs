//! A deeper read of one session's transcript: how its context grew, which
//! tools it used and how often they failed, which files it edited, which
//! skills, subagents, MCP servers and commands it used, and its subagents.
//!
//! Like everything read from transcripts this depends on Claude Code's internal
//! format, so unknown records are skipped.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::sessions::Usage;

/// Recent tool calls kept per session, for the activity feed.
const RECENT_EVENTS: usize = 40;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolStat {
    pub calls: u32,
    pub errors: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextPoint {
    pub at: Option<DateTime<Local>>,
    pub tokens: u64,
    /// The conversation was compacted just before this request.
    pub after_compaction: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub at: Option<DateTime<Local>>,
    pub tool: String,
    pub summary: String,
    pub failed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subagent {
    pub agent_type: String,
    pub description: String,
    pub model: String,
    pub background: bool,
    pub usage: Usage,
    pub tool_calls: u32,
    pub file: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub context: Vec<ContextPoint>,
    pub tools: BTreeMap<String, ToolStat>,
    /// Files written by Edit, Write or NotebookEdit, with the time of the last edit.
    pub edits: BTreeMap<String, Option<DateTime<Local>>>,
    /// Uses per skill, subagent type, MCP server and slash command,
    /// subagents' own calls included.
    pub skills: HashMap<String, u32>,
    pub agents: HashMap<String, u32>,
    pub mcp_servers: HashMap<String, u32>,
    pub commands: HashMap<String, u32>,
    pub subagents: Vec<Subagent>,
    /// The latest tool calls, oldest first.
    pub recent: Vec<Event>,
}

impl Analysis {
    /// Context of the first request: what a session carries before any work.
    pub fn first_context(&self) -> Option<u64> {
        self.context.first().map(|p| p.tokens)
    }
}

fn local_time(record: &Value) -> Option<DateTime<Local>> {
    record["timestamp"]
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Local))
}

/// Keeps the most useful input of a tool call on one line.
fn summarize(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "skill",
        "description",
    ] {
        if let Some(v) = input[key].as_str() {
            let line = v.lines().next().unwrap_or_default();
            return line.chars().take(120).collect();
        }
    }
    String::new()
}

/// Reads one transcript file into `out`. `main` is false for subagent files,
/// which only add to the usage counts.
fn read_file(path: &Path, main: bool, out: &mut Analysis) -> (Usage, u32) {
    let mut usage_by_request: HashMap<String, Usage> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut calls = 0u32;
    let mut pending_compaction = false;
    let Ok(file) = fs::File::open(path) else {
        return (Usage::default(), 0);
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let relevant = line.contains("\"type\":\"assistant\"")
            || line.contains("\"tool_result\"")
            || line.contains("compact_boundary")
            || line.contains("<command-name>");
        if !relevant {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let at = local_time(&record);
        match record["type"].as_str() {
            Some("assistant") => {
                if main && record["isSidechain"].as_bool() == Some(true) {
                    continue;
                }
                let message = &record["message"];
                if message["model"].as_str() == Some("<synthetic>") {
                    continue;
                }
                if let (Some(req), Ok(usage)) = (
                    record["requestId"].as_str(),
                    serde_json::from_value::<Usage>(message["usage"].clone()),
                ) {
                    let first_time = !usage_by_request.contains_key(req);
                    usage_by_request.insert(req.to_string(), usage);
                    if main && first_time {
                        out.context.push(ContextPoint {
                            at,
                            tokens: usage.context(),
                            after_compaction: std::mem::take(&mut pending_compaction),
                        });
                    } else if main && let Some(last) = out.context.last_mut() {
                        // Same request written again: keep its final size.
                        last.tokens = usage.context();
                    }
                }
                for block in message["content"].as_array().into_iter().flatten() {
                    if block["type"].as_str() != Some("tool_use") {
                        continue;
                    }
                    let name = block["name"].as_str().unwrap_or("tool").to_string();
                    let input = &block["input"];
                    calls += 1;
                    if let Some(id) = block["id"].as_str() {
                        tool_names.insert(id.to_string(), name.clone());
                    }
                    match name.as_str() {
                        "Skill" => {
                            if let Some(skill) =
                                input["skill"].as_str().or(input["command"].as_str())
                            {
                                *out.skills.entry(skill.to_string()).or_default() += 1;
                            }
                        }
                        "Agent" | "Task" => {
                            let kind = input["subagent_type"].as_str().unwrap_or("general-purpose");
                            *out.agents.entry(kind.to_string()).or_default() += 1;
                        }
                        _ => {}
                    }
                    if let Some(server) = name
                        .strip_prefix("mcp__")
                        .and_then(|r| r.split("__").next())
                    {
                        *out.mcp_servers.entry(server.to_string()).or_default() += 1;
                    }
                    if !main {
                        continue;
                    }
                    out.tools.entry(name.clone()).or_default().calls += 1;
                    if matches!(
                        name.as_str(),
                        "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                    ) && let Some(file) = input["file_path"]
                        .as_str()
                        .or(input["notebook_path"].as_str())
                    {
                        out.edits.insert(file.to_string(), at);
                    }
                    out.recent.push(Event {
                        at,
                        tool: name,
                        summary: summarize(input),
                        failed: false,
                    });
                    if out.recent.len() > RECENT_EVENTS {
                        out.recent.remove(0);
                    }
                }
            }
            Some("user") => {
                let content = &record["message"]["content"];
                if let Some(text) = content.as_str()
                    && let Some(rest) = text.split("<command-name>").nth(1)
                {
                    let command = rest.split("</command-name>").next().unwrap_or_default();
                    if !command.is_empty() {
                        *out.commands.entry(command.to_string()).or_default() += 1;
                    }
                }
                if !main {
                    continue;
                }
                for block in content.as_array().into_iter().flatten() {
                    if block["type"].as_str() != Some("tool_result")
                        || block["is_error"].as_bool() != Some(true)
                    {
                        continue;
                    }
                    let id = block["tool_use_id"].as_str().unwrap_or_default();
                    if let Some(name) = tool_names.get(id) {
                        out.tools.entry(name.clone()).or_default().errors += 1;
                        if let Some(event) = out.recent.iter_mut().rev().find(|e| &e.tool == name) {
                            event.failed = true;
                        }
                    }
                }
            }
            Some("system") if record["subtype"].as_str() == Some("compact_boundary") => {
                pending_compaction = true;
            }
            _ => {}
        }
    }
    let mut total = Usage::default();
    for u in usage_by_request.values() {
        total.add(u);
    }
    (total, calls)
}

/// Analyzes a session transcript and its subagents.
pub fn analyze(path: &Path) -> Analysis {
    let mut out = Analysis::default();
    read_file(path, true, &mut out);
    let dir = path.with_extension("").join("subagents");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    for file in files {
        let (usage, tool_calls) = read_file(&file, false, &mut out);
        let meta: Value = fs::read(file.with_extension("meta.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or(Value::Null);
        out.subagents.push(Subagent {
            agent_type: meta["agentType"].as_str().unwrap_or("subagent").to_string(),
            description: meta["description"].as_str().unwrap_or_default().to_string(),
            model: meta["model"].as_str().unwrap_or_default().to_string(),
            background: meta["requestShape"].as_str() == Some("background"),
            usage,
            tool_calls,
            file,
        });
    }
    // Biggest first: the ones worth a look.
    out.subagents
        .sort_by_key(|s| std::cmp::Reverse(s.usage.processed()));
    out
}

/// Analyses keyed by transcript path, reused while the file is unchanged.
pub type Cache = HashMap<PathBuf, (SystemTime, u64, std::sync::Arc<Analysis>)>;

/// Analyzes every given session, reusing `previous` for unchanged files.
pub fn analyze_all(sessions: &[(PathBuf, SystemTime, u64)], previous: &Cache) -> Cache {
    sessions
        .iter()
        .map(|(path, modified, size)| {
            let analysis = match previous.get(path) {
                Some((m, s, a)) if m == modified && s == size => a.clone(),
                _ => std::sync::Arc::new(analyze(path)),
            };
            (path.clone(), (*modified, *size, analysis))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_context_tools_edits_and_subagents() {
        let dir = std::env::temp_dir().join(format!("claudash-analysis-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("s/subagents")).unwrap();
        let usage = |ctx| format!(r#"{{"input_tokens":{ctx},"output_tokens":1}}"#);
        let lines = [
            format!(r#"{{"type":"assistant","requestId":"r1","timestamp":"2026-01-01T10:00:00Z","message":{{"model":"m","usage":{},"content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{"file_path":"/p/a.rs"}}}}]}}}}"#, usage(100)),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"no"}]}}"#.to_string(),
            r#"{"type":"system","subtype":"compact_boundary"}"#.to_string(),
            format!(r#"{{"type":"assistant","requestId":"r2","message":{{"model":"m","usage":{},"content":[{{"type":"tool_use","id":"t2","name":"Skill","input":{{"skill":"superpowers:brainstorming"}}}},{{"type":"tool_use","id":"t3","name":"mcp__playwright__browser_click","input":{{}}}}]}}}}"#, usage(40)),
            r#"{"type":"user","message":{"content":"<command-name>/compact</command-name>"}}"#.to_string(),
        ];
        fs::write(dir.join("s.jsonl"), lines.join("\n")).unwrap();
        fs::write(
            dir.join("s/subagents/agent-1.jsonl"),
            format!(r#"{{"type":"assistant","isSidechain":true,"requestId":"x","message":{{"model":"m","usage":{},"content":[{{"type":"tool_use","id":"t9","name":"Agent","input":{{"subagent_type":"code-reviewer"}}}}]}}}}"#, usage(7)),
        )
        .unwrap();
        fs::write(
            dir.join("s/subagents/agent-1.meta.json"),
            r#"{"agentType":"general-purpose","description":"Review","model":"opus","requestShape":"background"}"#,
        )
        .unwrap();

        let a = analyze(&dir.join("s.jsonl"));
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            a.context.iter().map(|p| p.tokens).collect::<Vec<_>>(),
            [100, 40]
        );
        assert!(a.context[1].after_compaction);
        assert_eq!(a.first_context(), Some(100));
        assert_eq!(
            a.tools["Edit"],
            ToolStat {
                calls: 1,
                errors: 1
            }
        );
        assert!(a.edits.contains_key("/p/a.rs"));
        assert_eq!(a.skills["superpowers:brainstorming"], 1);
        assert_eq!(a.mcp_servers["playwright"], 1);
        assert_eq!(a.commands["/compact"], 1);
        // The subagent's own calls count toward usage, not the main tool stats.
        assert_eq!(a.agents["code-reviewer"], 1);
        assert!(!a.tools.contains_key("Agent"));
        assert_eq!(a.subagents.len(), 1);
        assert!(a.subagents[0].background);
        assert_eq!(a.subagents[0].usage.input_tokens, 7);
        assert!(a.recent.iter().any(|e| e.tool == "Edit" && e.failed));
    }
}
