# Records demo/wrapped.gif: a week on one card (made-up data).
Output demo/wrapped.gif

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
Sleep 3s
Type "w"
Sleep 800ms
Show
Sleep 3s
Type "x"
Sleep 2.5s
Tab
Sleep 2.5s
Hide
Type "q"
