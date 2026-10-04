# Records demo/claudash.gif: the README's hero, with made-up data (see demo/README.md).
Output demo/claudash.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# Build the made-up data and open claudash; wait until it has loaded.
Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear && claudash"
Enter
Wait+Screen@180s /1 Now/
Sleep 4s
Show
Sleep 2s

# A session needs you: Enter shows exactly what it's asking to run.
Enter
Sleep 2.8s
Escape
Sleep 500ms

# Every project as a card; Enter opens a project's page.
Type "3"
Sleep 2.2s
Enter
Sleep 2.4s

# Its spec-driven changes and their next step.
Down
Down
Sleep 2.2s

# Its worktrees.
Down
Sleep 1.6s

# Its MCP servers: health, logs and sign-in.
Down
Down
Sleep 2.6s
Hide
Type "q"
