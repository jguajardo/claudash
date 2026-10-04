# Records demo/wrapped.gif: a week on one card (made-up data).
Output demo/wrapped.gif

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
Type "w"
Sleep 600ms
Show
Sleep 3s
Type "x"
Sleep 2.5s
Tab
Sleep 2.5s
Hide
Type "q"
