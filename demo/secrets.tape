# Records demo/secrets.gif: credentials left in transcripts, masked (made-up data).
Output demo/secrets.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# Build the made-up data and open claudash; wait until it has loaded.
Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear && claudash --view insights"
Enter
Wait+Screen@180s /1 Now/
Sleep 4s
Show
Sleep 800ms
Down
Down
Down
Down
Sleep 4s
Type "1"
Sleep 3.5s
Hide
Type "q"
