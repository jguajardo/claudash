//! Every key claudash understands, in one table. The key handlers in `app`
//! follow it; the footer, the help view and the command palette are built
//! from it, so they can't drift apart.
//!
//! The rules the table keeps:
//! - A letter means the same thing in every view where it works.
//! - `Enter` opens or runs the selected thing, `Esc` goes back.
//! - Keys that remove, stop or restart something are uppercase and ask first.

use ratatui::crossterm::event::KeyCode;

/// Where a key works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Global,
    Sessions,
    /// The MCP server list beside the sessions.
    Mcp,
    Activity,
    /// The background session list in Activity.
    Background,
    Projects,
    /// The spec changes list in Projects.
    Specs,
    Logs,
    Usage,
    Ecosystem,
    Inspect,
    Conversation,
    Help,
}

impl Context {
    pub fn title(self) -> &'static str {
        match self {
            Context::Global => "Everywhere",
            Context::Sessions => "Sessions",
            Context::Mcp => "MCP servers",
            Context::Activity => "Activity",
            Context::Background => "Background",
            Context::Projects => "Projects",
            Context::Specs => "Specs",
            Context::Logs => "Logs",
            Context::Usage => "Usage",
            Context::Ecosystem => "Ecosystem",
            Context::Inspect => "Inspector",
            Context::Conversation => "Conversation",
            Context::Help => "Help",
        }
    }
}

pub struct Binding {
    pub context: Context,
    /// As shown to the user.
    pub keys: &'static str,
    pub what: &'static str,
    /// Short label in the footer, for the few keys worth seeing all the time.
    pub footer: Option<&'static str>,
    /// The key the command palette presses to run it; `None` for movement,
    /// going back and the palette itself.
    pub press: Option<KeyCode>,
}

const fn key(
    context: Context,
    keys: &'static str,
    what: &'static str,
    footer: Option<&'static str>,
    press: Option<KeyCode>,
) -> Binding {
    Binding {
        context,
        keys,
        what,
        footer,
        press,
    }
}

const fn ch(c: char) -> Option<KeyCode> {
    Some(KeyCode::Char(c))
}

const ENTER: Option<KeyCode> = Some(KeyCode::Enter);
const TAB: Option<KeyCode> = Some(KeyCode::Tab);

use Context::*;

