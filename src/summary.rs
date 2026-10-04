//! "What happened today": for each project, what each session did (prompts,
//! tool calls and failures, files edited, tokens) and the commits made, as
//! Markdown. Built from local files only; Claude isn't called.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::{Local, NaiveDate};

use crate::{
    git,
    sessions::{Usage, human_tokens},
    transcript::{self, Kind},
    ui::plural,
};

/// What the summary needs from a session.
pub struct Input {
    pub title: String,
    pub project: String,
    pub cwd: Option<PathBuf>,
    pub path: PathBuf,
    pub tokens_today: Usage,
}

struct SessionDay {
    title: String,
    prompts: usize,
    tools: usize,
    failed: usize,
    files: BTreeSet<String>,
    tokens: u64,
    cost: f64,
}

/// The sessions that used tokens on `day`.
pub fn inputs(sessions: &[crate::sessions::Session], day: NaiveDate) -> Vec<Input> {
    sessions
        .iter()
        .filter(|s| s.tokens.daily.contains_key(&day))
        .map(|s| {
            let mut tokens = Usage::default();
            for usage in s.tokens.daily[&day].values() {
                tokens.add(usage);
            }
            Input {
                title: s.title.clone(),
                project: s.project_path.clone(),
                cwd: s.cwd.clone(),
                path: s.path.clone(),
                tokens_today: tokens,
            }
        })
        .collect()
}

/// Markdown for `day`. Reads each session's transcript and runs git per
/// project folder: call it off the UI thread.
pub fn build(inputs: Vec<Input>, day: NaiveDate) -> String {
    let mut projects: BTreeMap<String, (Option<PathBuf>, Vec<SessionDay>)> = BTreeMap::new();
    for input in inputs {
        let entries = transcript::load(&input.path).unwrap_or_default();
        let today: Vec<&transcript::Entry> = entries
            .iter()
            .filter(|e| e.at.is_some_and(|t| t.date_naive() == day))
            .collect();
        let mut s = SessionDay {
            title: input.title,
            prompts: today.iter().filter(|e| e.kind == Kind::User).count(),
            tools: 0,
            failed: 0,
            files: BTreeSet::new(),
            tokens: input.tokens_today.processed(),
            cost: input.tokens_today.cost,
        };
        for e in &today {
            match &e.kind {
                Kind::ToolUse { name } => {
                    s.tools += 1;
                    if matches!(
                        name.as_str(),
                        "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                    ) {
                        s.files.insert(e.text.clone());
                    }
                }
                Kind::ToolResult { is_error: true } => s.failed += 1,
                _ => {}
            }
        }
        let entry = projects
            .entry(input.project)
            .or_insert((input.cwd, Vec::new()));
        entry.1.push(s);
    }

    let (mut sessions, mut prompts, mut tools, mut tokens, mut cost) = (0, 0, 0, 0, 0.0);
    for (_, list) in projects.values() {
        for s in list {
            sessions += 1;
            prompts += s.prompts;
            tools += s.tools;
            tokens += s.tokens;
            cost += s.cost;
        }
    }
    let mut md = format!(
        "# Claude Code · {}\n\n**{} · {} · {} · {} tokens · ≈{} API-equivalent**\n\n",
        day.format("%A, %B %-d %Y"),
        plural(sessions as u64, "session"),
        plural(prompts as u64, "prompt"),
        plural(tools as u64, "tool call"),
        human_tokens(tokens),
        crate::pricing::format_usd(cost)
    );
    if sessions == 0 {
        md.push_str("No Claude Code activity on this day.\n");
        return md;
    }
    let mut seen_repos = BTreeSet::new();
    for (project, (cwd, list)) in projects {
        md.push_str(&format!("## {project}\n\n"));
        for s in list {
            md.push_str(&format!(
                "- **{}**: {}, {}{}, {} edited, {} tokens, ≈{}\n",
                s.title,
                plural(s.prompts as u64, "prompt"),
                plural(s.tools as u64, "tool call"),
                if s.failed > 0 {
                    format!(" ({} failed)", s.failed)
                } else {
                    String::new()
                },
                plural(s.files.len() as u64, "file"),
                human_tokens(s.tokens),
                crate::pricing::format_usd(s.cost)
            ));
            if !s.files.is_empty() {
                let names: Vec<String> = s
                    .files
                    .iter()
                    .take(8)
                    .map(|f| format!("`{}`", f.rsplit(['/', '\\']).next().unwrap_or(f)))
                    .collect();
                let more = s.files.len().saturating_sub(8);
                md.push_str(&format!(
                    "  - files: {}{}\n",
                    names.join(", "),
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                ));
            }
        }
        // Commits once per repository, only for today's summary.
        if day == Local::now().date_naive()
            && let Some(dir) = cwd.filter(|d| d.is_dir())
        {
            let repo = git::status(&dir).map(|s| s.common_dir);
            if repo.as_ref().is_none_or(|r| seen_repos.insert(r.clone())) {
                let commits = git::commits_today(&dir);
                if !commits.is_empty() {
                    md.push_str("\nCommits today:\n\n");
                    for c in commits {
                        md.push_str(&format!("- `{c}`\n"));
                    }
                }
            }
        }
        md.push('\n');
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn summarizes_a_day() {
        let dir = std::env::temp_dir().join(format!("claudash-summary-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-01-02T10:00:00Z","message":{"content":"Fix it"}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-02T10:00:05Z","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/p/src/a.rs"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-02T10:00:06Z","message":{"content":[{"type":"tool_result","is_error":true,"content":"no"}]}}"#,
            r#"{"type":"user","timestamp":"2025-12-31T10:00:00Z","message":{"content":"old prompt"}}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();
        let day = transcript::load(&file).unwrap()[0].at.unwrap().date_naive();
        let md = build(
            vec![Input {
                title: "Fix login".into(),
                project: "~/p".into(),
                cwd: None,
                path: file,
                tokens_today: Usage {
                    output_tokens: 1500,
                    cost: 0.0375,
                    ..Default::default()
                },
            }],
            day,
        );
        fs::remove_dir_all(&dir).unwrap();
        assert!(md.contains(
            "**1 session · 1 prompt · 1 tool call · 1.5k tokens · ≈$0.04 API-equivalent**"
        ));
        assert!(md.contains("## ~/p"));
        assert!(md.contains("- **Fix login**: 1 prompt, 1 tool call (1 failed), 1 file edited"));
        assert!(md.contains("`a.rs`"));
    }
}
