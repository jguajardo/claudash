# claudash

**The control room for Claude Code.** One terminal screen: which sessions need you and what they're asking, where your 5-hour and weekly limits went, what your usage costs at API prices, and the API keys and risky commands Claude left behind on disk.

A single Rust binary that reads Claude Code's local files and runs the `claude` CLI. No server, no account, no network requests of its own.

[![crates.io](https://img.shields.io/crates/v/claudash.svg)](https://crates.io/crates/claudash)
[![CI](https://github.com/jguajardo/claudash/actions/workflows/ci.yml/badge.svg)](https://github.com/jguajardo/claudash/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Built With Ratatui](https://img.shields.io/badge/Built_With_Ratatui-000?logo=ratatui&logoColor=fff)](https://ratatui.rs/)

![claudash: sessions that need you and what they ask, alerts, plan usage, project pages, where the limit went, and a wrapped card](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/claudash.gif)

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

You run Claude Code in three terminals and a background session. One has been waiting on a permission prompt for twenty minutes. You hit your 5-hour limit and don't know which session ate it. Last week you pasted an API key into a prompt, and it's still sitting in a plain-text transcript. claudash puts all of that on one screen:

- **What needs you, and what it wants.** Every open session, interactive or background, the ones waiting on you first, with the exact command or edit each one is asking to run. Desktop notification when one starts waiting.
- **Where the limit went.** Each session's, project's and model's part of the current 5-hour and 7-day windows, and what all of it would cost at API prices, by day, project, model and session. Sessions the limit stopped continue on their own when it resets.
- **What Claude left on disk.** API keys and tokens in your transcripts and prompt history (shown masked), force pushes, `curl | sh`, `sudo` and other risky commands, and permission rules broader than you think.
- **Each project on one page.** Its sessions, MCP servers (with their logs and sign-in), spec-driven changes (OpenSpec, spec-kit, Kiro, Task Master) and their next step, worktrees, snapshots that undo what `/rewind` can't (Bash commands, subagents), an AI review of any branch, skills and plugins with what they cost you per session.

It never drives Claude Code: Claude Code does the work, claudash keeps track of it. Everything it changes, it asks first.

| Where the limit went, and what it cost | What Claude left on disk | Your week on one card |
| --- | --- | --- |
| ![Insights: each session's part of the 5-hour and 7-day windows, and dollars per day](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/limits.gif) | ![Insights › Security: masked API keys found in transcripts and a force push](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/secrets.gif) | ![The wrapped card: prompts, cost, streak, projects, models, tools, hours](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/wrapped.gif) |

## Tour

| View | What's there |
| --- | --- |
| **1 · Now** | Open sessions (what a waiting one asks, last tool call, context use, subagents), background sessions you can attach to, stop or respawn, alerts that take you where to act, plan usage with a forecast and today's cost, a live feed of every tool call |
| **2 · Sessions** | Every session, searchable and filterable, with its state, cost, project instructions and token use. Resume, read, inspect (context per request, tool failures, audit, costliest prompts), send a prompt, tag, note, star, trash |
| **3 · Projects** | A card per project, then a page with Overview, Sessions, Specs, Worktrees, MCP servers, Skills & plugins (with a permission-rule linter) and Security |
| **4 · Insights** | Plan & usage in tokens or dollars, where the limit went, where tokens go (tool output by Bash command, costliest prompts, whether caveman or rtk changed anything), security across every session |

Anywhere: `f` searches every conversation, `h` searches every prompt you've typed, `s` summarizes today, `w` shows your **wrapped** week or month on one card made to be shared (`x` hides project names). The [guide](docs/guide.md) covers every view, key and setting.

From the command line, for scripts and status bars:

```sh
claudash status            # ▲ 1 needs you · 2 working · 5h 64% → limit 13:10 · 7d 31%   (tmux, Waybar…; --json)
claudash usage             # tokens and API-equivalent dollars by day; monthly, projects, models, sessions; --json
claudash quota             # where the current 5-hour and 7-day windows went; --json
claudash wrapped --redact  # your week on a card; month; --plain
claudash continue --wait   # continue the sessions a plan limit stopped when it resets
claudash summary           # today as Markdown
claudash export <ID>       # a conversation as Markdown, credentials masked
```

## How it compares

| | claudash | [ccusage](https://github.com/ryoppippi/ccusage) | [Claude Code Usage Monitor](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor) | `claude agents` (built in) |
| --- | --- | --- | --- | --- |
| Which session needs you, and what it's asking | ✓ every session | | | background sessions |
| Cost at API prices by day, project, model, session | ✓ | ✓ | ✓ | |
| Each session's part of the 5-hour and 7-day windows | ✓ | 5-hour blocks | forecasts | |
| Credentials in transcripts, risky-command audit, permission linter | ✓ | | | |
| MCP health, logs and sign-in; specs; worktrees; branch review | ✓ | | | |
| Other coding agents (Codex, Gemini…) | | ✓ | | |
| Form | TUI + CLI, one binary | CLI | TUI | Claude Code |

They're complementary: ccusage covers more agents and reports, the Usage Monitor focuses on forecasting. claudash is the one screen for running Claude Code day to day.

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
| MCP server logs | `~/.cache/claude-cli-nodejs/<project>/mcp-logs-<server>/` | Internal |

Its settings file is `~/.config/claudash/config.toml` (see [Configuration](docs/guide.md#configuration)). claudash's own data lives in `~/.local/share/claudash/` (`~/Library/Application Support/claudash/` on macOS): `library.json` for tags, notes and stars, `usage-history.json` for the charts, `trash/`, and `snapshots/` when snapshots are on.

Claude Code [documents the transcript format as internal](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored) and it can change between versions, so a Claude Code update may break the Sessions, Projects and Insights views until claudash catches up. claudash parses these files leniently and skips anything it doesn't recognize.

These actions change things, and only run when you ask:

- **`claudash setup --apply` / `--remove`**: edits `~/.claude/settings.json` (status line and hooks), after saving a timestamped backup next to it.
- **Move a session to the trash** (`D` then `y`, or `C`): moves its transcript, subagent transcripts and tool results, and its `file-history` checkpoints and `session-env`, out of Claude Code's directories into claudash's trash. `T` puts them back; they're deleted for good after 30 days or when you delete them from the trash (`D` twice).
- **Enable or disable a plugin** (`Space` in a project's Skills & plugins): runs `claude plugin enable` or `claude plugin disable`.
- **Stop, respawn or attach to a background session** (`S`, `R`, `Enter` in Now): runs `claude stop`, `claude respawn` or `claude attach`.
- **Remove or prune worktrees** (`D`, `P` in a project's Worktrees): runs `git worktree remove` (without `--force`) or `git worktree prune`.
- **Sign in to or out of an MCP server** (`a`, `L` in a project's MCP servers): runs `claude mcp login` or `claude mcp logout`.
- **Review a branch** (`b` in Projects): runs `git fetch`, adds a detached worktree under `~/.cache/claudash/reviews/` (or updates it for a second review), and runs Claude Code there: `claude -p` limited to reading files and `git log`/`diff`/`show` for a static review, or an interactive `claude` for the other modes. Reviews are saved in claudash's data directory.
- **Continue sessions a plan limit stopped** (`Enter` on the alert in Now, `claudash continue`, or `auto_continue = true`): runs `claude --bg --resume <id>` in each session's folder with the prompt "The usage limit has reset. Continue where you left off." Never for a session that's open.
- **Snapshots** (off unless `snapshots = true` in the settings file): before each prompt and after each reply, `claudash hook` commits the project's files to a separate git repository under `snapshots/` in claudash's data directory, using the project's `.gitignore`; the project's own `.git` is never touched. `U` in a project's Snapshots puts a snapshot's files back with `git checkout`, after asking and after snapshotting the current state.
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
