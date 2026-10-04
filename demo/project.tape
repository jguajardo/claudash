# Records demo/project.gif: a project's page and an MCP sign-in (made-up data).
Output demo/project.gif

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

# The API server's page: what Claude Code loads there, git state, sessions.
Enter
Sleep 2.6s

# Specs: each change's progress and the command for its next step.
Down
Down
Sleep 2.6s

# MCP servers. Linear needs you to sign in: `a` runs `claude mcp login`.
Down
Down
Down
Sleep 1.2s
Enter
Down
Down
Down
Sleep 1.4s
Type "a"
Sleep 5s
Sleep 2.6s

# Skills and plugins: the Plugins tab shows what each costs you per session.
Escape
Down
Sleep 1.4s
Enter
Tab
Tab
Tab
Tab
Sleep 3.2s
Hide
Type "q"