pub const BINDINGS: &[Binding] = &[
    // Everywhere.
    key(Global, "1", "Sessions view", None, ch('1')),
    key(Global, "2", "Activity view", None, ch('2')),
    key(Global, "3", "Projects view", None, ch('3')),
    key(Global, "4", "Logs view", None, ch('4')),
    key(Global, "5", "Usage view", None, ch('5')),
    key(Global, "6", "Ecosystem view", None, ch('6')),
    key(
        Global,
        ":  Ctrl+P",
        "commands: find any action by name",
        Some("commands"),
        None,
    ),
    key(
        Global,
        "f",
        "search the text of every conversation",
        None,
        ch('f'),
    ),
    key(
        Global,
        "h",
        "prompt history: Enter sends one to the selected session, Tab copies it",
        None,
        ch('h'),
    ),
    key(
        Global,
        "s",
        "today's summary across projects",
        None,
        ch('s'),
    ),
    key(
        Global,
        "r",
        "reload sessions, re-check MCP servers and the ecosystem",
        None,
        ch('r'),
    ),
    key(Global, "?", "help", Some("help"), ch('?')),
    key(Global, "q  Ctrl+C", "quit", Some("quit"), ch('q')),
    // Sessions.
    key(Sessions, "↑/↓  j/k", "move", None, None),
    key(
        Sessions,
        "Enter",
        "resume it in Claude Code (claude --resume)",
        Some("resume"),
        ENTER,
    ),
    key(
        Sessions,
        "v",
        "read its conversation",
        Some("read"),
        ch('v'),
    ),
    key(
        Sessions,
        "i",
        "inspect it: context per request, tools and failures, files, subagents",
        Some("inspect"),
        ch('i'),
    ),
    key(
        Sessions,
        "p",
        "send it a one-off prompt (claude -p --resume)",
        Some("prompt"),
        ch('p'),
    ),
    key(
        Sessions,
        "/",
        "filter by title, path, branch, tag or note",
        Some("filter"),
        ch('/'),
    ),
    key(Sessions, "t", "tag it", None, ch('t')),
    key(Sessions, "c", "add a note to it", None, ch('c')),
    key(Sessions, "*", "star it", None, ch('*')),
    key(
        Sessions,
        "Tab",
        "move to its project's MCP servers",
        None,
        TAB,
    ),
    key(
        Sessions,
        "D",
        "move it to the trash (asks first)",
        None,
        ch('D'),
    ),
    key(
        Sessions,
        "C",
        "bulk cleanup: old or large sessions to the trash (asks first)",
        None,
        ch('C'),
    ),
    key(
        Sessions,
        "T",
        "open the trash: restore or delete for good",
        None,
        ch('T'),
    ),
    key(Sessions, "Esc", "clear the filter", None, None),
    // MCP servers.
    key(Mcp, "↑/↓", "move", None, None),
    key(
        Mcp,
        "Enter  l",
        "the server's latest log (why it failed)",
        Some("server log"),
        ENTER,
    ),
    key(
        Mcp,
        "a",
        "sign in to the server (claude mcp login; opens the browser)",
        Some("sign in"),
        ch('a'),
    ),
    key(
        Mcp,
        "L",
        "sign out of the server (claude mcp logout, asks first)",
        None,
        ch('L'),
    ),
    key(
        Mcp,
        "Tab  Esc",
        "back to the sessions",
        Some("sessions"),
        None,
    ),
    // Activity.
    key(Activity, "↑/↓", "move between open sessions", None, None),
    key(
        Activity,
        "Enter",
        "what a session that needs you is asking to do (the command, the diff)",
        Some("what it asks"),
        ENTER,
    ),
    key(
        Activity,
        "v",
        "read the selected session",
        Some("read"),
        ch('v'),
    ),
    key(
        Activity,
        "i",
        "inspect the selected session",
        Some("inspect"),
        ch('i'),
    ),
    key(
        Activity,
        "Tab",
        "move to the background sessions",
        Some("background"),
        TAB,
    ),
    // Background sessions.
    key(Background, "↑/↓", "move", None, None),
    key(
        Background,
        "Enter",
        "attach to it in this terminal (claude attach)",
        Some("attach"),
        ENTER,
    ),
    key(Background, "l", "its log", Some("log"), ch('l')),
    key(
        Background,
        "S",
        "stop it (claude stop, asks first)",
        Some("stop"),
        ch('S'),
    ),
    key(
        Background,
        "R",
        "respawn it (claude respawn, asks first)",
        Some("respawn"),
        ch('R'),
    ),
    key(
        Background,
        "Tab  Esc",
        "back to the open sessions",
        Some("open sessions"),
        None,
    ),
    // Projects.
    key(Projects, "↑/↓", "move", None, None),
    key(
        Projects,
        "Tab",
        "move to the project's spec changes (OpenSpec, spec-kit, Kiro, Task Master…)",
        Some("specs"),
        TAB,
    ),
    key(
        Projects,
        "Enter",
        "show that folder's sessions (Esc there shows all again)",
        Some("its sessions"),
        ENTER,
    ),
    key(
        Projects,
        "b",
        "review a branch: fetch, pick one, and Claude Code reviews it in a worktree of its own",
        Some("review branch"),
        ch('b'),
    ),
    key(
        Projects,
        "D",
        "remove the worktree (git worktree remove, asks first; git refuses if it has work)",
        Some("remove worktree"),
        ch('D'),
    ),
    key(
        Projects,
        "P",
        "prune records of worktrees whose directory is gone (asks first)",
        Some("prune"),
        ch('P'),
    ),
    // Spec changes.
    key(Specs, "↑/↓", "move between changes", None, None),
    key(
        Specs,
        "Enter",
        "run the next step with the framework's own command in Claude Code (e.g. /opsx:apply <id>)",
        Some("next step"),
        ENTER,
    ),
    key(
        Specs,
        "v",
        "read the change: proposal, design, tasks",
        Some("read"),
        ch('v'),
    ),
    key(
        Specs,
        "Tab  Esc",
        "back to the repositories",
        Some("repositories"),
        None,
    ),
    // Logs.
    key(Logs, "↑/↓", "choose a log", Some("log"), None),
    key(
        Logs,
        "/",
        "show only lines containing some text",
        Some("filter"),
        ch('/'),
    ),
    key(Logs, "x", "errors only", Some("errors only"), ch('x')),
    key(
        Logs,
        "PgUp/PgDn",
        "scroll (scrolling up stops following)",
        Some("scroll"),
        None,
    ),
    key(
        Logs,
        "End  G",
        "follow new lines",
        Some("follow"),
        Some(KeyCode::End),
    ),
    // Usage.
    key(Usage, "m", "days or months", Some("days/months"), ch('m')),
    key(
        Usage,
        "Tab",
        "where tokens go: tool output by command, costliest prompts, reply length",
        Some("where tokens go"),
        TAB,
    ),
    // Ecosystem.
    key(Ecosystem, "←/→  Tab", "switch tab", Some("tab"), TAB),
    key(Ecosystem, "↑/↓", "move", None, None),
    key(Ecosystem, "Enter", "details", Some("details"), ENTER),
    key(
        Ecosystem,
        "Space",
        "enable or disable the selected plugin (claude plugin enable/disable)",
        Some("toggle plugin"),
        Some(KeyCode::Char(' ')),
    ),
    // Inspector.
    key(Inspect, "↑/↓  PgUp/PgDn", "scroll", None, None),
    key(
        Inspect,
        "v",
        "read the session's conversation",
        Some("read"),
        ch('v'),
    ),
    key(
        Inspect,
        "Tab  Shift+Tab",
        "select the next or previous subagent",
        Some("subagent"),
        TAB,
    ),
    key(
        Inspect,
        "Enter",
        "read the selected subagent's conversation",
        Some("read subagent"),
        ENTER,
    ),
    key(Inspect, "Esc", "back", Some("back"), None),
    // Conversation.
    key(
        Conversation,
        "↑/↓  PgUp/PgDn",
        "scroll · g/G top/bottom",
        None,
        None,
    ),
    key(
        Conversation,
        "/",
        "search inside it",
        Some("search"),
        ch('/'),
    ),
    key(
        Conversation,
        "n/N",
        "next/previous match",
        Some("next/prev"),
        ch('n'),
    ),
    key(
        Conversation,
        "o",
        "show or hide tool output",
        Some("tool output"),
        ch('o'),
    ),
    key(
        Conversation,
        "e",
        "export to Markdown in ~/Documents/claudash-exports/",
        Some("export"),
        ch('e'),
    ),
    key(Conversation, "Esc", "back", Some("back"), None),
    // Help.
    key(Help, "↑/↓  PgUp/PgDn", "scroll", None, None),
    key(Help, "Esc", "back", Some("back"), None),
];

