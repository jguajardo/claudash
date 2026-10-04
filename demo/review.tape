# Records demo/review.gif: reviewing a teammate's branch with Claude Code (made-up data).
Output demo/review.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# Build the made-up data and open claudash; wait until it has loaded.
Hide
Type "sh demo/fixtures.sh && export HOME=/tmp/demo CLAUDE_CONFIG_DIR=/tmp/demo/.claude XDG_CACHE_HOME=/tmp/demo/.cache PATH=/tmp/demo/bin:$PWD/target/x86_64-unknown-linux-musl/release:$PATH && clear && claudash --view projects"
Enter
Wait+Screen@180s /1 Now/
Sleep 4s
Show
Sleep 1.5s

# b: fetch and list the remote's branches.
Type "b"
Sleep 2.4s
Down
Sleep 1.6s

# Its size against the base, and how far the review goes.
Enter
Sleep 3s

# A static review runs in the background, in a worktree of its own.
Enter
Sleep 5.5s

# The findings, by file, line and severity.
Sleep 3s
Down
Sleep 1.2s

# One finding with the code around that line and the suggested change.
Enter
Sleep 4.5s
Hide
Escape
Escape
Type "q"
