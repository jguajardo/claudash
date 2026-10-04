# claudash

A terminal dashboard for [Claude Code](https://code.claude.com/docs): browse, search and resume your sessions, see which instructions and MCP servers each project gets, track context and plan usage, and manage your skills, subagents, hooks and plugins, all from one screen.

> **Unofficial project.** claudash is not affiliated with, endorsed by or supported by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic.

![claudash demo: sessions that need you, an MCP server log, live activity, a session inspector, the ecosystem and usage views, prompt history and a daily summary](https://raw.githubusercontent.com/jguajardo/claudash/main/demo/claudash.gif)

<sub>Recorded from made-up data with [VHS](https://github.com/charmbracelet/vhs); see [demo/](demo/).</sub>

## Features

Four views, switched with `1`–`4`: **Now** (what needs you), **Sessions**, **Projects** and **Insights**. Projects and Insights have a menu of sections on the left. The footer shows the keys of where you are, `:` (or `Ctrl+P`) finds any action by name, and `?` lists everything.

### 1 · Now

The screen claudash opens on: what needs a decision from you.

- **Open sessions**, the ones that need you first: their state and why they wait (for example "permission prompt"), what exactly a session that needs you is asking to do (the command it wants to run, the edit it wants to make; `Enter` shows all of it), their last tool call, how full their context is and how many subagents are running. claudash sends a desktop notification and rings the terminal bell when a session starts needing you or finishes.
- **Background sessions** (`claude --bg`, [agent view](https://code.claude.com/docs/en/agent-view)), finished ones included. `Tab` moves to them: `Enter` attaches to one in this terminal (`claude attach`), `l` shows its output, and `S` stops it and `R` respawns it, both after asking.
- **Needs a look**: alerts across projects. Credentials in transcripts, risky commands from the last week, MCP servers that failed or need you to sign in, two open sessions in one folder or editing the same file, worktrees with forgotten work, stale or finished spec changes. When there's nothing, it says so. `Tab` moves into the alerts and `Enter` takes you to the place to act on one: the session's audit in the inspector, or the project's MCP servers, worktrees, specs or sessions, with that item selected.
- **Plan**: 5-hour and 7-day limits with their reset times and a forecast of when you'll hit the limit at your current pace. claudash alerts you at 80% and 95%.
- **Live feed** of every tool call from sessions active in the last hour, failures in red, refreshed every 2 seconds.

### 2 · Sessions

Every Claude Code session on your machine, newest first, with its title, project folder, git branch and last activity, marked `▲ needs you`, `● working` or `● waiting` when open.

- `Enter` resumes the session with `claude --resume` in its project folder and returns to claudash when you exit.
- `v` opens the conversation: your prompts and Claude's replies with their times, tool calls, compaction points and, with `o`, tool output. `/` searches inside it, `n`/`N` jump between matches, `r` reloads it to follow a running session, and `e` exports it to Markdown in `~/Documents/claudash-exports/`.
- `i` inspects the session: a chart of its context per request with compactions, every tool it called with its failure rate, the tool output that entered its context, its costliest prompts, an audit of risky actions and secrets, the files it edited, its subagents (type, model, tokens) and the skills, MCP servers and commands it used. `Tab` selects a subagent and `Enter` opens its own conversation.
- `p` sends a one-off prompt to the session (`claude -p --resume`) and shows the reply.
- `Tab` opens the session's project page.
- `/` filters by title, path, branch, tag or note (`#tag` works too). `t` tags a session, `c` adds a note and `*` stars it; they're kept by claudash, not in Claude Code's files.
- `D` moves the session to claudash's trash, after asking; `T` opens the trash to restore sessions or delete them for good, and anything left there is deleted after 30 days. `C` cleans up in bulk: pick a criterion (not used for 7/14/21 days, larger than 5/20 MB), check the list and move them all to the trash. Open and starred sessions are never included.

Beside the list, the selected session's **project** (the instruction files Claude Code loads there, following the [documented precedence rules](https://code.claude.com/docs/en/memory#agents-md) and your *Project instructions* setting, with ignored files crossed out and why; its git state; how many tokens a session there starts with; MCP health) and its **token usage**: how full its context window is, input, cache and output tokens, cost, model and subagents, with **cache diagnostics** (how much input came from cache; with the status line, whether the cache is still warm and until when, its TTL, misses and their cause, and what the next reply re-caches once it goes cold).

From any view:

- `f` searches the content of every conversation and opens the one you pick at the match.
- `h` searches every prompt you've typed in Claude Code, newest first. `Enter` puts the chosen one in the prompt box for the selected session, `Tab` copies it to the clipboard (OSC 52, works over SSH).
- `s` summarizes today across projects: sessions, prompts, tool calls and failures, files edited, tokens and each repository's commits. `e` exports it to Markdown in `~/Documents/claudash-exports/`.

### 3 · Projects

Every project your sessions ran in, as a card: its branch and git state, worktrees, sessions and which are open or need you, last activity and tokens this week, spec progress, and badges for secrets, risky commands and MCP servers that need a look. Arrows move between cards, `Enter` opens a project's page, `b` reviews one of its branches.

A project's page has a menu of sections on the left (`↑`/`↓` to choose, `Enter` or `→` to go in, `Esc` or `←` to come back):

| Section | What's there |
| --- | --- |
| **Overview** | What Claude Code loads there, its git state, recent sessions, activity, security and specs at a glance |
| **Sessions** | The project's sessions in a table; `Enter` resumes, `v` reads, `i` inspects |
| **Specs** | Spec-driven changes and their next step (see below) |
| **Worktrees** | Every checkout with its git state and sessions; `Enter` shows its sessions, `b` reviews a branch, `D` removes a worktree with `git worktree remove` (never `--force`), `P` prunes records of worktrees whose directory is gone; both ask first |
| **MCP servers** | `claude mcp list` run in the project's folder, so project-scoped servers show up too. `Enter` reads a server's latest log, which usually says why it failed, and `l` opens it in the Logs view, which follows new lines and filters them. For servers that need you to sign in, such as claude.ai connectors (Gmail, Google Calendar, Atlassian…) or HTTP servers, `a` runs `claude mcp login` and re-checks it when you're back; `L` signs out after asking |
| **Skills & plugins** | Skills, subagents, commands, hooks and plugins available there, with how often each was used in the last 30 days; plugins show their always-on token cost per session, so unused ones that cost tokens stand out, and `Space` enables or disables one. The **Permissions** tab merges allow, ask and deny rules from every settings file and flags the risky ones (see Security) |
| **Security** | Credentials in this project's transcripts, risky things Claude did here, and the permission rules worth a look |

#### Specs

If a project uses a spec-driven development framework, its Specs section lists its changes with a progress bar of their tasks, their stage (planning, implementing, complete), the sessions that worked on them and what those cost in tokens:

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

Now's alerts also flag changes with tasks left that nobody touched for 14 days, and changes whose tasks are all done, with the command to wrap them up.

#### Review a branch

`b` on a project card, in its menu or in Worktrees reviews one of its branches the way you'd review a pull request, with Claude Code doing the reviewing:

1. claudash runs `git fetch` and lists the remote's branches, newest first, with their last commit, author, how many commits they add and whether you've reviewed them. Type to search. Your own checkout's branch comes first when it has commits the base doesn't, so you can review your work before pushing it (only committed work is reviewed).
2. Pick one: it shows its size against the base (`develop`, or `main`, or `master`, whichever the remote has first) and asks how far to go:
   - **Static review**: Claude Code reads the commits, the diff and the code around it, in the background. It can only read files and run `git log`, `git diff` and `git show`; nothing runs and nothing changes.
   - **Review and run the tests** or **Review and start the project**: Claude Code opens in the terminal, reviews, then finds how to run the tests or start the project and does it, asking you before each command. The findings show when you exit.
3. The branch is checked out, detached, in a worktree under claudash's cache (`~/.cache/claudash/reviews/`), so your checkout, your branch and your `git status` are never touched.
4. The findings list each comment by file, line and severity, with a suggested change where it helps. `Enter` shows one with the code around that line, `Tab` copies it to paste into GitHub or Bitbucket, `e` exports the whole review to Markdown, `v` opens the review's Claude Code session to ask follow-up questions, and `D` removes the worktree. claudash never posts anything.

Comments are written in the language of the branch's commit messages. A review uses your plan like any other Claude Code session.

### 4 · Insights

A menu of three sections:

- **Plan & usage**: plan usage gauges with the forecast, a chart of tokens or dollars (`$`) per day (or per month with `m`), totals for today, 7 days, 30 days and a year, usage by model, and your most active projects this week, each with what it would cost at API prices. Claude Code deletes transcripts after 30 days by default, so claudash keeps its own copy of the daily numbers.
- **Where the limit went**: each session's, project's and model's part of the current 5-hour and 7-day windows, with their API-equivalent cost. With the status line set up, claudash uses the exact windows Claude Code reports and splits the percentage used among sessions by cost ("api-server took ≈26 of the 64 points"); without it, the 5-hour window is estimated from your activity. How requests count against plan limits isn't published, so the split is an estimate and says so.
- **Where tokens go** over the last 7 days, from your own transcripts:
  - **Tool output that entered the context**, by tool and, for Bash, by command (`$ cargo test`, `$ git diff`…), with calls and tokens per call. Verbose commands are the usual reason a context fills up.
  - **Costliest prompts**: every prompt with the tokens processed by all the requests it caused.
  - **Reply length and tool output per call** against the previous 30 days. If you install a token saver such as [caveman](https://github.com/JuliusBrussee/caveman) (shorter replies) or [rtk](https://github.com/rtk-ai/rtk) (shorter command output), claudash says it's installed and these numbers show what it actually saves you. Tool output is measured in characters, about 4 per token.
- **Security**: everything below, across every session.

### Security

claudash reads what Claude already did and points out what deserves a second look. It blocks nothing; that's what Claude Code's permissions are for.

- **Secrets in transcripts.** Claude Code keeps conversations in plain text for 30 days, so an API key you pasted or a token a command printed stays on disk. claudash looks for Anthropic, OpenAI, GitHub, GitLab, AWS, Google, Slack, Stripe, npm and crates.io keys and private keys in every transcript and in your prompt history. Sessions that have one are marked `🔑 secret` and Now lists them (masked, never in full) so you can rotate the key and trash the session with `D`. Exports (`e`, `claudash export`) replace any credential with a masked note.
- **Risky actions**: force pushes, `curl … | sh`, `sudo`, recursive deletes, `git reset --hard` and other ways to lose work, publishing, destructive SQL, reading `.env` or SSH keys, and editing files outside the project, by the session or its subagents.
- **Permission rules**: `Bash` with no restriction, `bypassPermissions`, rules such as `Bash(curl:*)` or `Bash(git push *)` that let Claude run any command of that kind without asking, allows that a deny overrides, repeats, and allow rules nothing used in 30 days.

Logs (MCP servers and background sessions) open with `l` from a server or a background session; they follow new lines, `/` filters them and `x` shows errors only.

### What it costs

Every response Claude Code records says which model answered and how many input, cache-write (5-minute and 1-hour), cache-read and output tokens it took, whether it ran in fast mode and how many web searches it made. claudash prices each one with [Claude API prices](https://platform.claude.com/docs/en/about-claude/pricing) and shows the result next to sessions, projects, days, models and the daily summary. On a Pro or Max plan you don't pay this; it's what that usage would cost through the API, which is the honest way to compare models, projects and habits.

```sh
claudash usage                 # last 30 days, by day
claudash usage monthly         # every month in claudash's history
claudash usage projects        # or models, sessions; --since 2026-09-01
claudash usage --json          # for scripts
claudash quota                 # where the current 5-hour and 7-day windows went (--json)
```

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
claudash --view projects       # open on another view
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
| `1`–`4` | everywhere | Now, Sessions, Projects, Insights |
| `:` `Ctrl+P` | everywhere | Commands: find any action by name and run it |
| `f` · `h` · `s` | everywhere | Search every conversation · prompt history · today's summary |
| `r` · `?` · `q` | everywhere | Reload · help · quit |
| `↑` `↓` / `j` `k` | lists, menus | Move |
| `←` `→` | project cards, menus | Move between cards · into a section and back |
| `Enter` | | Open or run the selected thing: what a session asks, resume, a project, a section, a log, the next spec step |
| `Esc` | | Back · clear the filter |
| `Tab` | | Next pane: background sessions and alerts in Now, the session's project, subagents in the inspector, setup tabs |
| `/` | | Filter sessions · search the conversation · filter log lines |
| `v` · `i` | sessions | Read the conversation · inspect it |
| `p` | sessions | Send a one-off prompt |
| `t` · `c` · `*` | sessions | Tag · note · star |
| `b` | projects | Review a branch with Claude Code |
| `l` | MCP servers, background | Log |
| `a` · `L` | MCP servers | Sign in (`claude mcp login`) · sign out (asks first) |
| `Space` | skills & plugins | Enable or disable the plugin |
| `o` · `n`/`N` · `e` | conversation | Tool output · next/previous match · export to Markdown |
| `m` | insights | Days or months |
| `x` | logs | Errors only |
| `D` | sessions, worktrees | Move the session to the trash · remove the worktree |
| `C` · `T` | sessions | Bulk cleanup · open the trash |
| `S` · `R` | background | Stop · respawn |
| `P` | worktrees | Prune missing worktrees |

### Configuration

Settings go in `~/.config/claudash/config.toml` (`~/Library/Application Support/claudash/config.toml` on macOS); `claudash config --init` writes one with every setting commented out. Flags win over environment variables, which win over the file.

| Setting | File | Flag | Environment variable | Default |
| --- | --- | --- | --- | --- |
| View to open on | `view = "projects"` | `--view <NAME>` | | `now` |
| Context window when the status line hasn't reported one | `context_limit = "200k"` | `--context-limit <TOKENS>` | `CLAUDASH_CONTEXT_LIMIT` | `1M` |
| Desktop notifications and bell | `notify = false` | `--no-notify` | | on |
| Colors | `colors = false` | | [`NO_COLOR`](https://no-color.org) | on |
| Claude Code's config directory | | | `CLAUDE_CONFIG_DIR` | `~/.claude` |

Token values accept suffixes: `1M`, `200k`, `1.5m`, `500000`.

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

Its settings file is `~/.config/claudash/config.toml` (see [Configuration](#configuration)). claudash's own data lives in `~/.local/share/claudash/` (`~/Library/Application Support/claudash/` on macOS): `library.json` for tags, notes and stars, `usage-history.json` for the charts, and `trash/`.

Claude Code [documents the transcript format as internal](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored) and it can change between versions, so a Claude Code update may break the Sessions, Projects and Insights views until claudash catches up. claudash parses these files leniently and skips anything it doesn't recognize.

These actions change things, and only run when you ask:

- **`claudash setup --apply` / `--remove`**: edits `~/.claude/settings.json` (status line and hooks), after saving a timestamped backup next to it.
- **Move a session to the trash** (`D` then `y`, or `C`): moves its transcript, subagent transcripts and tool results, and its `file-history` checkpoints and `session-env`, out of Claude Code's directories into claudash's trash. `T` puts them back; they're deleted for good after 30 days or when you delete them from the trash (`D` twice).
- **Enable or disable a plugin** (`Space` in a project's Skills & plugins): runs `claude plugin enable` or `claude plugin disable`.
- **Stop, respawn or attach to a background session** (`S`, `R`, `Enter` in Now): runs `claude stop`, `claude respawn` or `claude attach`.
- **Remove or prune worktrees** (`D`, `P` in a project's Worktrees): runs `git worktree remove` (without `--force`) or `git worktree prune`.
- **Sign in to or out of an MCP server** (`a`, `L` in a project's MCP servers): runs `claude mcp login` or `claude mcp logout`.
- **Review a branch** (`b` in Projects): runs `git fetch`, adds a detached worktree under `~/.cache/claudash/reviews/` (or updates it for a second review), and runs Claude Code there: `claude -p` limited to reading files and `git log`/`diff`/`show` for a static review, or an interactive `claude` for the other modes. Reviews are saved in claudash's data directory.
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
