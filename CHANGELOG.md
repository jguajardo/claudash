# Changelog

What changed in each release of claudash. Install or update with `brew upgrade claudash`, `cargo binstall claudash` or the installers on the [releases page](https://github.com/jguajardo/claudash/releases).

## Unreleased

- The README and the recordings lead with sessions and projects: new recordings of a project's page with an MCP sign-in, a branch review and a conversation, and a comparison by area.
- Interface: in a conversation, a reply that starts with a tool call gets its "Claude" header and MCP tools read `server › tool`; ages of two days or more are shown in days; counts are spelled out ("1 session", "2 sessions"); a project's overview no longer cuts a line that wraps; after `claude mcp login` the servers are checked again.

## 1.1.1 — 2026-10-04

- `claudash plan --share` prints whether your plan is worth it as a card to post, signed "made with claudash".

## 1.1.0 — 2026-10-04

- **Continue sessions a plan limit stopped.** Now lists them; once the limit resets, `Enter` continues them in the background (`claude --bg --resume`), and before it, `Enter` has them continue at the reset. `claudash continue [--wait]` does the same from the command line, and `auto_continue = true` does it on its own.
- **Is your plan worth it** (Insights and `claudash plan`): your last 30 days at API prices against the plan's price, how often a limit stopped you, and what Pro, Max 5x, Max 20x and the API would have cost you and how often they would have stopped you.
- **Snapshots** (opt-in, `snapshots = true`): the hook copies a project's files to a shadow git repository before each prompt and after each reply, so you can see and undo changes `/rewind` can't, from Bash commands and subagents.
- **Cache and idle gaps**: when a waiting session's cache goes cold and what the next prompt will cost to re-cache; Insights counts the prompts that re-cached after a break.
- **Allow-rule suggestions** for build, test and git-reading commands you run often and no rule allows.
- **Plan reset countdown**, and an alert when a window you used 90% or more of resets.
- **Compactions**: the inspector shows which files a compaction summary doesn't mention.
- Interface: MCP sign-in alerts are grouped per project; long names are cut instead of wrapped; risky-command checks ignore here-document bodies and quoted text; `--version`.
- The Homebrew formula is published by the release workflow.

## 1.0.0 — 2026-10-04

- **What it would cost at API prices**, everywhere: sessions, projects, Insights, `claudash usage` by day, month, project, model or session, with `--json`.
- **Where the limit went**: each session's, project's and model's part of the current 5-hour and 7-day windows (Insights and `claudash quota`).
- **`claudash wrapped`**: your week or month on one card (`w` in the dashboard).
- `claudash status` says when you'll hit the 5-hour limit at the current pace.
- Installers: Homebrew tap, shell and PowerShell scripts, static musl builds for older Linux, build attestations, and a Claude Code plugin.
- Alerts in Now take you where to act; `NO_COLOR` and `colors = false`.

## 0.8.0 — 2026-10-03

- Four views: Now, Sessions, Projects (cards, and a page per project with a menu) and Insights. 0.8.1 opens the Logs view from an MCP server.

## 0.7.0 — 2026-10-02

- Audit of what Claude did: credentials in transcripts and prompt history, risky commands, permission rules broader than you think.
- Where tokens go: tool output by command, costliest prompts, reply length.
- Spec-driven development: OpenSpec, spec-kit, Kiro, Task Master and GSD changes with their next step.
- MCP sign-in from the dashboard, what a waiting session asks to run, and a review before pushing.

## 0.6.0 — 2026-09-30

- Branch reviews: claudash checks a branch out in its own worktree and Claude Code reviews it.

## 0.5.0 — 2026-09-30

- A command palette (`:` or Ctrl+P), consistent keys, `claudash status` and `claudash summary`.

## 0.4.0 — 2026-09-30

- Background sessions, worktree cleanup, a Logs view, a live activity feed, per-subagent conversations, a Projects view with conflict detection, a session inspector, `claudash doctor` and a full help view.

## 0.3.0 — 2026-09-30

- Tags, notes, stars, a trash and bulk cleanup; a conversation viewer with search and export; alerts, a plan forecast and cache diagnostics.

## 0.2.0 — 2026-09-30

- Project context, skills and plugins, usage views and plan usage.

## 0.1.0 — 2026-09-30

- First release: every Claude Code session on one screen.
