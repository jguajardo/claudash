# Records demo/claudash.gif, the README's hero, with made-up data (see demo/README.md).
Output demo/claudash.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear"
Enter
Sleep 1s
Type "claudash"
Enter
Sleep 2.5s
Show
Sleep 2s

# A session needs you: Enter shows exactly what it's asking to run.
Enter
Sleep 2.5s
Escape
Sleep 600ms

# A session the 5-hour limit stopped: Enter continues it now that it reset.
Tab
Tab
Sleep 1.2s
Enter
Sleep 2.8s
Escape
Sleep 300ms
Escape
Sleep 300ms

# Where the limits went, and whether the plan is worth it.
Type "4"
Sleep 400ms
Down
Sleep 2.2s
Down
Sleep 3.5s
Hide
Type "q"
