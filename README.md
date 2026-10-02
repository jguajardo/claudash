# claudash

A terminal dashboard for [Claude Code](https://code.claude.com/docs): browse, search and resume your sessions, see which instructions and MCP servers each project gets, track context and plan usage, and manage your skills, subagents, hooks and plugins, all from one screen.

> **Unofficial project.** claudash is not affiliated with, endorsed by or supported by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic.

![claudash demo: sessions that need you, an MCP server log, live activity, a session inspector, the ecosystem and usage views, prompt history and a daily summary](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/claudash.gif)

<sub>Recorded from made-up data with [VHS](https://github.com/charmbracelet/vhs); see [demo/](demo/).</sub>

## Features

Six views, switched with `1`–`6`. Press `:` (or `Ctrl+P`) anywhere to find any action by name, and `?` for everything each view shows and does.

### 1 · Sessions

- **Sessions**: every Claude Code session on your machine, newest first, with its title, project folder, git branch and last activity. Sessions open right now in Claude Code are marked `▲ needs you` (waiting for a permission decision or other input), `● working` or `● waiting` (finished, waiting for your next prompt), and claudash sends a desktop notification and rings the terminal bell when a session starts needing you or finishes. The list refreshes every 5 seconds.
  - `Enter` resumes the session with `claude --resume` in its project folder and returns to claudash when you exit.
  - `v` opens the conversation: your prompts and Claude's replies with their times, tool calls, compaction points and, with `o`, tool output. `/` searches inside it, `n`/`N` jump between matches, `r` reloads it to follow a running session, and `e` exports it to Markdown in `~/Documents/claudash-exports/`.
  - `i` inspects the session: a chart of its context per request with compactions, every tool it called with its failure rate, the files it edited, its subagents (type, model, tokens) and the skills, subagents, MCP servers and commands it used. `Tab` selects a subagent and `Enter` opens its own conversation.
  - `p` sends a one-off prompt to the session (`claude -p --resume`) and shows the reply.
  - `/` filters by title, path, branch, tag or note (`#tag` works too).
  - `t` tags a session, `c` adds a note and `*` stars it. They're kept by claudash, not in Claude Code's files.
  - `D` moves the session to claudash's trash, after asking; `T` opens the trash to restore sessions or delete them for good, and anything left there is deleted after 30 days. `C` cleans up in bulk: pick a criterion (not used for 7/14/21 days, larger than 5/20 MB), check the list and move them all to the trash. Open and starred sessions are never included.
- **Project**: the instruction files Claude Code loads for the selected session's folder: managed, user and project `CLAUDE.md`, `CLAUDE.local.md` and `AGENTS.md`. It follows the [documented precedence rules](https://code.claude.com/docs/en/memory#agents-md) and your *Project instructions* setting, and files that are present but ignored (such as an `AGENTS.md` next to a `CLAUDE.md`) are crossed out with the reason.
- **Project** also shows the folder's git state (branch, changed files, commits to push or pull), how many tokens sessions there start with before your first prompt, and a warning when two open sessions share the folder, since their edits can collide.
- **MCP Status**: `claude mcp list` run in the selected project's folder, so project-scoped servers show up too, cached per project. Press `Tab` to move into the list and `Enter` to read a server's latest log, which usually says why it failed. For servers that need you to sign in (`AUTH`), such as claude.ai connectors (Gmail, Google Calendar, Atlassian…) or HTTP servers, `a` runs `claude mcp login` and re-checks the server when you're back; `L` signs out (`claude mcp logout`) after asking.
- **Token Usage**: how full the selected session's context window is, its input, cache and output tokens, cost, model, and how many subagents it ran and what they used.
  - **Cache diagnostics**: how much of the input was read from cache, flagged when cache writes dominate; with the status line, whether the cache is still warm and until when, its TTL, cache misses and their cause (such as `tools_changed` or `ttl_expired_5m`), and how many tokens the next reply re-caches once it goes cold.
  - **Plan usage**: 5-hour and 7-day limits with their reset times and a forecast of when you'll hit the limit at your current pace. claudash alerts you at 80% and 95%.

From any view:

- `f` searches the content of every conversation and opens the one you pick at the match.
- `h` searches every prompt you've typed in Claude Code, newest first. `Enter` puts the chosen one in the prompt box for the selected session, `Tab` copies it to the clipboard (OSC 52, works over SSH).
- `s` summarizes today across projects: sessions, prompts, tool calls and failures, files edited, tokens and each repository's commits. `e` exports it to Markdown in `~/Documents/claudash-exports/`.

### 2 · Activity

What open sessions are doing right now, the ones that need you first: their state and why they wait (for example "permission prompt"), what exactly a session that needs you is asking to do (the command it wants to run, the edit it wants to make; `Enter` shows all of it), their last tool call, how full their context is and how many subagents are running. Below, a live feed of every tool call from sessions active in the last hour, with failures in red. It refreshes every 2 seconds while it's open, re-reading only the transcripts that changed.

Background sessions (`claude --bg`, [agent view](https://code.claude.com/docs/en/agent-view)) are listed too, with their state, including finished ones. `Tab` moves to them: `Enter` attaches to one in this terminal (`claude attach`), `l` shows its output, and `S` stops it and `R` respawns it, both after asking. To dispatch new ones, use Claude Code's own `claude agents`.

### 3 · Projects

Every repository your sessions ran in, with all its checkouts (the main one and its worktrees) and their git state: branch, changed files, commits to push or pull, how many sessions each has and which are open. `Enter` shows a folder's sessions in the Sessions view.

On top, a **Problems** panel collects what can go wrong with several sessions at once:

- two open sessions working in the same folder, whose edits can collide;
- the same file edited by two open sessions in the last day;
- worktrees with uncommitted or unpushed work that no open session is using;
- worktrees whose directory is gone.

`D` removes the selected worktree with `git worktree remove` (never `--force`, so git refuses when it holds uncommitted changes), and `P` prunes the records of worktrees whose directory is gone. Both ask first.

#### Specs

If a project uses a spec-driven development framework, its changes show under the selected folder, with a progress bar of their tasks, their stage (planning, implementing, complete), the sessions that worked on them and what those cost in tokens. `Tab` moves into the list:

- `Enter` runs the next step in Claude Code with the framework's own command, such as `/opsx:apply add-login` for OpenSpec or `/speckit-plan` for spec-kit. claudash only offers a command your project actually has, so it never invents one.
- `v` shows the change's files: proposal, requirements, design, plan, tasks.

| Framework | What claudash reads | Next step |
| --- | --- | --- |
| [OpenSpec](https://github.com/Fission-AI/OpenSpec) | `openspec/changes/<id>/` (archived ones left out) | `/opsx:apply`, `/opsx:archive`, `/opsx:update` |
| [spec-kit](https://github.com/github/spec-kit) | `specs/NNN-name/` | `/speckit-plan`, `/speckit-tasks`, `/speckit-implement` (on the feature's own branch) |
| Kiro and [cc-sdd](https://github.com/gotalab/cc-sdd) | `.kiro/specs/<feature>/` | `/kiro-spec-requirements`, `-design`, `-tasks`, `/kiro-impl` |
| [spec-workflow-mcp](https://github.com/Pimzino/spec-workflow-mcp) | `.spec-workflow/specs/<feature>/tasks.md` | driven over MCP |
| [Task Master](https://github.com/eyaltoledano/claude-task-master) | `.taskmaster/tasks/tasks.json`, per tag | driven over MCP |
| [GSD](https://github.com/open-gsd/gsd-core) | `.planning/ROADMAP.md` | `/gsd-progress` |
| BMAD, Agent OS | detected only | |

The Problems panel also flags changes with tasks left that nobody touched for 14 days, and changes whose tasks are all done, with the command to wrap them up.

#### Review a branch

`b` on a repository reviews one of its branches the way you'd review a pull request, with Claude Code doing the reviewing:

1. claudash runs `git fetch` and lists the remote's branches, newest first, with their last commit, author, how many commits they add and whether you've reviewed them. Type to search. Your own checkout's branch comes first when it has commits the base doesn't, so you can review your work before pushing it (only committed work is reviewed).
2. Pick one: it shows its size against the base (`develop`, or `main`, or `master`, whichever the remote has first) and asks how far to go:
   - **Static review**: Claude Code reads the commits, the diff and the code around it, in the background. It can only read files and run `git log`, `git diff` and `git show`; nothing runs and nothing changes.
   - **Review and run the tests** or **Review and start the project**: Claude Code opens in the terminal, reviews, then finds how to run the tests or start the project and does it, asking you before each command. The findings show when you exit.
3. The branch is checked out, detached, in a worktree under claudash's cache (`~/.cache/claudash/reviews/`), so your checkout, your branch and your `git status` are never touched.
4. The findings list each comment by file, line and severity, with a suggested change where it helps. `Enter` shows one with the code around that line, `Tab` copies it to paste into GitHub or Bitbucket, `e` exports the whole review to Markdown, `v` opens the review's Claude Code session to ask follow-up questions, and `D` removes the worktree. claudash never posts anything.

Comments are written in the language of the branch's commit messages. A review uses your plan like any other Claude Code session.

### 4 · Logs

Logs in one place: every MCP server of the selected project and every background session's output. It follows new lines every 2 seconds; scrolling up with `PgUp` pauses that and `End` resumes it. `/` filters lines, `x` shows errors only.

### 5 · Usage

Plan usage gauges with the forecast, a chart of tokens per day (or per month with `m`), totals for today, 7 days, 30 days and a year, usage by model, and your most active projects this week. Subagents are included.

Claude Code deletes transcripts after 30 days by default, so claudash keeps its own copy of the daily numbers: the charts keep your history after the transcripts are gone.

### 6 · Ecosystem

Everything Claude Code has available for the selected project, in five tabs: **Skills**, **Agents** (subagents), **Commands**, **Hooks** and **Plugins**. It covers user scope, project scope, claude.ai-synced skills and every enabled plugin, with how often each skill, subagent and command was used in the last 30 days. Plugins show their projected always-on token cost per session from `claude plugin details` and how often anything they provide was used, so plugins that cost tokens in every session without being used stand out, with their total. `Space` enables or disables the selected plugin with `claude plugin enable/disable`.

### From the command line and status bars

```sh
claudash status            # ▲ 1 needs you · 2 working · 1 waiting · 5h 64% · 7d 31%
claudash status --json     # the same for scripts
claudash summary           # today's summary as Markdown (-o FILE to save it)
claudash export 6b1f3c2e   # a conversation as Markdown, by session ID or its first characters
```

`claudash status` takes about 0.2 seconds, so a status bar can poll it. For tmux:

```sh
set -g status-right '#(claudash status)'
set -g status-interval 15
```

## Installation

Requires [Claude Code](https://code.claude.com/docs) on your `PATH`: claudash runs the `claude` command for MCP checks, plugins, resuming sessions and prompts.

### Prebuilt binaries

Prebuilt binaries for Linux, macOS and Windows are on the [Releases page](https://github.com/jguajardo/claudash/releases), along with a one-line installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jguajardo/claudash/releases/latest/download/claudash-installer.sh | sh
```

### With Cargo

```sh
cargo install claudash          # builds from source
cargo binstall claudash         # downloads a prebuilt binary (needs cargo-binstall)
```

### From source

```sh
git clone https://github.com/jguajardo/claudash
cd claudash
cargo install --path .
```

## Setup: alerts, plan usage and the real context window

Some data is only available from Claude Code while a session runs: plan usage and the real context window size come through the [status line](https://code.claude.com/docs/en/statusline), and knowing that a session is waiting for your permission needs [hooks](https://code.claude.com/docs/en/hooks). One command connects both:

```sh
claudash setup           # shows what it would change in ~/.claude/settings.json
claudash setup --apply   # makes the changes, after backing the file up
claudash setup --remove  # undoes them
```

It registers `claudash statusline` as the status line and `claudash hook` as an asynchronous hook for `UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop` and `SessionEnd`. Running it twice changes nothing, and your own hooks are left alone. If you already have a status line, claudash wraps it and prints that command's output, so it looks the same as before; `--remove` restores it.

- `claudash statusline` saves what Claude Code reports and, on its own, prints a compact line (`Opus · ctx 23% · 5h 41% · 7d 12% · $6.41`).
- `claudash hook` records only the event type and time for each session. It never stores your prompts or Claude's replies, and prints nothing.

Claude Code only sends plan usage to Pro and Max subscribers, after the first reply of a session. Without the setup everything else works: sessions show `working`/`waiting` from `claude agents --json`, and the context gauge uses `--context-limit`.

## Usage

```sh
claudash                       # open claudash
claudash --view activity       # open on another view
claudash --no-notify           # no desktop notifications or bell
claudash --context-limit 200k  # context window to assume when the status line hasn't reported one
claudash setup                 # connect the status line and hooks (see above)
claudash doctor                # check claudash's connection to Claude Code
claudash config                # where the settings file is and what's in effect
claudash --help
```

### Keys

A letter means the same thing in every view where it works, `Enter` opens or runs the selected thing and `Esc` goes back. Keys that remove, stop or restart something are uppercase and ask first. The footer shows the keys of the view you're in, `:` finds any action by name and `?` lists them all.

| Key | Where | Action |
| --- | --- | --- |
| `1`–`6` | everywhere | Sessions, Activity, Projects, Logs, Usage, Ecosystem |
| `:` `Ctrl+P` | everywhere | Commands: find any action by name and run it |
| `f` · `h` · `s` | everywhere | Search every conversation · prompt history · today's summary |
| `r` · `?` · `q` | everywhere | Reload · help · quit |
| `↑` `↓` / `j` `k` | lists | Move |
| `Enter` | | Resume the session · attach to a background session · show a folder's sessions · open a log or details |
| `Esc` | | Back · clear the filter |
| `/` | | Filter sessions · search the conversation · filter log lines |
| `v` · `i` | sessions | Read the conversation · inspect it |
| `p` | sessions | Send a one-off prompt |
| `t` · `c` · `*` | sessions | Tag · note · star |
| `Tab` | | Next pane: MCP servers, background sessions, subagents in the inspector, ecosystem tabs |
| `l` | MCP servers, background | Log |
| `a` · `L` | MCP servers | Sign in (`claude mcp login`) · sign out (asks first) |
| `o` · `n`/`N` · `e` | conversation | Tool output · next/previous match · export to Markdown |
| `x` | logs | Errors only |
| `m` | usage | Days or months |
| `Space` | ecosystem | Enable or disable the plugin |
| `D` | sessions, projects | Move the session to the trash · remove the worktree |
| `C` · `T` | sessions | Bulk cleanup · open the trash |
| `S` · `R` | background | Stop · respawn |
| `P` | projects | Prune missing worktrees |
| `b` | projects | Review a branch with Claude Code |

### Configuration

Settings go in `~/.config/claudash/config.toml` (`~/Library/Application Support/claudash/config.toml` on macOS); `claudash config --init` writes one with every setting commented out. Flags win over environment variables, which win over the file.

| Setting | File | Flag | Environment variable | Default |
| --- | --- | --- | --- | --- |
| View to open on | `view = "activity"` | `--view <NAME>` | | `sessions` |
| Context window when the status line hasn't reported one | `context_limit = "200k"` | `--context-limit <TOKENS>` | `CLAUDASH_CONTEXT_LIMIT` | `1M` |
| Desktop notifications and bell | `notify = false` | `--no-notify` | | on |
| Claude Code's config directory | | | `CLAUDE_CONFIG_DIR` | `~/.claude` |

Token values accept suffixes: `1M`, `200k`, `1.5m`, `500000`.

## What claudash reads and changes

claudash makes no network requests of its own. It reads local files and runs the `claude` CLI.

| Data | Source | Kind |
| --- | --- | --- |
| Plan usage, context window size, prompt cache | [Status line](https://code.claude.com/docs/en/statusline) JSON saved by `claudash statusline` to `~/.cache/claudash/statusline/` | Documented |
| Needs you / working / waiting | [Hook](https://code.claude.com/docs/en/hooks) events saved by `claudash hook` to `~/.cache/claudash/state/` | Documented |
| Open sessions | `claude agents --json` | Documented |
| MCP servers | `claude mcp list`, run in the project folder. Only names and statuses are shown, never commands or URLs, which may contain credentials | Documented |
| Plugins | `claude plugin list --json`, `claude plugin details` | Documented |
| Instruction files | The [memory docs](https://code.claude.com/docs/en/memory) rules; the *Project instructions* setting from user and managed settings | Documented |
| Skills, agents, commands, hooks | `SKILL.md` and Markdown frontmatter, `hooks` in settings files and plugins' `hooks/hooks.json` | Documented |
| Sessions, conversations, tokens, cost | `~/.claude/projects/<project>/<session-id>.jsonl` | Internal |
| Prompt history | `~/.claude/history.jsonl` (pasted content is left out) | Internal |
| MCP server logs | `~/.cache/claude-cli-nodejs/<project>/mcp-logs-<server>/` | Internal |

Its settings file is `~/.config/claudash/config.toml` (see [Configuration](#configuration)). claudash's own data lives in `~/.local/share/claudash/` (`~/Library/Application Support/claudash/` on macOS): `library.json` for tags, notes and stars, `usage-history.json` for the charts, and `trash/`.

Claude Code [documents the transcript format as internal](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored) and it can change between versions, so a Claude Code update may break the Sessions, Token Usage and Usage views until claudash catches up. claudash parses these files leniently and skips anything it doesn't recognize.

These actions change things, and only run when you ask:

- **`claudash setup --apply` / `--remove`**: edits `~/.claude/settings.json` (status line and hooks), after saving a timestamped backup next to it.
- **Move a session to the trash** (`D` then `y`, or `C`): moves its transcript, subagent transcripts and tool results, and its `file-history` checkpoints and `session-env`, out of Claude Code's directories into claudash's trash. `T` puts them back; they're deleted for good after 30 days or when you delete them from the trash (`D` twice).
- **Enable or disable a plugin** (`Space`): runs `claude plugin enable` or `claude plugin disable`.
- **Stop, respawn or attach to a background session** (`S`, `R`, `Enter` in Activity): runs `claude stop`, `claude respawn` or `claude attach`.
- **Remove or prune worktrees** (`D`, `P` in Projects): runs `git worktree remove` (without `--force`) or `git worktree prune`.
- **Sign in to or out of an MCP server** (`a`, `L` in the MCP list): runs `claude mcp login` or `claude mcp logout`.
- **Review a branch** (`b` in Projects): runs `git fetch`, adds a detached worktree under `~/.cache/claudash/reviews/` (or updates it for a second review), and runs Claude Code there: `claude -p` limited to reading files and `git log`/`diff`/`show` for a static review, or an interactive `claude` for the other modes. Reviews are saved in claudash's data directory.
- **Send a prompt** (`p`): runs `claude -p --resume <id>`, which adds the exchange to that session. Headless runs can't ask for permission, so tools that need approval are denied and reported in the reply.

Resuming, prompting and trashing are refused while the session is open in Claude Code, because two processes writing one transcript interleave their messages.

## Limitations

- `claude mcp list` briefly starts local MCP servers to check them, so each project is checked once, when you first select one of its sessions, and again only when you press `r`.
- `claude mcp list` doesn't say whether a server is user- or project-scoped; both show as `local`.
- The Project panel lists the instruction files loaded at launch. Files in subdirectories, which Claude Code loads when it works there, aren't shown.
- Plugin components are read from the standard layout (`skills/`, `agents/`, `commands/`, `hooks/hooks.json`). A plugin that declares custom paths in its manifest may show fewer components.
- Per-day cost isn't available locally, so the Usage view charts tokens; cost is shown per session where Claude Code recorded it.

## Platform support

Developed and tested on Linux (Ubuntu). macOS should work the same way; CI runs the test suite on Linux, macOS and Windows.

On Windows, `claude` is found whether Claude Code was installed natively (`claude.exe`) or through npm (`claude.cmd`). The full dashboard hasn't been tried on a Windows machine yet, so reports are welcome.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
