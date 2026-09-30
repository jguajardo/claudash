# claudash

A terminal dashboard for [Claude Code](https://code.claude.com/docs): browse, search and resume your sessions, see which instructions and MCP servers each project gets, track context and plan usage, and manage your skills, subagents, hooks and plugins, all from one screen.

> **Unofficial project.** claudash is not affiliated with, endorsed by or supported by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic.

```
 ◆ claudash    1 Dashboard   2 Ecosystem   3 Usage                                         ? keys 
╭ Sessions ────────────────────────────────────────╮╭ Project ──────────────── ~/code/api-server ╮
│ ▶ Add OAuth login to the API  ● working          ││ ● CLAUDE.md  project                       │
│     ~/code/api-server  feat/oauth  · just now    ││ ○ AGENTS.md  agents  (CLAUDE.md wins)      │
│   Fix flaky integration tests                    │╰────────────────────────────────────────────╯
│     ~/code/api-server  main  · 2 h ago           │╭ MCP Status ────────────────────────────────╮
│   Landing page redesign                          ││ ● filesystem         ONLINE     local      │
│     ~/code/website  · yesterday                  ││ ● context7           ONLINE     context7   │
│                                                  ││ ◐ linear             AUTH       engineering│
│                                                  ││ ✘ postgres           ERROR      local      │
│                                                  ││ ○ gmail              NOT SET    engineering│
╰────────────────────────────────────── 3 sessions ╯╰─────────────────────── 2/5 online · 4s ago ╯
╭ Token Usage ─────────────────────────────────────────────────────── Add OAuth login to the API ╮
│ ██████████████████████Context 23.4%  (234.1k / 1.0M, from status line)                         │
│ input 1.2k   cache write 310.5k   cache read 4.8M   output 52.3k   cost $6.41                  │
│ model claude-opus-5-5                                                                          │
│ plan 5h 41% (resets 16:30)   7d 12% (resets Sat 09:00)                                         │
╰────────────────────────────────────────────────────────────────────────────────────────────────╯
 Enter  resume   /  search   p  prompt   d  delete   Tab  MCP   r  reload   q  quit
```

## Features

### 1 · Dashboard

- **Sessions**: every Claude Code session on your machine, newest first, with its title, project folder, git branch and last activity. Sessions open right now in Claude Code are marked `▲ needs you` (waiting for a permission decision or other input), `● working` or `● waiting` (finished, waiting for your next prompt), and claudash sends a desktop notification and rings the terminal bell when a session starts needing you or finishes. The list refreshes every 5 seconds.
  - `Enter` resumes the session with `claude --resume` in its project folder and returns to the dashboard when you exit.
  - `/` filters by title, path or branch.
  - `p` sends a one-off prompt to the session (`claude -p --resume`) and shows the reply.
  - `d` deletes the session, after asking.
- **Project**: the instruction files Claude Code loads for the selected session's folder: managed, user and project `CLAUDE.md`, `CLAUDE.local.md` and `AGENTS.md`. It follows the [documented precedence rules](https://code.claude.com/docs/en/memory#agents-md) and your *Project instructions* setting, and files that are present but ignored (such as an `AGENTS.md` next to a `CLAUDE.md`) are crossed out with the reason.
- **MCP Status**: `claude mcp list` run in the selected project's folder, so project-scoped servers show up too, cached per project. Press `Tab` to move into the list and `Enter` to read a server's latest log, which usually says why it failed.
- **Token Usage**: how full the selected session's context window is, its input, cache and output tokens, cost and model.
  - **Cache diagnostics**: how much of the input was read from cache, flagged when cache writes dominate; with the status line, whether the cache is still warm and until when, its TTL, cache misses and their cause (such as `tools_changed` or `ttl_expired_5m`), and how many tokens the next reply re-caches once it goes cold.
  - **Plan usage**: 5-hour and 7-day limits with their reset times and a forecast of when you'll hit the limit at your current pace. claudash alerts you at 80% and 95%.

### 2 · Ecosystem

Everything Claude Code has available for the selected project, in five tabs: **Skills**, **Agents** (subagents), **Commands**, **Hooks** and **Plugins**. It covers user scope, project scope, claude.ai-synced skills and every enabled plugin. Plugins show their projected always-on token cost per session from `claude plugin details`, and `Space` enables or disables the selected plugin with `claude plugin enable/disable`.

### 3 · Usage

Plan usage gauges, a chart of tokens per day for the last 30 days (subagents included), totals for today, 7 and 30 days, and your most active projects this week.

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
claudash                       # open the dashboard
claudash --no-notify           # no desktop notifications or bell
claudash --context-limit 200k  # context window to assume when the status line hasn't reported one
claudash setup                 # connect the status line and hooks (see above)
claudash --help
```

### Keys

| Key | Action |
| --- | --- |
| `1` `2` `3` | Dashboard, Ecosystem, Usage |
| `↑` `↓` / `j` `k` | Move |
| `Enter` | Resume the session · open the MCP server's log · show details |
| `/` | Search sessions (`Enter` keeps the filter, `Esc` clears it) |
| `p` | Send a one-off prompt to the selected session |
| `d` / `Delete` | Delete the selected session (asks first) |
| `Tab` | Switch between the session list and the MCP list · next ecosystem tab |
| `←` `→` | Previous / next ecosystem tab |
| `Space` | Enable or disable the selected plugin |
| `r` | Reload sessions and re-check MCP servers and the ecosystem |
| `?` | Show all keys |
| `q`, `Ctrl+C` | Quit |

### Configuration

| Setting | Flag | Environment variable | Default |
| --- | --- | --- | --- |
| Context window when the status line hasn't reported one | `--context-limit <TOKENS>` | `CLAUDASH_CONTEXT_LIMIT` | `1M` |
| Claude Code's config directory | | `CLAUDE_CONFIG_DIR` | `~/.claude` |
| Desktop notifications and bell | `--no-notify` to turn off | | on |

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
| Sessions, tokens, cost | `~/.claude/projects/<project>/<session-id>.jsonl` | Internal |
| MCP server logs | `~/.cache/claude-cli-nodejs/<project>/mcp-logs-<server>/` | Internal |

Claude Code [documents the transcript format as internal](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored) and it can change between versions, so a Claude Code update may break the Sessions, Token Usage and Usage views until claudash catches up. claudash parses these files leniently and skips anything it doesn't recognize.

These actions change things, and only run when you ask:

- **`claudash setup --apply` / `--remove`**: edits `~/.claude/settings.json` (status line and hooks), after saving a timestamped backup next to it.
- **Delete a session** (`d`, then `y`): removes its transcript, subagent transcripts and tool results, and its `file-history` checkpoints and `session-env` under Claude Code's config directory. It can't be undone.
- **Enable or disable a plugin** (`Space`): runs `claude plugin enable` or `claude plugin disable`.
- **Send a prompt** (`p`): runs `claude -p --resume <id>`, which adds the exchange to that session. Headless runs can't ask for permission, so tools that need approval are denied and reported in the reply.

Resuming, prompting and deleting are refused while the session is open in Claude Code, because two processes writing one transcript interleave their messages.

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
