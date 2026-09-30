//! The Help view: what claudash is, whether it's connected to Claude Code, and
//! everything each view shows and lets you do.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    app::App,
    doctor::{self, Level},
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

/// One view: its name, what it shows, and its keys.
struct ViewHelp {
    name: &'static str,
    shows: &'static str,
    keys: &'static [(&'static str, &'static str)],
}

const VIEWS: &[ViewHelp] = &[
    ViewHelp {
        name: "1 · Dashboard",
        shows: "Every Claude Code session on this machine, newest first, with what's open right now \
                (▲ needs you, ● working, ● waiting; ⚠ when two open sessions share a folder). Beside \
                it, the selected session's project: the instruction files Claude Code loads there, its \
                git state, how many tokens a session there starts with, and its MCP servers' health. \
                Below, the session's context, tokens, cost, prompt cache and your plan usage with a \
                forecast.",
        keys: &[
            ("↑/↓  j/k", "move between sessions"),
            (
                "Enter",
                "resume the session in Claude Code (claude --resume)",
            ),
            ("v", "read its conversation"),
            (
                "i",
                "inspect it: context per request, tools and failures, files, subagents",
            ),
            ("p", "send it a one-off prompt (claude -p --resume)"),
            ("/", "filter by title, path, branch, tag or note"),
            ("f", "search the text of every conversation"),
            ("t · n · *", "tag · add a note · star (kept by claudash)"),
            ("d", "move it to the trash (asks first)"),
            ("T · C", "open the trash · bulk cleanup"),
            (
                "Tab",
                "move to the MCP list; Enter there shows the server's log",
            ),
        ],
    },
    ViewHelp {
        name: "Conversation (v)",
        shows: "The session's prompts and replies with their times, tool calls, compactions and, on \
                demand, tool output.",
        keys: &[
            ("↑/↓ PgUp/PgDn", "scroll · g/G top/bottom"),
            ("o", "show or hide tool output"),
            ("/ · n/N", "search inside · next/previous match"),
            ("r", "reload (follow a running session)"),
            ("e", "export to Markdown in ~/Documents/claudash-exports/"),
            ("Esc", "back"),
        ],
    },
    ViewHelp {
        name: "2 · Projects",
        shows: "Every repository your sessions ran in, with all its checkouts (the main one and its \
                worktrees) and their git state, how many sessions each has and which are open. On \
                top, problems: open sessions sharing a folder, the same file edited by two open \
                sessions, worktrees with work but no session, and worktrees that no longer exist.",
        keys: &[
            ("↑/↓", "move"),
            (
                "Enter",
                "show that folder's sessions in the Dashboard (Esc there shows all again)",
            ),
        ],
    },
    ViewHelp {
        name: "4 · Ecosystem",
        shows: "Skills, subagents, commands, hooks and plugins available in the selected project, at \
                user, project, claude.ai and plugin scope, with how often each was used in the last 30 \
                days. Plugins show their always-on token cost per session, and the ones you never use \
                are flagged with what they cost you.",
        keys: &[
            ("←/→  Tab", "switch tab"),
            ("Enter", "details"),
            (
                "Space",
                "enable or disable the selected plugin (claude plugin enable/disable)",
            ),
        ],
    },
    ViewHelp {
        name: "5 · Usage",
        shows: "Plan limits with a forecast, tokens per day or month, totals, usage by model and top \
                projects. Kept beyond Claude Code's 30-day cleanup.",
        keys: &[("m", "days or months")],
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
        for (k, what) in view.keys {
            out.push(key(k, what));
        }
        out.push(Line::default());
    }

    out.push(heading("Everywhere"));
    out.push(key("1 2 4 5", "switch view"));
    out.push(key(
        "r",
        "reload sessions, re-check MCP servers and the ecosystem",
    ));
    out.push(key("?", "this help"));
    out.push(key("q  Ctrl+C", "quit"));
    out.push(Line::default());

    out.push(heading("What changes things"));
    for text in [
        "Resume and prompt run Claude Code on the session (a prompt adds to its conversation).",
        "Trash moves the session's files out of ~/.claude into claudash's trash; T restores them.",
        "Plugin toggles run `claude plugin enable/disable`.",
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
            "open the dashboard (--no-notify, --context-limit 1M)",
        ),
        (
            "claudash setup",
            "show, --apply or --remove the status line and hooks",
        ),
        ("claudash doctor", "the Setup checks above"),
        (
            "claudash export <ID>",
            "a conversation as Markdown (-o FILE)",
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
