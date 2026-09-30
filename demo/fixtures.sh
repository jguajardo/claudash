#!/bin/sh
# Builds a made-up Claude Code environment in $DEMO (default /tmp/demo) so the
# README demo can be recorded without anyone's real sessions. Used by
# demo/claudash.tape; see demo/README.md.
set -eu

DEMO=${DEMO:-/tmp/demo}
rm -rf "$DEMO"
CLAUDE="$DEMO/.claude"
CACHE="$DEMO/.cache"
NOW=$(date +%s)

iso() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%S.000Z; }

# --- Projects and instruction files ------------------------------------------
for p in api-server web-app data-pipeline cli-tool; do
    mkdir -p "$DEMO/code/$p"
done
printf '# API server\n\nRun `cargo test` before committing.\n' > "$DEMO/code/api-server/CLAUDE.md"
printf '# Agents\n\nShared instructions for every coding agent.\n' > "$DEMO/code/api-server/AGENTS.md"
printf '# Web app\n\nUse pnpm, never npm.\n' > "$DEMO/code/web-app/AGENTS.md"

# --- Sessions -------------------------------------------------------------------
# session <id> <project> <branch> <title> <age-seconds> <requests> <context>
session() {
    id=$1 project=$2 branch=$3 title=$4 age=$5 requests=$6 context=$7
    dir="$CLAUDE/projects/-tmp-demo-code-$project"
    mkdir -p "$dir"
    file="$dir/$id.jsonl"
    last=$((NOW - age))
    printf '{"type":"user","cwd":"%s","gitBranch":"%s","timestamp":"%s","message":{"role":"user","content":"%s"}}\n' \
        "$DEMO/code/$project" "$branch" "$(iso "$last")" "$title" > "$file"
    printf '{"type":"ai-title","aiTitle":"%s","sessionId":"%s"}\n' "$title" "$id" >> "$file"
    i=1
    while [ "$i" -le "$requests" ]; do
        # Spread requests back over the session's lifetime, a few days at most.
        at=$((last - (requests - i) * 5400))
        read_tokens=$((context - 4000 + i * 37))
        printf '{"type":"assistant","requestId":"req_%s_%d","timestamp":"%s","message":{"model":"claude-opus-5-5","usage":{"input_tokens":%d,"cache_creation_input_tokens":%d,"cache_read_input_tokens":%d,"output_tokens":%d}}}\n' \
            "$id" "$i" "$(iso "$at")" $((3 + i % 5)) $((2500 + (i * 911) % 9000)) "$read_tokens" $((900 + (i * 577) % 4000)) >> "$file"
        i=$((i + 1))
    done
    printf '{"type":"cost-state","totalCostUSD":%d.%02d}\n' $((requests / 9)) $((requests * 7 % 100)) >> "$file"
    touch -d "@$last" "$file"
}

A=6b1f3c2e-0a41-4c1e-9d55-1f7a0c6b2a01
B=7c2e4d3f-1b52-4d2f-8e66-2a8b1d7c3b02
C=8d3f5e4a-2c63-4e3a-9f77-3b9c2e8d4c03
D=9e4a6f5b-3d74-4f4b-8a88-4cad3f9e5d04
E=af5b7a6c-4e85-4a5c-9b99-5dbe4aaf6e05
F=b06c8b7d-5f96-4b6d-8caa-6ecf5bb07f06
session "$A" api-server feat/oauth "Add OAuth login to the API" 40 60 412000
session "$B" web-app main "Redesign the pricing page" 150 44 238000
session "$C" api-server main "Fix flaky integration tests" 7300 30 164000
session "$D" data-pipeline main "Speed up the nightly ETL job" 190000 80 530000
session "$E" cli-tool release/2.0 "Write the v2.0 changelog" 520000 26 92000
session "$F" data-pipeline main "Migrate jobs to the new scheduler" 1300000 120 610000

# --- Skills, agents, hooks --------------------------------------------------------
skill() {
    mkdir -p "$1/$2"
    printf -- '---\nname: %s\ndescription: %s\n---\n' "$2" "$3" > "$1/$2/SKILL.md"
}
skill "$CLAUDE/skills" release-notes "Drafts release notes from merged pull requests."
skill "$CLAUDE/skills" sql-review "Reviews SQL migrations for locking and index problems."
mkdir -p "$CLAUDE/agents"
printf -- '---\nname: test-writer\ndescription: Writes focused unit tests next to the code they cover.\n---\n' > "$CLAUDE/agents/test-writer.md"
cat > "$CLAUDE/settings.json" <<EOF
{
  "hooks": {
    "PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "cargo fmt"}]}]
  }
}
EOF

