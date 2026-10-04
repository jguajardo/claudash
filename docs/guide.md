# claudash guide

Everything each view shows and does, every key and setting. For what claudash is and how to install it, see the [README](../README.md).

## Views

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
- `s` summarizes today across projects: sessions, prompts, tool calls and failures, files edited, tokens, dollars and each repository's commits. `e` exports it to Markdown in `~/Documents/claudash-exports/`.
- `w` shows your **wrapped** card: the last week (or month, with `Tab`) on one screen made to be shared. Prompts, requests, sessions, tokens and API-equivalent dollars, active days and streak, the busiest day, the top session, projects and models by share, most used tools and commands, the hours you work, and secrets or risky commands found. `x` hides project and session names before you post it, and `e` exports it. `claudash wrapped [week|month] [--redact]` prints it in color in the terminal.

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
| **Skills & plugins** | Skills, subagents, commands, hooks and plugins available there, with how often each was used in the last 30 days; plugins show their always-on token cost per session, so unused ones that cost tokens stand out, and `Space` enables or disables one. The **Permissions** tab merges allow, ask and deny rules from every settings file and flags the risky ones (see [Security](#security)) |
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
claudash wrapped --redact      # your week on a card to share (month, --plain)
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


## Command line

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