pub fn for_context(context: Context) -> impl Iterator<Item = &'static Binding> {
    BINDINGS.iter().filter(move |b| b.context == context)
}

/// Keys for the footer: the context's own, then the global ones.
pub fn footer(context: Context) -> Vec<&'static Binding> {
    for_context(context)
        .chain(for_context(Context::Global))
        .filter(|b| b.footer.is_some())
        .collect()
}

/// Commands the palette offers, ordered with `current` first. Each is run by
/// pressing its key in its context.
pub fn commands(current: Context, available: &[Context]) -> Vec<&'static Binding> {
    let mut order = vec![current, Context::Global];
    for context in available {
        if !order.contains(context) {
            order.push(*context);
        }
    }
    order
        .into_iter()
        .flat_map(for_context)
        .filter(|b| b.press.is_some())
        .collect()
}

/// Indices of the commands matching every word of `query`, case-insensitive.
pub fn search(commands: &[&Binding], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    commands
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let text = format!("{} {} {}", b.context.title(), b.what, b.keys).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A letter can't mean two things: every context plus the global keys
    /// must be free of duplicates.
    #[test]
    fn no_key_means_two_things() {
        let globals: Vec<KeyCode> = for_context(Global).filter_map(|b| b.press).collect();
        for context in [
            Sessions,
            Specs,
            Mcp,
            Activity,
            Background,
            Projects,
            Logs,
            Usage,
            Ecosystem,
            Inspect,
            Conversation,
        ] {
            let mut seen = globals.clone();
            for b in for_context(context) {
                if let Some(code) = b.press {
                    assert!(
                        !seen.contains(&code),
                        "{code:?} is bound twice in {context:?}"
                    );
                    seen.push(code);
                }
            }
        }
        // A letter used in several views does the same kind of thing.
        let meaning = |c: char| -> Vec<&str> {
            BINDINGS
                .iter()
                .filter(|b| b.press == Some(KeyCode::Char(c)))
                .map(|b| b.context.title())
                .collect()
        };
        assert_eq!(meaning('D'), ["Sessions", "Projects"]);
    }

    #[test]
    fn searches_commands_by_words() {
        let all = commands(Sessions, &[Projects, Logs]);
        let hits = search(&all, "remove work");
        assert_eq!(hits.len(), 1);
        assert_eq!(all[hits[0]].context, Projects);
        assert!(search(&all, "").len() == all.len());
        // The current context comes first.
        assert_eq!(all[0].context, Sessions);
    }

    #[test]
    fn footers_stay_short() {
        for context in [Sessions, Background, Logs, Inspect, Conversation] {
            assert!(footer(context).len() <= 9, "{context:?}");
        }
    }
}
