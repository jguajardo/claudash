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
        name: "1 · Now",
        shows: "What needs you right now. Open sessions, the ones that need you first, with what \
                they're asking to do, their last tool call, how full their context is and how many \
                subagents are running; background sessions (claude --bg); alerts across projects \
                (sessions a plan limit stopped, which Enter continues once it resets; credentials in transcripts, risky commands, MCP servers that failed or need you to \
                sign in, stale specs, forgotten worktrees); plan usage with a forecast; and a live \
                feed of every tool call from the last hour. Tab moves into the alerts; Enter \
                takes you where to act on one.",
        contexts: &[Context::Now, Context::Background, Context::Alerts],
    },
    ViewHelp {
        name: "2 · Sessions",
        shows: "Every Claude Code session on this machine, newest first, with its state (▲ needs \
                you, ● working, ● waiting). Beside it, the selected session's context, tokens, cost, \
                prompt cache, and the instruction files and git state of its project.",
        contexts: &[Context::Sessions],
    },
    ViewHelp {
        name: "3 · Projects",
        shows: "Every project as a card: its branch and git state, sessions, open ones, spec \
                progress, MCP health and alerts. Enter opens its page, with a menu of sections: \
                Overview, Sessions, Specs (OpenSpec, spec-kit, Kiro, Task Master, GSD), Worktrees & \
                branches (with AI branch review), Snapshots (undo what /rewind can't), MCP servers \
                (logs, sign in), Skills, plugins & rules, and Security.",
        contexts: &[
            Context::Projects,
            Context::ProjectMenu,
            Context::ProjectSessions,
            Context::Specs,
            Context::Worktrees,
            Context::Snapshots,
            Context::Mcp,
            Context::Setup,
        ],
    },
    ViewHelp {
        name: "4 · Insights",
        shows: "Plan & usage: limits with a forecast, tokens or API-equivalent dollars ($) per day \
                or month, by model and project, \
                kept beyond Claude Code's 30-day cleanup. Where tokens go: tool output that entered \
                the context by command, the costliest prompts, reply length against the previous 30 \
                days. Where the limit went: each session's, project's and model's part of the \
                current 5-hour and 7-day windows, with the share of the limit when it's reported \
                (estimated by API-equivalent cost). Is your plan worth it: your use at API prices against \
                the plan's price, and how often Pro, Max 5x and Max 20x would have stopped you. \
                Security: credentials found in transcripts and risky things Claude did.",
        contexts: &[Context::Insights],
    },
    ViewHelp {
        name: "Inspector (i)",
        shows: "One session in depth: its context per request with compactions, every tool with its \
                failure rate, the tool output that entered its context, its costliest prompts, an \
                audit of risky actions and secrets, its subagents, the files it edited and the \
                skills, MCP servers and commands it used.",
        contexts: &[Context::Inspect],
    },
    ViewHelp {
        name: "Conversation (v)",
        shows: "The session's prompts and replies with their times, tool calls, compactions and, on \
                demand, tool output.",
        contexts: &[Context::Conversation],
    },
    ViewHelp {
        name: "Logs (l)",
        shows: "MCP server logs of the selected project and background session output, following \
                new lines.",
        contexts: &[Context::Logs],
    },
];

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let mut out = vec![
        heading("claudash — the control room for Claude Code"),
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
        "Plugin toggles run `claude plugin enable/disable`; MCP sign-in and sign-out run `claude mcp login/logout`.",
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
