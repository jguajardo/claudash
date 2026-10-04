# Demo recording

The GIFs in the main README are recorded with [VHS](https://github.com/charmbracelet/vhs) from made-up data, so they never show anyone's real sessions. The credentials in them are fake too, assembled from pieces so no script holds a whole token.

- `fixtures.sh` builds a fake Claude Code environment in `/tmp/demo`: a month of sessions with tool calls, costs and a pending edit, one the 5-hour limit stopped, a Max 5x account, instruction files, skills, plugins, status line and hook data, an MCP log, prompt history, subagents, made-up credentials and a force push, and a stand-in `claude` command that answers `--version`, `mcp list`, `agents --json` and `plugin` queries.
- `claudash.tape` records the README's hero (`claudash.gif`); `limits.tape`, `plan.tape`, `secrets.tape` and `wrapped.tape` record one feature each; `tour.tape` records the full tour (`tour.gif`, at 20 frames a second so rendering it fits in memory).

To regenerate them (Linux, Docker):

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
for tape in claudash limits plan secrets wrapped tour; do
    docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD":/vhs \
        ghcr.io/charmbracelet/vhs demo/$tape.tape
done
```

The static musl build runs inside the VHS container whatever glibc the host has.
