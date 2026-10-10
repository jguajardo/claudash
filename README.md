# claudash

**The control room for Claude Code.** Every session and every project on one terminal screen: see what a waiting session is asking, resume or read any conversation, sign in to MCP servers, review a branch in its own worktree, run the next spec step, undo what `/rewind` can't, and see where your limits went.

A single Rust binary that reads Claude Code's local files and runs the `claude` CLI. No server, no account, no network requests of its own.

[![crates.io](https://img.shields.io/crates/v/claudash.svg)](https://crates.io/crates/claudash)
[![CI](https://github.com/jguajardo/claudash/actions/workflows/ci.yml/badge.svg)](https://github.com/jguajardo/claudash/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Built With Ratatui](https://img.shields.io/badge/Built_With_Ratatui-000?logo=ratatui&logoColor=fff)](https://ratatui.rs/)

![claudash: a waiting session and what it asks to run, then a project's page with its spec-driven changes, worktrees and MCP servers](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/claudash.gif)

<sub>Recorded from made-up data with [VHS](https://github.com/charmbracelet/vhs); see [demo/](demo/). Unofficial project: not affiliated with, endorsed by or supported by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic.</sub>

## Install

```sh
brew install jguajardo/tap/claudash                 # macOS, Linux
cargo binstall claudash                             # prebuilt, with cargo-binstall
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jguajardo/claudash/releases/latest/download/claudash-installer.sh | sh
```

On Windows: `powershell -c "irm https://github.com/jguajardo/claudash/releases/latest/download/claudash-installer.ps1 | iex"`. From source: `cargo install claudash`. Prebuilt binaries for Linux (glibc and static musl), macOS and Windows are on the [Releases page](https://github.com/jguajardo/claudash/releases), with build attestations (`gh attestation verify <file> --owner jguajardo`).

Then:

```sh
claudash setup --apply   # optional: plan usage, alerts and exact limit windows (backs up settings.json first)
claudash                 # 1 Now · 2 Sessions · 3 Projects · 4 Insights · : any action · ? help
```

## Why

You run Claude Code in three terminals and a background session, across four repositories. One session has been waiting on a permission prompt for twenty minutes. A branch is waiting for a review. An MCP server needs you to sign in again, so the session that uses it is working without it. A change is five tasks into eight and you don't remember the command for the next one. claudash shows all of that on one screen, and acts on it from there.

**Sessions**

- **What needs you, and what it wants.** Every open session, interactive or background, the waiting ones first, with the exact command or edit each one is asking to run. A desktop notification when one starts waiting; attach to, stop or respawn the background ones.
- **Any conversation, a few keys away.** Every session you've run, filterable, with full-text search across all of them. Resume one in Claude Code, read it with its tool output and subagents, send it a prompt, inspect what filled its context, tag, note, star or trash it.
- **Several at once, like terminal tabs.** Inside Zellij or tmux a session you resume or attach to opens in a tab of its own, so claudash stays on screen while it works and you open the next one.
- **Stopped at a limit, continued at the reset.** Sessions a plan limit stopped are listed, and continue in the background once the window resets.

**Projects**

- **Each project on one page.** What Claude Code loads there (instruction files, skills, agents, commands, hooks, permission rules, plugins and what each adds to every session), its git state, sessions and worktrees, and a warning when two open sessions edit the same file.
- **MCP servers you can fix.** Which are online, which failed and why (the server's log), which need you to sign in; `a` signs in without leaving the dashboard.
- **A review of any branch.** claudash fetches, checks the branch out in a worktree of its own and has Claude Code review it there: findings by file, line and severity, each with the code around it and a suggested change. Your working tree isn't touched, and several reviews, of one project or of several, run at once and wait in a list until you read them.
- **Specs and their next step.** OpenSpec, spec-kit, Kiro, Task Master and GSD changes with their progress; `Enter` runs the next step in Claude Code.
- **Undo what `/rewind` can't.** Snapshots of a project's files before each prompt and after each reply, turned on with one key, so changes made by Bash commands and subagents can be seen and put back.

**Security.** API keys and tokens left in your transcripts and prompt history (shown masked), force pushes, `curl | sh`, `sudo` and other risky commands, and permission rules broader than you think.

**Usage.** Each session's, project's and model's part of the current 5-hour and 7-day windows, what your use would cost at API prices, and whether Pro, Max 5x, Max 20x or the API fits it.

claudash doesn't replace Claude Code or sit between you and it: Claude Code does the work in its own terminal, claudash keeps track of it and runs the `claude` CLI for you. Everything it changes, it asks first.

| A project's page, and an MCP sign-in | A review of a branch |
| --- | --- |
| ![A project's page: its spec-driven changes with the next step, MCP servers, signing in to one that needs it, and what each plugin adds to a session](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/project.gif) | ![Reviewing a branch: the remote's branches, a static review in a worktree of its own, findings by file, line and severity, and one finding with its code and suggested change](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/review.gif) |
| **Any conversation, searchable** | **What Claude left on disk** |
| ![Searching every conversation, opening the one that matches and reading it with its tool output](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/conversation.gif) | ![Insights › Security: masked API keys found in transcripts and a force push](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/secrets.gif) |
| **Where the limit went, and what it cost** | **Is your plan worth it** |
| ![Insights: each session's part of the 5-hour and 7-day windows, and dollars per day](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/limits.gif) | ![claudash plan --share: a month of use at API prices against the plan's price, and how often Pro, Max 5x and Max 20x would have stopped you](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/plan.gif) |

## Tour

| View | What's there |
| --- | --- |
| **1 · Now** | Open sessions (what a waiting one asks, last tool call, context use, subagents), background sessions you can attach to, stop or respawn, alerts that take you where to act, plan usage with a forecast and today's cost, a live feed of every tool call |
| **2 · Sessions** | Every session, searchable and filterable, with its state, cost, project instructions and token use. Resume, read (search, tool output, subagents, export), inspect (context per request, tool failures, audit, costliest prompts), send a prompt, tag, note, star, trash |
| **3 · Projects** | A card per project (`b` reviews one of its branches; `B` lists every project's reviews), then a page with Overview, Sessions, Specs, Worktrees (remove one, or clean up the idle ones), Snapshots, MCP servers, Skills & plugins (with a permission-rule linter) and Security |
| **4 · Insights** | Plan & usage in tokens or dollars, where the limit went, where tokens go (tool output by Bash command, costliest prompts, whether caveman or rtk changed anything), security across every session |

Anywhere: `f` searches every conversation, `h` searches every prompt you've typed, `B` lists every project's branch reviews, `s` summarizes today, `:` runs any action by name, `w` shows your [wrapped](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/wrapped.gif) week or month on one card made to be shared (`x` hides project names). The [guide](docs/guide.md) covers every view, key and setting, and the [full tour](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/tour.gif) shows them in a minute.

From the command line, for scripts and status bars:

```sh
claudash status            # ▲ 1 needs you · 2 working · 5h 64% → limit 13:10 · 7d 31%   (tmux, Waybar…; --json)
claudash continue --wait   # continue the sessions a plan limit stopped when it resets
claudash export <ID>       # a conversation as Markdown, credentials masked
claudash summary           # today as Markdown
claudash usage             # tokens and API-equivalent dollars by day; monthly, projects, models, sessions; --json
claudash quota             # where the current 5-hour and 7-day windows went; --json
claudash plan --share      # is your plan worth it, as a card to post: API-equivalent use vs. price
claudash wrapped --redact  # your week on a card; month; --plain
```

## How it compares

Other tools cover parts of this, from two sides: the ones that run agents for you, and the ones that count what you used. claudash's job is a third one: the screen for the sessions you start yourself, and for the projects around them.

**Sessions and projects**

| | claudash | [`claude agents`](https://code.claude.com/docs/en/agent-view) (built in) | [claude-squad](https://github.com/smtg-ai/claude-squad) | [ccmanager](https://github.com/kbwo/ccmanager) | [Agent of Empires](https://github.com/agent-of-empires/agent-of-empires) | [opcode](https://github.com/winfunc/opcode) |
| --- | --- | --- | --- | --- | --- | --- |
| Starts and runs agents for you | no, you start them | ✓ background sessions | ✓ | ✓ | ✓ | ✓ |
| Sessions you opened yourself, in any terminal | ✓ open and past | once you background them | | | imports past ones | past ones |
| Which session is waiting on you | ✓ | ✓ | | ✓ | ✓ | |
| What it's asking to run | ✓ | ✓ and you reply there | | | ✓ structured sessions | |
| Resume a past conversation | ✓ | ✓ as a background session | sessions it paused | as an argument | ✓ | ✓ |
| Read and search the text of every conversation | ✓ | | | | | read |
| Continue sessions when a plan limit resets | ✓ | | | | ✓ opt-in | |
| What Claude Code loads in a project: instructions, skills, hooks, plugins | ✓ | | | | skills | CLAUDE.md, agents |
| MCP servers: health, logs, sign-in | ✓ | | | | list | registry, connection test |
| Spec-driven changes and their next step | ✓ | | | | | |
| Worktrees | list, clean up | one per session | one per session | create, merge, delete | create, clean up | |
| AI review of a branch in a worktree of its own | ✓ | | | | | |
| Undo file changes `/rewind` can't | ✓ opt-in snapshots | | | | | checkpoints |
| Other coding agents (Codex, Gemini…) | | | ✓ | ✓ | ✓ | |
| Form | TUI + CLI | Claude Code | TUI on tmux | TUI | TUI and web on tmux | desktop app |

**Security and usage**

| | claudash | [ccusage](https://github.com/ccusage/ccusage) | [Claude Code Usage Monitor](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor) | [opcode](https://github.com/winfunc/opcode) |
| --- | --- | --- | --- | --- |
| Credentials left in transcripts, risky commands, a permission-rule linter | ✓ | | | |
| Cost at API prices by day, project, model and session | ✓ | ✓ | by day; project and model in an opt-in history | by model, project and date |
| Each session's part of the 5-hour and 7-day windows | ✓ | 5-hour blocks | forecasts for the account | |
| Pro, Max 5x, Max 20x or the API for your use | ✓ | API-equivalent value | plan suggestions | |
| Other coding agents | | ✓ | | |
| Form | TUI + CLI | CLI | terminal monitor | desktop app |

<sub>From each project's README and documentation on 2026-10-04. A blank cell means they don't mention it, not that the tool can't do it; corrections are welcome in an issue.</sub>

They're complementary. claude-squad, ccmanager, Agent of Empires and Claude Code's own agent view start agents for you and keep them in panes or background sessions of their own, and several of them drive other agents too. claudash doesn't run your agents: it watches the sessions you start yourself, in any terminal, and adds the project around them (MCP servers, specs, worktrees, branch reviews, what Claude Code loads). ccusage covers many more agents and reports, and the Usage Monitor focuses on forecasting.

## Claude Code plugin

claudash also ships as a Claude Code plugin, so Claude itself can answer "what did this week cost?", "what used up my limit?" or "which session is waiting?" by running the claudash CLI (it needs claudash installed):

```sh
claude plugin marketplace add jguajardo/claudash
claude plugin install claudash@claudash
```

It adds one skill (`/claudash:report`), about 160 tokens per session, and nothing else.

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

## What claudash reads and changes

claudash makes no network requests of its own. It reads local files and runs the `claude` CLI.

| Data | Source | Kind |
| --- | --- | --- |
| API-equivalent dollars | Token counts in the transcripts priced with the [published Claude API prices](https://platform.claude.com/docs/en/about-claude/pricing) built into claudash | Documented prices |
| Plan usage, context window size, prompt cache | [Status line](https://code.claude.com/docs/en/statusline) JSON saved by `claudash statusline` to `~/.cache/claudash/statusline/` | Documented |
| Needs you / working / waiting | [Hook](https://code.claude.com/docs/en/hooks) events saved by `claudash hook` to `~/.cache/claudash/state/` | Documented |
| Open sessions | `claude agents --json` | Documented |
| MCP servers | `claude mcp list`, run in the project folder. Only names and statuses are shown, never commands or URLs, which may contain credentials | Documented |
| Plugins | `claude plugin list --json`, `claude plugin details` | Documented |
| Instruction files | The [memory docs](https://code.claude.com/docs/en/memory) rules; the *Project instructions* setting from user and managed settings | Documented |
| Skills, agents, commands, hooks | `SKILL.md` and Markdown frontmatter, `hooks` in settings files and plugins' `hooks/hooks.json` | Documented |
| Sessions, conversations, tokens, cost | `~/.claude/projects/<project>/<session-id>.jsonl` | Internal |
| Prompt history | `~/.claude/history.jsonl` (pasted content is left out) | Internal |
| Your plan (Pro, Max 5x, Max 20x) | `oauthAccount` rate-limit tier in `~/.claude.json`, unless `plan` is set in the settings file | Internal |
| Where a plan limit stopped a session, and when it resets | The `rate_limit` error reply in its transcript | Internal |
| MCP server logs | `~/.cache/claude-cli-nodejs/<project>/mcp-logs-<server>/` | Internal |

Its settings file is `~/.config/claudash/config.toml` (see [Configuration](docs/guide.md#configuration)). claudash's own data lives in `~/.local/share/claudash/` (`~/Library/Application Support/claudash/` on macOS): `library.json` for tags, notes and stars, `usage-history.json` for the charts, `trash/`, and `snapshots/` when snapshots are on.

Claude Code [documents the transcript format as internal](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored) and it can change between versions, so a Claude Code update may break the Sessions, Projects and Insights views until claudash catches up. claudash parses these files leniently and skips anything it doesn't recognize.

These actions change things, and only run when you ask:

- **`claudash setup --apply` / `--remove`**: edits `~/.claude/settings.json` (status line and hooks), after saving a timestamped backup next to it.
- **Move a session to the trash** (`D` then `y`, or `C`): moves its transcript, subagent transcripts and tool results, and its `file-history` checkpoints and `session-env`, out of Claude Code's directories into claudash's trash. `T` puts them back; they're deleted for good after 30 days or when you delete them from the trash (`D` twice).
- **Enable or disable a plugin** (`Space` in a project's Skills & plugins): runs `claude plugin enable` or `claude plugin disable`.
- **Stop, respawn or attach to a background session** (`S`, `R`, `Enter` in Now): runs `claude stop`, `claude respawn` or `claude attach`.
- **Remove or prune worktrees** (`D`, `C`, `P` in a project's Worktrees): runs `git worktree remove`, or `git worktree prune` for records whose directory is gone. Before removing, claudash says what stands in the way and asks: it can stop a background session running there (`claude stop`), unlock a worktree whose lock was left by a process that has ended or was set by hand (`git worktree unlock`; the question says which), and, only after listing them and a second key, delete its modified and untracked files (`--force`). Files its `.gitignore` ignores go with the worktree, as git does it; the question names them. The branch is always kept. It never removes a worktree that a session has open in another terminal, or whose lock belongs to a running process.
- **Sign in to or out of an MCP server** (`a`, `L` in a project's MCP servers): runs `claude mcp login` or `claude mcp logout`.
- **Review a branch** (`b` in Projects): runs `git fetch`, adds a detached worktree under `~/.cache/claudash/reviews/` (or updates it for a second review), and runs Claude Code there: `claude -p` limited to reading files and `git log`/`diff`/`show` for a static review, or an interactive `claude` for the other modes. Up to four run at once. Reviews are saved in claudash's data directory.
- **Open a session in a tab or pane** (inside Zellij or tmux, when you resume, attach, run a spec step or start an interactive review): runs `zellij action new-tab` or `zellij run`, or `tmux new-window` or `tmux split-window`, with the `claude` command in the session's folder. `open_in = "here"` in the settings file turns this off.
- **Continue sessions a plan limit stopped** (`Enter` on the alert in Now, `claudash continue`, or `auto_continue = true`): runs `claude --bg --resume <id>` in each session's folder with the prompt "The usage limit has reset. Continue where you left off." Never for a session that's open.
- **Snapshots** (off until you turn them on: `T` in a project's Snapshots writes `snapshots = true` to the settings file and, if `claudash setup --apply` hasn't been run, does what it does, after asking): before each prompt and after each reply, `claudash hook` commits the project's files to a separate git repository under `snapshots/` in claudash's data directory, using the project's `.gitignore`; the project's own `.git` is never touched. `U` in a project's Snapshots puts a snapshot's files back with `git checkout`, after asking and after snapshotting the current state.
- **Send a prompt** (`p`): runs `claude -p --resume <id>`, which adds the exchange to that session. Headless runs can't ask for permission, so tools that need approval are denied and reported in the reply.

Resuming, prompting and trashing are refused while the session is open in Claude Code, because two processes writing one transcript interleave their messages.

## Limitations

- `claude mcp list` briefly starts local MCP servers to check them, so each project is checked once, when you first select one of its sessions, and again only when you press `r`.
- `claude mcp list` doesn't say whether a server is user- or project-scoped; both show as `local`.
- The Project panel lists the instruction files loaded at launch. Files in subdirectories, which Claude Code loads when it works there, aren't shown.
- Plugin components are read from the standard layout (`skills/`, `agents/`, `commands/`, `hooks/hooks.json`). A plugin that declares custom paths in its manifest may show fewer components.
- Dollars are API-equivalent, computed from the token counts with the prices built into this release: a model released after it shows tokens without dollars until claudash is updated.

## Platform support

Developed and tested on Linux (Ubuntu). macOS should work the same way; CI runs the test suite on Linux, macOS and Windows.

On Windows, `claude` is found whether Claude Code was installed natively (`claude.exe`) or through npm (`claude.cmd`). The full dashboard hasn't been tried on a Windows machine yet, so reports are welcome.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
