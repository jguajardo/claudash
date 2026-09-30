//! The Help view: what claudash is, whether it's connected to Claude Code, and
//! everything each view shows and lets you do.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    app::App,
    doctor::{self, Level},
    keys::{self, Context},
};

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(Color::LightMagenta).bold(),
    ))
}

fn para(text: &str) -> Line<'static> {
    Line::from(format!("  {text}"))
}

fn dim(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {text}"),
        Style::new().fg(Color::DarkGray),
    ))
}

/// A key and what it does.
fn key(keys: &str, what: &str) -> Line<'static> {
    keyw(keys, what, 14)
}

fn keyw(keys: &str, what: &str, width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("    {keys:<width$}"),
            Style::new().fg(Color::Yellow).bold(),
        ),
        Span::raw(what.to_string()),
    ])
}

/// One view: its name, what it shows, and where its keys come from.
struct ViewHelp {
    name: &'static str,
    shows: &'static str,
    contexts: &'static [Context],
}

const VIEWS: &[ViewHelp] = &[
    ViewHelp {
        name: "1 · Sessions",
        shows: "Every Claude Code session on this machine, newest first, with what's open right now \
                (▲ needs you, ● working, ● waiting; ⚠ when two open sessions share a folder). Beside \
                it, the selected session's project: the instruction files Claude Code loads there, its \
                git state, how many tokens a session there starts with, and its MCP servers' health. \
                Below, the session's context, tokens, cost, prompt cache and your plan usage with a \
                forecast.",
        contexts: &[Context::Sessions, Context::Mcp],
    },
    ViewHelp {
        name: "2 · Activity",
        shows: "What open sessions are doing right now, the ones that need you first: their state and \
                why they wait, their last tool call, how full their context is and how many subagents \
                are running. Below, a live feed of every tool call from sessions active in the last \
                hour, failures in red. Refreshes every 2 seconds while open. \
                Background sessions (claude --bg) are listed with their state, finished ones included.",
        contexts: &[Context::Activity, Context::Background],
    },
    ViewHelp {
        name: "3 · Projects",
        shows: "Every repository your sessions ran in, with all its checkouts (the main one and its \
                worktrees) and their git state, how many sessions each has and which are open. On \
                top, problems: open sessions sharing a folder, the same file edited by two open \
                sessions, worktrees with work but no session, and worktrees that no longer exist. \
                From here, b reviews a branch: claudash fetches, checks it out in a worktree of its \
                own and hands the review to Claude Code, then lists the findings by file and line.",
        contexts: &[Context::Projects],
    },
    ViewHelp {
        name: "4 · Logs",
        shows: "Logs in one place: every MCP server of the selected project (what Claude Code wrote \
                when it connected) and every background session's output. Follows new lines every 2 \
                seconds.",
        contexts: &[Context::Logs],
    },
    ViewHelp {
        name: "5 · Usage",
        shows: "Plan limits with a forecast, tokens per day or month, totals, usage by model and top \
                projects. Kept beyond Claude Code's 30-day cleanup.",
        contexts: &[Context::Usage],
    },
    ViewHelp {
        name: "6 · Ecosystem",
        shows: "Skills, subagents, commands, hooks and plugins available in the selected project, at \
                user, project, claude.ai and plugin scope, with how often each was used in the last 30 \
                days. Plugins show their always-on token cost per session, and the ones you never use \
                are flagged with what they cost you.",
        contexts: &[Context::Ecosystem],
    },
    ViewHelp {
        name: "Inspector (i)",
        shows: "One session in depth: its context per request with compactions, every tool with its \
                failure rate, its subagents by usage (running ones marked), the files it edited and the \
                skills, subagents, MCP servers and commands it used.",
        contexts: &[Context::Inspect],
    },
    ViewHelp {
        name: "Conversation (v)",
        shows: "The session's prompts and replies with their times, tool calls, compactions and, on \
                demand, tool output.",
        contexts: &[Context::Conversation],
    },
];

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let mut out = vec![
        heading("claudash — a control panel for Claude Code"),
        para(
            "claudash reads what Claude Code keeps on this machine and puts it on one screen, and",
        ),
        para(
            "lets you act on it: resume, read, search, tag, clean up, check MCP servers and plugins.",
        ),
        para("It never talks to Claude itself except when you ask (resume, prompt) and makes no"),
        para("network requests of its own. Claude Code does the work; claudash keeps track of it."),
        Line::default(),
        para("Press : (or Ctrl+P) anywhere to find any action by name and run it."),
        Line::default(),
        heading("Setup"),
    ];
    match &app.doctor {
        None => out.push(dim(&format!("{} checking…", app.spinner()))),
        Some(checks) => {
            for c in checks {
                let color = match c.level {
                    Level::Ok => Color::Green,
                    Level::Warn => Color::Yellow,
                    Level::Missing => Color::Red,
                    Level::Info => Color::DarkGray,
                };
                out.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", doctor::symbol(c.level)),
                        Style::new().fg(color).bold(),
                    ),
                    Span::styled(format!("{:<16}", c.name), Style::new().bold()),
                    Span::raw(c.detail.clone()),
                ]));
                if let Some(fix) = &c.fix {
                    out.push(Line::from(vec![
                        Span::raw(" ".repeat(20)),
                        Span::styled(format!("→ {fix}"), Style::new().fg(Color::Cyan)),
                    ]));
                }
            }
        }
    }
    out.push(Line::default());

    out.push(heading("Views"));
    for view in VIEWS {
        out.push(Line::from(Span::styled(
            format!("  {}", view.name),
            Style::new().fg(Color::Cyan).bold(),
        )));
        for chunk in crate::app::textwrap(view.shows, 90) {
            out.push(dim(&chunk));
        }
        for context in view.contexts {
            if view.contexts.len() > 1 && *context != view.contexts[0] {
                out.push(dim(&format!("{}:", context.title())));
            }
            for b in keys::for_context(*context) {
                out.push(key(b.keys, b.what));
            }
        }
        out.push(Line::default());
    }

    out.push(heading("Everywhere"));
    for b in keys::for_context(Context::Global) {
        out.push(key(b.keys, b.what));
    }
    out.push(dim(
        "Keys that remove, stop or restart something are uppercase and ask first.",
    ));
    out.push(Line::default());

    out.push(heading("What changes things"));
    for text in [
        "Resume and prompt run Claude Code on the session (a prompt adds to its conversation).",
        "Trash moves the session's files out of ~/.claude into claudash's trash; T restores them.",
        "Plugin toggles run `claude plugin enable/disable`.",
        "Background session actions run `claude attach`, `stop` and `respawn`.",
        "Worktree removal runs `git worktree remove` without --force; prune runs `git worktree prune`.",
        "Branch reviews run `git fetch`, check the branch out in a worktree under claudash's cache \
         (never in your checkout) and start Claude Code there. Nothing is posted anywhere.",
        "`claudash setup --apply` edits ~/.claude/settings.json after backing it up.",
        "Tags, notes, stars and the usage history are claudash's own files; Claude's are untouched.",
        "Resume, prompt and trash are refused while the session is open elsewhere.",
    ] {
        out.push(para(&format!("• {text}")));
    }
    out.push(Line::default());

    out.push(heading("Command line"));
    for (cmd, what) in [
        (
            "claudash",
            "open claudash (--view NAME, --no-notify, --context-limit 1M)",
        ),
        (
            "claudash status",
            "one line for tmux or other status bars (--json)",
        ),
        ("claudash summary", "today's summary as Markdown (-o FILE)"),
        (
            "claudash export <ID>",
            "a conversation as Markdown (-o FILE)",
        ),
        (
            "claudash setup",
            "show, --apply or --remove the status line and hooks",
        ),
        ("claudash doctor", "the Setup checks above"),
        (
            "claudash config",
            "the settings file and what's in effect (--init writes one)",
        ),
        (
            "claudash statusline",
            "status line command Claude Code runs",
        ),
        ("claudash hook", "hook command Claude Code runs"),
    ] {
        out.push(keyw(cmd, what, 24));
    }
    out
}