# --- Plugins -------------------------------------------------------------------------
plugin() {
    dir="$DEMO/plugins/$1"
    mkdir -p "$dir/.claude-plugin"
    printf '{"name":"%s","description":"%s"}\n' "$1" "$2" > "$dir/.claude-plugin/plugin.json"
    skill "$dir/skills" "$3" "$4"
}
plugin superpowers "Brainstorming, planning and TDD workflows." brainstorming "Explores intent and design before building."
plugin context7 "Up-to-date library documentation over MCP." docs-lookup "Looks up current docs for a library."
plugin frontend-design "Distinctive, production-grade frontend design." frontend-design "Designs interfaces that don't look templated."

# --- A fake `claude` CLI ---------------------------------------------------------------
mkdir -p "$DEMO/bin"
cat > "$DEMO/bin/claude" <<EOF
#!/bin/sh
case "\$1 \$2" in
  "mcp list")
    sleep 1
    echo "Checking MCP server health…"
    echo
    echo "filesystem: npx -y @modelcontextprotocol/server-filesystem - ✔ Connected"
    echo "github: https://api.githubcopilot.com/mcp/ (HTTP) - ✔ Connected"
    echo "plugin:context7:context7: https://mcp.context7.com/mcp (HTTP) - ✔ Connected"
    echo "claude.ai Linear: https://mcp.linear.app/mcp - ! Needs authentication"
    echo "postgres: npx -y @modelcontextprotocol/server-postgres - ✘ Failed to connect — connection refused"
    ;;
  "agents --json")
    echo '[{"pid":4101,"sessionId":"$A","status":"busy"},{"pid":4102,"sessionId":"$B","status":"busy"}]'
    ;;
  "plugin list")
    printf '[{"id":"context7@claude-plugins-official","version":"2a8ad9f7","scope":"user","enabled":true,"installPath":"%s","mcpServers":{"context7":{}}},' "$DEMO/plugins/context7"
    printf '{"id":"frontend-design@claude-plugins-official","version":"1.2.0","scope":"user","enabled":false,"installPath":"%s"},' "$DEMO/plugins/frontend-design"
    printf '{"id":"superpowers@claude-plugins-official","version":"6.4.1","scope":"user","enabled":true,"installPath":"%s"}]\n' "$DEMO/plugins/superpowers"
    ;;
  "plugin details")
    case "\$3" in
      superpowers*) tokens="~840" ;;
      context7*) tokens="~120" ;;
      *) tokens="~310" ;;
    esac
    printf '%s\n\nComponent inventory\n  Skills (1)\n  Hooks (0)\n\nProjected token cost\n  Always-on:   %s tok   added to every session\n' "\$3" "\$tokens"
    ;;
  *) echo "demo claude: \$*" >&2; exit 1 ;;
esac
EOF
chmod +x "$DEMO/bin/claude"

# --- What the status line and hooks would have recorded ----------------------------------
mkdir -p "$CACHE/claudash/statusline" "$CACHE/claudash/state"
R5=$((NOW + 7200))
R7=$((NOW + 330000))
printf '{"session_id":"%s","context_window":{"context_window_size":1000000},"rate_limits":{"five_hour":{"used_percentage":64,"resets_at":%d},"seven_day":{"used_percentage":31,"resets_at":%d}},"prompt_cache":{"ttl":"1h","expires_at":%d,"hit_ratio":0.94,"misses":1,"last_miss_cause":{"causes":["tools_changed"]}}}\n' \
    "$A" "$R5" "$R7" $((NOW + 2400)) > "$CACHE/claudash/statusline/$A.json"
{
    printf '{"at":%d,"five_hour":[38.0,%d],"seven_day":[28.0,%d]}\n' $((NOW - 3000)) "$R5" "$R7"
    printf '{"at":%d,"five_hour":[64.0,%d],"seven_day":[31.0,%d]}\n' $((NOW - 30)) "$R5" "$R7"
} > "$CACHE/claudash/statusline/history.jsonl"
printf '{"event":"Notification","notification_type":"permission_prompt","at":%d}\n' "$NOW" > "$CACHE/claudash/state/$A.json"
printf '{"event":"UserPromptSubmit","notification_type":null,"at":%d}\n' "$NOW" > "$CACHE/claudash/state/$B.json"

# --- An MCP server log explaining the failure --------------------------------------------
log="$CACHE/claude-cli-nodejs/-tmp-demo-code-api-server/mcp-logs-postgres"
mkdir -p "$log"
{
    printf '{"debug":"Starting connection with timeout of 30000ms","timestamp":"%s"}\n' "$(iso $((NOW - 20)))"
    printf '{"debug":"Spawning npx -y @modelcontextprotocol/server-postgres","timestamp":"%s"}\n' "$(iso $((NOW - 20)))"
    printf '{"error":"connect ECONNREFUSED 127.0.0.1:5432","timestamp":"%s"}\n' "$(iso $((NOW - 19)))"
    printf '{"error":"Connection failed: is PostgreSQL running on localhost:5432?","timestamp":"%s"}\n' "$(iso $((NOW - 19)))"
} > "$log/$(date -u +%Y-%m-%dT%H-%M-%S-000Z).jsonl"
