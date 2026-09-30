# Records demo/claudash.gif with made-up data (see demo/README.md).
Output demo/claudash.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 70ms

Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear"
Enter
Sleep 1s
Show

Type "claudash"
Sleep 400ms
Enter
Sleep 3.5s

# Browse sessions: the Project and MCP panels follow the selection.
Down
Sleep 1.5s
Down
Sleep 1.5s
Up
Up
Sleep 1.5s

# Why is postgres failing? Open its log.
Tab
Sleep 400ms
Down
Down
Down
Down
Sleep 800ms
Enter
Sleep 3s
Escape
Sleep 400ms
Tab
Sleep 400ms

# Ecosystem: skills, agents, commands, hooks, plugins with their token cost.
Type "2"
Sleep 2s
Right
Sleep 900ms
Right
Sleep 900ms
Right
Sleep 900ms
Right
Sleep 3s

# Usage: plan limits with forecast, tokens per day, top projects.
Type "3"
Sleep 3.5s

# Back to the dashboard and search.
Type "1"
Sleep 800ms
Type "/"
Sleep 300ms
Type "data"
Sleep 1.5s
Enter
Sleep 1.5s
Escape
Sleep 1.5s
Type "q"
Sleep 500ms
