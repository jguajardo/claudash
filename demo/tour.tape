# Records demo/tour.gif: the full tour, with made-up data (see demo/README.md).
Output demo/tour.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set Framerate 20
Set TypingSpeed 70ms

# Build the made-up data and leave a clean prompt.
Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear && echo READY"
Enter
Wait+Screen@180s /READY/
Type "clear"
Enter
Sleep 700ms
Show

Type "claudash"
Sleep 400ms
Enter
Sleep 5s

# Now: what needs you. Enter shows what the session is asking to do.
Enter
Sleep 2.8s
Escape
Sleep 800ms

# Sessions: every session you've run. / filters them, v reads a conversation.
Type "2"
Sleep 1.8s
Type "/"
Sleep 400ms
Type "cart"
Sleep 800ms
Enter
Sleep 1s
Type "v"
Sleep 3.2s
Escape
Sleep 400ms
Escape
Sleep 600ms

# Projects: a card per project. b reviews a branch in a worktree of its own.
Type "3"
Sleep 2.2s
Type "b"
Sleep 2.2s
Down
Sleep 1.2s
Enter
Sleep 2.2s
Enter
Sleep 5.5s
Sleep 3s
Escape
Sleep 600ms

# Enter opens the project's page: specs and their next step, worktrees.
Enter
Sleep 2.2s
Down
Down
Sleep 2.2s
Down
Sleep 1.8s

# MCP servers: Linear needs a sign-in, and `a` runs it.
Down
Down
Sleep 1s
Enter
Down
Down
Down
Sleep 1.2s
Type "a"
Sleep 5s
Sleep 2.2s
Escape
Sleep 400ms
Escape
Sleep 600ms

# Insights: security across every session, then where the limit went.
Type "4"
Sleep 2s
Down
Down
Down
Down
Sleep 3s
Up
Up
Up
Sleep 3s

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
