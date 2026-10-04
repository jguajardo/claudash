//! A deeper read of one session's transcript: how its context grew, which
//! tools it used and how often they failed, which files it edited, which
//! skills, subagents, MCP servers and commands it used, and its subagents.
//!
//! Like everything read from transcripts this depends on Claude Code's internal
//! format, so unknown records are skipped.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local, NaiveDate};
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
    /// Slash commands run, with their arguments (`opsx:apply add-login`).
    pub invocations: Vec<String>,
    /// Files read or written by the session itself.
    pub touched: BTreeSet<String>,
    /// Tool output that went into the context, per day and tool (Bash calls
    /// by command), subagents included.
    pub tool_output: BTreeMap<(NaiveDate, String), OutputStat>,
    /// What each of your prompts cost, in order.
    pub prompts: Vec<PromptCost>,
    /// Replies and their output tokens per day (main session).
    pub replies: BTreeMap<NaiveDate, (u32, u64)>,
    /// Risky things the session or its subagents did.
    pub audit: Vec<crate::audit::Event>,
    /// Credentials that appear in the transcript (masked).
    pub secrets: Vec<crate::audit::Secret>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutputStat {
    pub calls: u32,
    pub bytes: u64,
}

impl OutputStat {
    /// About four bytes per token.
    pub fn tokens(&self) -> u64 {
        self.bytes / 4
    }
}

/// One prompt and the usage of every request it caused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptCost {
    pub at: Option<DateTime<Local>>,
    pub text: String,
    pub usage: Usage,
    pub requests: u32,
}

/// Drops quoted strings and `$(…)` / backtick substitutions, so their words
/// aren't mistaken for commands.
fn scrub(command: &str) -> String {
    let mut out = String::new();
    let mut chars = command.chars().peekable();
    let mut depth = 0u32;
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '$') if chars.peek() == Some(&'(') => {
                chars.next();
                depth += 1;
            }
            (None, '(') if depth > 0 => depth += 1,
            (None, ')') if depth > 0 => depth -= 1,
            (None, _) if depth > 0 => {}
            (None, _) => out.push(c),
        }
    }
    out
}

/// `cd x && FOO=1 cargo test --all 2>&1 | tail` -> `cargo test`.
pub fn command_key(command: &str) -> String {
    let script = scrub(command);
    let segments = script
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|s| !s.is_empty());
    for segment in segments {
        let mut words = segment
            .split_whitespace()
            .filter(|w| !w.contains('='))
            .skip_while(|w| {
                matches!(
                    *w,
                    "sudo" | "env" | "time" | "timeout" | "nice" | "then" | "do"
                )
            })
            .skip_while(|w| w.chars().all(|c| c.is_ascii_digit() || c == 's'));
        let Some(program) = words.next() else {
            continue; // Only assignments.
        };
        if matches!(
            program,
            "cd" | "export"
                | ":"
                | "set"
                | "source"
                | "."
                | "done"
                | "fi"
                | "esac"
                | "{"
                | "}"
                | "("
                | ")"
        ) || program.starts_with('>')
            || program.starts_with('#')
        {
            continue;
        }
        if matches!(program, "for" | "while" | "until" | "if" | "case") {
            return "shell loop or condition".into();
        }
        let program = program.trim_start_matches('(');
        let program = program.rsplit('/').next().unwrap_or(program);
        let is_word = |w: &&str| {
            w.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':')
                && !w.starts_with('-')
        };
        let mut key = program.to_string();
        if let Some(sub) = words.next().filter(is_word) {
            key.push(' ');
            key.push_str(sub);
            if matches!(sub, "run" | "exec" | "x")
                && let Some(third) = words.next().filter(is_word)
            {
                key.push(' ');
                key.push_str(third);
            }
        }
        return key;
    }
    "other".into()
}

/// Label for a tool's output: Bash by command, others by tool name.
fn output_label(name: &str, input: &Value) -> String {
    match (name, input["command"].as_str()) {
        ("Bash", Some(command)) => format!("Bash: {}", command_key(command)),
        _ => name.to_string(),
    }
}

