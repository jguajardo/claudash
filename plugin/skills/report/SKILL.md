---
name: report
description: Answers questions about your own Claude Code use on this machine with the claudash CLI. What sessions and projects cost at API prices, where the current 5-hour and 7-day plan windows went, which sessions are open or waiting for you, what was done today, and a week or month summary card.
when_to_use: When the user asks how much they spent or used, what used up their limit, why they hit their usage limit, which session is waiting, what they did today or this week, or wants their Claude Code stats or "wrapped".
allowed-tools: Bash(claudash *)
---

Use the `claudash` CLI; it reads Claude Code's local files and makes no network requests. Prefer `--json` and summarize the result for the user in a few lines, with the numbers that answer their question.

| Question | Command |
| --- | --- |
| What did my usage cost, by day / month / project / model / session? | `claudash usage [daily\|monthly\|projects\|models\|sessions] [--since YYYY-MM-DD] --json` |
| Where did my 5-hour or weekly limit go? | `claudash quota --json` |
| Which sessions are open, working or waiting for me? Plan usage now? | `claudash status --json` |
| What did I do today? | `claudash summary` (Markdown) |
| My week or month on one card | `claudash wrapped [week\|month] --plain` (add `--redact` before sharing) |
| Is claudash connected to Claude Code? | `claudash doctor` |

Notes for your answer:

- Dollars are API-equivalent: what the usage would cost at Claude API prices. On a Pro or Max plan the user doesn't pay this; say so when it matters.
- In `claudash quota`, each session's share of the limit is an estimate by API-equivalent cost; how requests count against plan limits isn't published. When the window is "estimated", running `claudash setup --apply` gives the exact window and percentage.
- If `claudash` isn't installed, tell the user how to get it instead of guessing numbers: `brew install jguajardo/tap/claudash`, `cargo binstall claudash`, or the installer at https://github.com/jguajardo/claudash.
- For the full picture, suggest opening `claudash` in a terminal.
