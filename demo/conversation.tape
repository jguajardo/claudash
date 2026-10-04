# Records demo/conversation.gif: searching every conversation and reading one (made-up data).
Output demo/conversation.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# Build the made-up data and open claudash; wait until it has loaded.
Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear && claudash --view sessions"
Enter
Wait+Screen@180s /1 Now/
Sleep 4s
Show
Sleep 1.5s

# f searches the text of every conversation.
Type "f"
Sleep 700ms
Type "one cent"
Sleep 1.2s
Enter
Sleep 2.4s

# Enter opens the conversation at the match.
Enter
Sleep 3.5s

# o shows what each tool printed.
Type "o"
Sleep 3.5s
PageDown
Sleep 3s

# e exports it to Markdown.
Type "e"
Sleep 2.8s
Hide
Escape
Type "q"
