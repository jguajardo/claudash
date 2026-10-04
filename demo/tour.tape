# Records demo/tour.gif, the full tour, with made-up data (see demo/README.md).
Output demo/tour.gif

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
Sleep 4s

# Now: what needs you. Enter shows what the session is asking to do.
Enter
Sleep 2.5s
Escape
Sleep 800ms

# Alerts take you where to act: here, a failing MCP server's section.
Tab
Tab
Sleep 600ms
Down
Down
Down
Down
Sleep 1.2s
Enter
Sleep 2.5s
Escape
Sleep 300ms
Escape
Sleep 300ms

# Sessions: every session, its project and token usage.
Type "2"
Sleep 1.5s
Down
Sleep 1.2s
Down
Sleep 1.2s

# Projects: a card per project; Enter opens its page with a menu.
Type "3"
Sleep 2.5s
Enter
Sleep 2.5s
Down
Sleep 1.5s
Down
Down
Down
Sleep 2s
Enter
Sleep 600ms
Down
Down
Down
Down
Sleep 600ms
Enter
Sleep 2.5s
Escape
Sleep 400ms
Escape
Sleep 400ms
Down
Sleep 2s
Escape
Sleep 400ms
Escape
Sleep 600ms

# Insights: usage, where tokens go, security.
Type "4"
Sleep 2.5s
Down
Sleep 3s
Down
Sleep 2.5s

# Any action by name: today's summary from the command palette.
Type ":"
Sleep 800ms
Type "summary"
Sleep 1s
Enter
Sleep 3s
Escape
Sleep 800ms
Type "q"
Sleep 500ms
