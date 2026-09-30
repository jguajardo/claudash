# claudash

A terminal dashboard for [Claude Code](https://docs.claude.com/en/docs/claude-code): browse and resume your sessions, check the health of your MCP servers and keep an eye on context usage, all from one screen.

> **Unofficial project.** claudash is not affiliated with, endorsed by or supported by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic.

```
 ◆ claudash   Control dashboard for Claude Code
╭ Sessions ────────────────────────────────────╮╭ MCP Status ────────────────────────────╮
│ ▶ Add OAuth login to the API                 ││ ● filesystem       ONLINE      local   │
│     ~/code/api-server  feat/oauth  · just now││ ● context7         ONLINE      context7│
│   Fix flaky integration tests                ││ ● playwright       ONLINE      local   │
│     ~/code/api-server  main  · 2 h ago       ││ ◐ linear           AUTH        eng     │
│   Landing page redesign                      ││ ✘ postgres         ERROR       local   │
│     ~/code/website  · yesterday              ││ ○ gmail            NOT SET     eng     │
╰────────────────────────────────── 3 sessions ╯╰─────────────────── 3/6 online · 4s ago ╯
╭ Token Usage ─────────────────────────────────────────────── Add OAuth login to the API ╮
│ ██████████████████████Context 23.4%  (234.1k / 1.0M)                                   │
│ input 1.2k   cache write 310.5k   cache read 4.8M   output 52.3k   cost $6.41          │
│ model claude-opus-5-5                                                                  │
╰────────────────────────────────────────────────────────────────────────────────────────╯
 ↑/↓  navigate   Enter  resume   /  search   r  reload   q  quit
```

## Features

- **Sessions** — every Claude Code session found in `~/.claude/projects`, newest first, with its AI-generated title, project folder, git branch and last activity. Refreshes automatically every 5 seconds (only changed files are re-read).
- **Resume with `Enter`** — suspends the dashboard, runs `claude --resume <id>` in the session's project folder and comes back when you exit Claude Code.
- **Search with `/`** — filter sessions by title, path or branch as you type.
- **MCP health** — runs `claude mcp list` in the background and shows every server (local, project, plugin and claude.ai connectors) as online, needs auth, failed or not configured.
- **Context usage** — how full the selected session's context window is, plus total input / cache / output tokens, cost and model.

## Installation

Requires [Claude Code](https://docs.claude.com/en/docs/claude-code) to be installed and on your `PATH` (the `claude` command is used for MCP checks and resuming sessions).

### Prebuilt binaries

Once the first release is published, prebuilt binaries for Linux, macOS and Windows will be available on the [Releases page](https://github.com/jguajardo/claudash/releases), along with a one-line installer:

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

## Usage

```sh
claudash                       # default context window: 1M tokens
claudash --context-limit 200k  # use a 200k context window
claudash --help
```

Launch it from the project folder whose MCP servers you want to see: project-scoped servers (from `.mcp.json` or per-project config) only show up in their own folder.

### Keys

| Key | Action |
| --- | --- |
| `↑` / `↓`, `k` / `j` | Move through sessions |
| `Home` / `End` | First / last session |
| `Enter` | Resume the selected session in Claude Code |
| `/` | Search sessions (`Enter` to keep the filter, `Esc` to clear it) |
| `r` | Reload sessions and re-check MCP servers |
| `Esc` | Clear the filter, or quit if there is none |
| `q`, `Ctrl+C` | Quit |

### Configuration

| Setting | Flag | Environment variable | Default |
| --- | --- | --- | --- |
| Context window size | `--context-limit <TOKENS>` | `CLAUDASH_CONTEXT_LIMIT` | `1M` |

Values accept suffixes: `1M`, `200k`, `1.5m`, `500000`. The flag takes precedence over the environment variable.

## How it works

claudash only reads local files and calls the `claude` CLI. It makes no network requests of its own.

| Panel | Source |
| --- | --- |
| Sessions | `~/.claude/projects/<project>/<session-id>.jsonl`: `ai-title`, `last-prompt`, and the `cwd` / `gitBranch` fields of each record |
| MCP status | Output of `claude mcp list`. Only server names and statuses are shown, never the command or URL, which may contain credentials |
| Token usage | `message.usage` of each assistant response (deduplicated by `requestId`, subagents excluded) and the last `cost-state` record |

These file formats are internal to Claude Code and may change between versions.

**Limitations**

- The context window size isn't stored in the session files, so it is a setting rather than detected per session.
- Plan quotas (5-hour / weekly limits) aren't available locally and aren't shown.
- `claude mcp list` briefly starts local MCP servers to check them, so it only runs at startup and when you press `r`.

## Platform support

Developed and tested on Linux (Ubuntu). macOS should work the same way; CI runs the test suite on Linux, macOS and Windows.

On Windows, `claude` is found whether Claude Code was installed natively (`claude.exe`) or through npm (`claude.cmd`). The full dashboard hasn't been tried on a Windows machine yet, so reports are welcome.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