/// Text length of a tool result's content.
fn result_bytes(content: &Value) -> u64 {
    match content {
        Value::String(s) => s.len() as u64,
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| b["text"].as_str().map_or(0, |t| t.len() as u64))
            .sum(),
        _ => 0,
    }
}

/// The prompt you typed, from a user record, if it is one: a slash command
/// shows as `/name args`; tool results and Claude Code's own messages aren't.
fn prompt_text(record: &Value) -> Option<String> {
    if record["isMeta"].as_bool() == Some(true)
        || record["isCompactSummary"].as_bool() == Some(true)
    {
        return None;
    }
    let content = &record["message"]["content"];
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().find(|b| b["type"].as_str() == Some("text"))?["text"]
            .as_str()?
            .to_string(),
        _ => return None,
    };
    if let Some(rest) = text.split("<command-name>").nth(1) {
        let name = rest.split("</command-name>").next()?;
        let args = text
            .split("<command-args>")
            .nth(1)
            .and_then(|a| a.split("</command-args>").next())
            .unwrap_or_default();
        return Some(format!("{name} {}", args.trim()).trim_end().to_string());
    }
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted"))
        .then(|| text.to_string())
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
    let mut labels: HashMap<String, String> = HashMap::new();
    // Which prompt each request answered, and the day it was made.
    let mut request_prompt: HashMap<String, (Option<usize>, Option<NaiveDate>)> = HashMap::new();
    let mut current_prompt: Option<usize> = None;
    let mut calls = 0u32;
    let mut pending_compaction = false;
    let Ok(file) = fs::File::open(path) else {
        return (Usage::default(), 0);
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        crate::audit::find_secrets(&line, &mut out.secrets);
        let relevant = line.contains("\"type\":\"assistant\"")
            || line.contains("\"tool_result\"")
            || line.contains("compact_boundary")
            || line.contains("<command-name>")
            || (main && line.contains("\"type\":\"user\""));
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
                    serde_json::from_value::<crate::pricing::RawUsage>(message["usage"].clone())
                        .map(|u| u.priced(message["model"].as_str().unwrap_or_default())),
                ) {
                    let first_time = !usage_by_request.contains_key(req);
                    usage_by_request.insert(req.to_string(), usage);
                    if main && first_time {
                        request_prompt.insert(
                            req.to_string(),
                            (current_prompt, at.map(|t| t.date_naive())),
                        );
                    }
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
                        labels.insert(id.to_string(), output_label(&name, input));
                    }
                    crate::audit::check_tool(
                        &name,
                        input,
                        record["cwd"].as_str(),
                        at,
                        &mut out.audit,
                    );
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
                    if let Some(file) = input["file_path"]
                        .as_str()
                        .or(input["notebook_path"].as_str())
                    {
                        out.touched.insert(file.to_string());
                    }
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
                if let Some(day) = at.map(|t| t.date_naive()) {
                    for block in content.as_array().into_iter().flatten() {
                        if block["type"].as_str() != Some("tool_result") {
                            continue;
                        }
                        let id = block["tool_use_id"].as_str().unwrap_or_default();
                        if let Some(label) = labels.get(id) {
                            let stat = out.tool_output.entry((day, label.clone())).or_default();
                            stat.calls += 1;
                            stat.bytes += result_bytes(&block["content"]);
                        }
                    }
                }
                if main
                    && record["isSidechain"].as_bool() != Some(true)
                    && let Some(text) = prompt_text(&record)
                {
                    out.prompts.push(PromptCost {
                        at,
                        text: text.chars().take(200).collect(),
                        ..Default::default()
                    });
                    current_prompt = Some(out.prompts.len() - 1);
                }
                if let Some(text) = content.as_str()
                    && let Some(rest) = text.split("<command-name>").nth(1)
                {
                    let command = rest.split("</command-name>").next().unwrap_or_default();
                    if !command.is_empty() {
                        *out.commands.entry(command.to_string()).or_default() += 1;
                        if main {
                            let args = text
                                .split("<command-args>")
                                .nth(1)
                                .and_then(|a| a.split("</command-args>").next())
                                .unwrap_or_default()
                                .trim();
                            let name = command.trim_start_matches('/');
                            out.invocations
                                .push(format!("{name} {args}").trim_end().to_string());
                        }
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
    for (req, u) in &usage_by_request {
        total.add(u);
        if let Some((prompt, day)) = request_prompt.get(req) {
            if let Some(p) = prompt.and_then(|i| out.prompts.get_mut(i)) {
                p.usage.add(u);
                p.requests += 1;
            }
            if let Some(day) = day {
                let entry = out.replies.entry(*day).or_default();
                entry.0 += 1;
                entry.1 += u.output_tokens;
            }
        }
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
    fn measures_tool_output_and_what_each_prompt_cost() {
        let dir = std::env::temp_dir().join(format!("claudash-cost-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let t = "2026-01-01T10:00:00Z";
        let lines = [
            format!(
                r#"{{"type":"user","timestamp":"{t}","message":{{"content":"run the tests"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r1","timestamp":"{t}","message":{{"model":"m","usage":{{"input_tokens":10,"output_tokens":5}},"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"cargo test"}}}}]}}}}"#
            ),
            format!(
                r#"{{"type":"user","timestamp":"{t}","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"{}"}}]}}}}"#,
                "x".repeat(400)
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r2","timestamp":"{t}","message":{{"model":"m","usage":{{"input_tokens":20,"output_tokens":7}},"content":[]}}}}"#
            ),
            format!(
                r#"{{"type":"user","timestamp":"{t}","message":{{"content":"<command-name>/opsx:apply</command-name><command-args>add-login</command-args>"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","requestId":"r3","timestamp":"{t}","message":{{"model":"m","usage":{{"input_tokens":1,"output_tokens":1}},"content":[]}}}}"#
            ),
        ];
        fs::write(dir.join("s.jsonl"), lines.join("\n")).unwrap();
        let a = analyze(&dir.join("s.jsonl"));
        fs::remove_dir_all(&dir).unwrap();

        let day = DateTime::parse_from_rfc3339(t)
            .unwrap()
            .with_timezone(&Local)
            .date_naive();
        let out = a.tool_output[&(day, "Bash: cargo test".to_string())];
        assert_eq!((out.calls, out.tokens()), (1, 100));
        assert_eq!(a.prompts.len(), 2);
        assert_eq!(a.prompts[0].text, "run the tests");
        assert_eq!(
            (a.prompts[0].requests, a.prompts[0].usage.output_tokens),
            (2, 12)
        );
        assert_eq!(a.prompts[1].text, "/opsx:apply add-login");
        assert_eq!(a.invocations, ["opsx:apply add-login"]);
        assert_eq!(a.replies.values().next(), Some(&(3, 13)));
    }

    #[test]
    fn names_commands_by_what_they_run() {
        assert_eq!(
            command_key("cd /x && FOO=1 cargo test --all 2>&1 | tail"),
            "cargo test"
        );
        assert_eq!(command_key("npm run build"), "npm run build");
        assert_eq!(command_key("timeout 60 git diff --stat"), "git diff");
        assert_eq!(command_key("/usr/bin/ls -la"), "ls");
        assert_eq!(command_key("python3 script.py"), "python3");
        assert_eq!(
            command_key("SP=/tmp/x; B=$(ls | wc) && cd $SP && cargo build -q"),
            "cargo build"
        );
        assert_eq!(
            command_key("export A=1\nfor f in *; do echo $f; done"),
            "shell loop or condition"
        );
        assert_eq!(command_key("echo \"a | b && c\" | grep x"), "echo");
        assert_eq!(command_key("A=1"), "other");
    }

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
