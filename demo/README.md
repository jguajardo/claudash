# Demo recording

`claudash.gif` in the main README is recorded with [VHS](https://github.com/charmbracelet/vhs) from made-up data, so it never shows anyone's real sessions.

- `fixtures.sh` builds a fake Claude Code environment in `/tmp/demo`: sessions, instruction files, skills, plugins, status line and hook data, an MCP log, and a stand-in `claude` command that answers `mcp list`, `agents --json` and `plugin` queries.
- `claudash.tape` runs claudash against it and records the GIF.

To regenerate it (Linux, Docker):

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD":/vhs \
    ghcr.io/charmbracelet/vhs demo/claudash.tape
```

The static musl build runs inside the VHS container whatever glibc the host has.
