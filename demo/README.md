# Demo recording

The GIFs in the main README are recorded with [VHS](https://github.com/charmbracelet/vhs) from made-up data, so they never show anyone's real sessions. The credentials in them are fake too, assembled from pieces so no script holds a whole token.

- `fixtures.sh` builds a fake Claude Code environment in `/tmp/demo`:
  - four projects that are real git repositories, each with a remote; the API server has branches to review, commits to push, a worktree with uncommitted work and three OpenSpec changes at different stages;
  - a month of sessions with tool calls, costs and a pending edit, one the 5-hour limit stopped, and one short, realistic conversation to read and search;
  - a Max 5x account, instruction files, skills, plugins, status line and hook data, an MCP log, prompt history, subagents, made-up credentials and a force push;
  - a stand-in `claude` command that answers `--version`, `mcp list`, `mcp login`, `agents --json` and `plugin` queries, and returns a canned review for `claude -p`.
- `Dockerfile` adds git to the VHS image, which `fixtures.sh` needs for the repositories (it also uses the image's python3).
- One tape per GIF:

  | Tape | Shows |
  | --- | --- |
  | `claudash.tape` | The README's hero: what a waiting session asks, then a project's page |
  | `project.tape` | A project's page: specs, MCP servers and a sign-in, plugins |
  | `review.tape` | A review of a branch, from the list of branches to one finding |
  | `conversation.tape` | Searching every conversation and reading one with its tool output |
  | `secrets.tape` | Credentials left in transcripts, and risky commands |
  | `limits.tape` | Where the limit went, and what it cost |
  | `plan.tape` | `claudash plan --share` |
  | `wrapped.tape` | The wrapped card |
  | `tour.tape` | The full tour (at 20 frames a second, so rendering it fits in memory) |

To regenerate them (Linux, Docker):

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
docker build -t claudash-vhs demo
for tape in demo/*.tape; do
    docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD":/vhs claudash-vhs "$tape"
done
```

The static musl build runs inside the VHS container whatever glibc the host has. The tapes wait for claudash to appear on screen before they start recording, so a slow machine only makes them take longer.
