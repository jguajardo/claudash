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

# --- The API server's code, spec-driven changes and commands -------------------
api="$DEMO/code/api-server"
stage="$DEMO/.stage"
mkdir -p "$api/src" "$api/tests" "$api/.claude/commands/opsx" "$stage"
printf '[package]\nname = "api-server"\nversion = "0.4.0"\nedition = "2024"\n' > "$api/Cargo.toml"
printf 'target/\n.claude/worktrees/\n' > "$api/.gitignore"
cat > "$api/src/lib.rs" <<'EOF'
pub mod auth;

pub fn handle_health() -> &'static str {
    "ok"
}
EOF
cat > "$api/src/auth.rs" <<'EOF'
use crate::db;

pub fn login(user: &str, password: &str) -> Result<Session> {
    let row = db.query(&format!("SELECT id FROM users WHERE name = '{user}'"))?;
    Session::start(row.id)
}
EOF
cat > "$api/tests/login.rs" <<'EOF'
#[test]
fn logs_in_with_the_right_password() {
    assert!(api_server::auth::login("ada", "correct horse").is_ok());
}
EOF
for command in apply archive continue; do
    printf -- '---\ndescription: OpenSpec: %s a change\n---\nRun the %s step for openspec/changes/$ARGUMENTS.\n' \
        "$command" "$command" > "$api/.claude/commands/opsx/$command.md"
done

# OpenSpec: one change being implemented, one finished, one still being planned.
change="$api/openspec/changes/add-oauth-login"
mkdir -p "$change"
printf '# Add OAuth login\n\n## Why\n\nCustomers ask to sign in with GitHub and Google instead of a password.\n\n## What changes\n\nAn OAuth callback, linked accounts and sessions with a hashed token.\n' > "$change/proposal.md"
printf '# Design\n\nThe callback exchanges the code for a token, then creates or links the user.\nSessions keep only a hash of the token.\n' > "$change/design.md"
cat > "$change/tasks.md" <<'EOF'
## 1. Provider setup
- [x] 1.1 Register the GitHub and Google OAuth apps
- [x] 1.2 Keep the client secrets in the vault

## 2. Callback
- [x] 2.1 Add the /oauth/callback route
- [x] 2.2 Exchange the code for a token
- [x] 2.3 Create or link the user

## 3. Sessions
- [ ] 3.1 Issue a session with a hashed token
- [ ] 3.2 Refresh tokens before they expire
- [ ] 3.3 Integration tests for both providers
EOF
change="$api/openspec/changes/audit-log-export"
mkdir -p "$change"
printf '# Export the audit log\n\nAdmins need the audit log as CSV for their compliance reviews.\n' > "$change/proposal.md"
printf -- '- [x] 1.1 Add GET /audit/export\n- [x] 1.2 Stream the rows as CSV\n- [x] 1.3 Limit it to admins\n- [x] 1.4 Document it\n' > "$change/tasks.md"
change="$api/openspec/changes/rate-limit-public-api"
mkdir -p "$change"
printf '# Rate limit the public API\n\nOne client took the API down twice last month. Limit requests per API key.\n' > "$change/proposal.md"

# What a teammate's branch adds, for the branch review.
cat > "$stage/rate_limit.rs" <<'EOF'
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Requests allowed per window, per API key.
pub struct RateLimiter {
    window: Duration,
    limit: u32,
    hits: HashMap<String, Vec<Instant>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { window, limit, hits: HashMap::new() }
    }

    /// Whether the request is allowed, recording it if so.
    pub fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let hits = self.hits.entry(key.to_string()).or_default();
        hits.retain(|t| now.duration_since(*t) < self.window);
        if hits.len() as u32 > self.limit {
            return false;
        }
        hits.push(now);
        true
    }
}

pub fn key_from_header(header: &str) -> String {
    header.split(' ').nth(1).unwrap().to_string()
}
EOF
cat > "$stage/rate_limit_test.rs" <<'EOF'
use api_server::rate_limit::RateLimiter;
use std::time::Duration;

#[test]
fn allows_again_after_the_window() {
    let mut limiter = RateLimiter::new(2, Duration::from_secs(1));
    assert!(limiter.allow("key"));
    assert!(limiter.allow("key"));
    assert!(!limiter.allow("key"));

    // Wait for the whole window before trying again.
    std::thread::sleep(Duration::from_secs(1));
    assert!(limiter.allow("key"));
}
EOF

# --- Git: real repositories with a remote, branches to review and a worktree ---
# Without git the projects show as plain folders and the rest still works.
if command -v git > /dev/null 2>&1; then
    # g <dir> <author> <age in seconds> <git arguments…>
    g() {
        gdir=$1 gauthor=$2 gwhen="@$((NOW - $3)) +0000"
        shift 3
        GIT_AUTHOR_DATE="$gwhen" GIT_COMMITTER_DATE="$gwhen" git -C "$gdir" \
            -c user.name="$gauthor" -c user.email=dev@example.com \
            -c commit.gpgsign=false -c init.defaultBranch=main \
            -c advice.detachedHead=false "$@"
    }
    # repo <project> <author> <age> <message>: commit what's there, add a remote
    repo() {
        rdir="$DEMO/code/$1"
        g "$rdir" "$2" "$3" init -q
        g "$rdir" "$2" "$3" add -A
        g "$rdir" "$2" "$3" commit -q --allow-empty -m "$4"
        git clone -q --bare "$rdir" "$DEMO/remotes/$1.git"
        g "$rdir" "$2" "$3" remote add origin "$DEMO/remotes/$1.git"
        g "$rdir" "$2" "$3" fetch -q origin
        g "$rdir" "$2" "$3" branch -q --set-upstream-to=origin/main main
    }
    mkdir -p "$DEMO/remotes"
    # The identity git uses when claudash runs it (fetch, worktrees).
    printf '[user]\n\tname = Dana Kim\n\temail = dana@example.com\n' > "$DEMO/.gitconfig"
    repo api-server "Dana Kim" 2600000 "API server with password login"
    repo web-app "Mia Torres" 1900000 "Pricing page scaffold"
    repo data-pipeline "Dana Kim" 3100000 "Nightly ETL job"
    repo cli-tool "Sam Ortiz" 2200000 "Importer and config loader"

    # Teammates' branches on the remote, newest first in the branch picker.
    g "$api" "Sam Ortiz" 432000 checkout -q -b chore/bump-deps main
    printf '[package]\nname = "api-server"\nversion = "0.4.1"\nedition = "2024"\n' > "$api/Cargo.toml"
    g "$api" "Sam Ortiz" 432000 commit -q -am "Bump dependencies for the 0.4.1 release"
    g "$api" "Sam Ortiz" 432000 push -q origin chore/bump-deps
    g "$api" "Priya Nair" 172800 checkout -q -b fix/session-expiry main
    printf '\npub const SESSION_HOURS: u64 = 12;\n' >> "$api/src/auth.rs"
    g "$api" "Priya Nair" 172800 commit -q -am "Expire sessions after twelve hours"
    g "$api" "Priya Nair" 172800 push -q origin fix/session-expiry
    g "$api" "Sam Ortiz" 14400 checkout -q -b feat/rate-limit main
    cp "$stage/rate_limit.rs" "$api/src/rate_limit.rs"
    printf 'pub mod auth;\npub mod rate_limit;\n\npub fn handle_health() -> &'"'"'static str {\n    "ok"\n}\n' > "$api/src/lib.rs"
    g "$api" "Sam Ortiz" 14400 add -A
    g "$api" "Sam Ortiz" 14400 commit -q -m "Add a sliding-window rate limiter per API key"
    cp "$stage/rate_limit_test.rs" "$api/tests/rate_limit.rs"
    g "$api" "Sam Ortiz" 10800 add -A
    g "$api" "Sam Ortiz" 10800 commit -q -m "Read the API key from the Authorization header"
    g "$api" "Sam Ortiz" 10800 push -q origin feat/rate-limit

    # Your own branch: two commits pushed, one not yet, and work in progress.
    g "$api" "Dana Kim" 90000 checkout -q -b feat/oauth main
    printf 'pub fn callback(code: &str) -> Result<Session> {\n    todo!("exchange {code} for a token")\n}\n' > "$api/src/oauth.rs"
    g "$api" "Dana Kim" 90000 add -A
    g "$api" "Dana Kim" 90000 commit -q -m "Add the OAuth callback route"
    printf '\npub fn link_user(provider: &str, id: &str) {}\n' >> "$api/src/oauth.rs"
    g "$api" "Dana Kim" 86400 commit -q -am "Create or link the user after the callback"
    g "$api" "Dana Kim" 86400 push -q -u origin feat/oauth
    printf '\npub fn refresh(session: &Session) {}\n' >> "$api/src/oauth.rs"
    g "$api" "Dana Kim" 3600 commit -q -am "Refresh tokens before they expire"
    printf '\n#[test]\nfn logs_in_with_github() {}\n' >> "$api/tests/login.rs"
    for branch in chore/bump-deps fix/session-expiry feat/rate-limit; do
        g "$api" "Dana Kim" 3600 branch -q -D "$branch"
    done

    # A worktree Claude Code made for a spike, with work nobody committed.
    g "$api" "Dana Kim" 260000 worktree add -q -b spike/pagination "$api/.claude/worktrees/pagination" main
    printf 'pub fn page(offset: usize, limit: usize) {}\n' > "$api/.claude/worktrees/pagination/src/pagination.rs"

    printf '<main>three tiers, annual toggle</main>\n' > "$DEMO/code/web-app/pricing.html"
    g "$DEMO/code/cli-tool" "Sam Ortiz" 520000 checkout -q -b release/2.0
fi

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
        at=$((last - (requests - i) * 120))
        # Context grows with each request; long sessions compact halfway.
        half=$((requests / 2))
        if [ "$requests" -ge 60 ] && [ "$i" -gt "$half" ]; then
            [ "$i" -eq $((half + 1)) ] &&
                printf '{"type":"system","subtype":"compact_boundary","timestamp":"%s"}\n' "$(iso "$at")" >> "$file"
            read_tokens=$((30000 + (context - 30000) * (i - half) / half))
        else
            read_tokens=$((24000 + (context - 24000) * i / requests))
        fi
        case $((i % 6)) in
            0) tool='"name":"Edit","input":{"file_path":"'"$DEMO/code/$project"'/src/auth.rs"}' ;;
            1) tool='"name":"Bash","input":{"command":"cargo test --workspace"}' ;;
            2) tool='"name":"Read","input":{"file_path":"'"$DEMO/code/$project"'/src/lib.rs"}' ;;
            3) tool='"name":"Grep","input":{"pattern":"fn handle_"}' ;;
            4) tool='"name":"mcp__github__search_issues","input":{"query":"is:open label:bug"}' ;;
            *) tool='"name":"Write","input":{"file_path":"'"$DEMO/code/$project"'/tests/login.rs"}' ;;
        esac
        printf '{"type":"assistant","requestId":"req_%s_%d","timestamp":"%s","message":{"model":"claude-opus-5-5","usage":{"input_tokens":%d,"cache_creation_input_tokens":%d,"cache_read_input_tokens":%d,"output_tokens":%d},"content":[{"type":"tool_use","id":"tu_%d",%s}]}}\n' \
            "$id" "$i" "$(iso "$at")" $((3 + i % 5)) $((2500 + (i * 911) % 9000)) "$read_tokens" $((900 + (i * 577) % 4000)) "$i" "$tool" >> "$file"
        # Every seventh call fails, like a test run that doesn't pass yet.
        if [ $((i % 7)) -eq 1 ]; then
            printf '{"type":"user","timestamp":"%s","message":{"content":[{"type":"tool_result","tool_use_id":"tu_%d","is_error":true,"content":"test failed"}]}}\n' "$(iso "$at")" "$i" >> "$file"
        fi
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
# A month of earlier work, so the plan and usage views have history.
n=0
for entry in \
    "api-server|Add rate limiting to the public API" \
    "web-app|Move the dashboard to server components" \
    "data-pipeline|Backfill the events table" \
    "cli-tool|Add shell completions" \
    "api-server|Upgrade to the new ORM" \
    "web-app|Fix the checkout race condition" \
    "data-pipeline|Partition the warehouse tables" \
    "api-server|Write the webhook retry worker" \
    "web-app|Add end-to-end tests for signup" \
    "cli-tool|Rewrite the config loader" \
    "data-pipeline|Cut the Spark job's memory use" \
    "api-server|Audit the auth middleware"; do
    n=$((n + 1))
    project=${entry%%|*} title=${entry#*|}
    session "$(printf 'd%07x-0000-4000-8000-%012x' "$n" "$n")" "$project" main "$title" $((n * 190000 + 260000)) 170 640000
done
G=c17d9c8e-6aa7-4c7e-9dbb-7fd06cc18a07
session "$G" cli-tool main "Port the importer to async" 10800 90 720000

# The 5-hour limit stopped that one; it has reset since, so Now offers to
# continue it, and Is your plan worth it calibrates on it.
printf '{"type":"assistant","timestamp":"%s","error":"rate_limit","isApiErrorMessage":true,"quotaLimits":{"status":"rejected","resetsAt":%d,"rateLimitType":"five_hour"},"message":{"model":"<synthetic>","content":[{"type":"text","text":"You'"'"'ve hit your session limit"}]}}\n' \
    "$(iso $((NOW - 10790)))" $((NOW - 600)) >> "$CLAUDE/projects/-tmp-demo-code-cli-tool/$G.jsonl"
touch -d "@$((NOW - 10790))" "$CLAUDE/projects/-tmp-demo-code-cli-tool/$G.jsonl"

# A session with a real back-and-forth, for the conversation view and the search.
H=d28e0d9f-7bb8-4d8f-8ecc-80e17dd29b08
if command -v python3 > /dev/null 2>&1; then
    python3 - "$CLAUDE/projects/-tmp-demo-code-web-app/$H.jsonl" "$DEMO/code/web-app" "$NOW" "$H" <<'PY'
import datetime
import json
import sys

path, cwd, now, sid = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
clock = now - 3 * 3600
records = [{"type": "ai-title", "aiTitle": "Fix the cart total rounding bug", "sessionId": sid}]
requests = 0


def stamp(seconds):
    global clock
    clock += seconds
    when = datetime.datetime.fromtimestamp(clock, datetime.timezone.utc)
    return when.strftime("%Y-%m-%dT%H:%M:%S.000Z")


def user(text):
    records.append({"type": "user", "cwd": cwd, "gitBranch": "main", "sessionId": sid,
                    "timestamp": stamp(90), "message": {"role": "user", "content": text}})


def reply(*blocks):
    global requests
    requests += 1
    usage = {"input_tokens": 4, "cache_creation_input_tokens": 2800,
             "cache_read_input_tokens": 38000 + requests * 2600, "output_tokens": 420}
    records.append({"type": "assistant", "requestId": f"req_cart_{requests}", "sessionId": sid,
                    "timestamp": stamp(18),
                    "message": {"role": "assistant", "model": "claude-opus-5-5", "usage": usage,
                                "content": list(blocks)}})


def text(words):
    return {"type": "text", "text": words}


def call(call_id, name, **arguments):
    return {"type": "tool_use", "id": call_id, "name": name, "input": arguments}


def result(call_id, output, failed=False):
    block = {"type": "tool_result", "tool_use_id": call_id, "content": output, "is_error": failed}
    records.append({"type": "user", "sessionId": sid, "timestamp": stamp(5),
                    "message": {"role": "user", "content": [block]}})


user("The cart total is off by one cent for some orders. Find out why and fix it.")
reply(text("I'll reproduce it with the order from the bug report, then look at how the total is computed."),
      call("c1", "Grep", pattern="function cartTotal"))
result("c1", "src/cart.ts:42:export function cartTotal(items: Item[]): number {")
reply(call("c2", "Read", file_path=f"{cwd}/src/cart.ts"))
result("c2", "42  export function cartTotal(items: Item[]): number {\n"
             "43    const net = items.reduce((sum, item) => sum + item.price * item.quantity, 0);\n"
             "44    return Math.round(net * (1 + TAX_RATE) * 100) / 100;\n"
             "45  }")
reply(text("The total is summed in floating point and rounded once at the end, but the invoice "
           "rounds the tax of every line. For three items at 19.99 with 7.25% tax the two differ "
           "by one cent."),
      call("c3", "Bash", command="pnpm test cart", description="Run the cart tests"))
result("c3", " FAIL  tests/cart.test.ts > total matches the invoice\n"
             "   expected 64.32, received 64.31\n"
             " Tests  1 failed | 5 passed (6)", failed=True)
reply(call("c4", "Edit", file_path=f"{cwd}/src/cart.ts",
           old_string="  const net = items.reduce((sum, item) => sum + item.price * item.quantity, 0);\n"
                      "  return Math.round(net * (1 + TAX_RATE) * 100) / 100;",
           new_string="  // Work in integer cents and round the tax of each line, like the invoice.\n"
                      "  const cents = items.reduce((sum, item) => sum + lineCents(item), 0);\n"
                      "  return cents / 100;"))
result("c4", f"The file {cwd}/src/cart.ts has been updated.")
reply(call("c5", "Bash", command="pnpm test cart", description="Run the cart tests again"))
result("c5", " ✓ tests/cart.test.ts (6 tests) 41ms\n Tests  6 passed (6)")
reply(text("Fixed. Totals are now computed in integer cents and the tax is rounded per line, "
           "which is what the invoice does. All six cart tests pass."))
user("Add a regression test with the order from the bug report.")
reply(call("c6", "Write", file_path=f"{cwd}/tests/cart.rounding.test.ts",
           content="import { cartTotal } from '../src/cart';\n\n"
                   "test('three items at 19.99 match the invoice', () => {\n"
                   "  const items = Array(3).fill({ price: 19.99, quantity: 1 });\n"
                   "  expect(cartTotal(items)).toBe(64.32);\n"
                   "});\n"))
result("c6", f"File created at {cwd}/tests/cart.rounding.test.ts")
reply(call("c7", "Bash", command="pnpm test cart", description="Run the cart tests with the new one"))
result("c7", " ✓ tests/cart.test.ts (6 tests) 38ms\n ✓ tests/cart.rounding.test.ts (1 test) 9ms\n"
             " Tests  7 passed (7)")
reply(text("Added tests/cart.rounding.test.ts with the three 19.99 items from the report. "
           "It fails on the old code and passes now."))

with open(path, "w", encoding="utf-8") as out:
    for record in records:
        out.write(json.dumps(record, ensure_ascii=False) + "\n")
PY
    touch -d "@$((NOW - 10000))" "$CLAUDE/projects/-tmp-demo-code-web-app/$H.jsonl"
fi

# The account's plan, where Claude Code keeps it.
printf '{"oauthAccount":{"organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_5x"}}\n' > "$CLAUDE/.claude.json"

# Made-up credentials that ended up in transcripts, for the security views.
# Assembled from pieces so this script never contains a whole token.
FAKE_ANTHROPIC="sk-ant-""api03-DEMOonlyNOTaREALkey000000000000"
FAKE_GITHUB="ghp_""DEMOonlyNOTaREALtoken0000000000000000"
dir="$CLAUDE/projects/-tmp-demo-code-web-app"
printf '{"type":"user","timestamp":"%s","message":{"content":"the deploy fails, my key is %s, can you check?"}}\n' \
    "$(iso $((NOW - 400)))" "$FAKE_ANTHROPIC" >> "$dir/$B.jsonl"
dir="$CLAUDE/projects/-tmp-demo-code-data-pipeline"
printf '{"type":"user","timestamp":"%s","message":{"content":[{"type":"tool_result","tool_use_id":"x1","content":"GITHUB_TOKEN=%s"}]}}\n' \
    "$(iso $((NOW - 190000)))" "$FAKE_GITHUB" >> "$dir/$D.jsonl"
printf '{"type":"assistant","requestId":"risk_1","timestamp":"%s","cwd":"%s","message":{"model":"claude-opus-5-5","usage":{"input_tokens":3,"output_tokens":40},"content":[{"type":"tool_use","id":"r1","name":"Bash","input":{"command":"git push --force origin main"}}]}}\n' \
    "$(iso $((NOW - 3000)))" "$DEMO/code/web-app" >> "$CLAUDE/projects/-tmp-demo-code-web-app/$B.jsonl"
touch -d "@$((NOW - 150))" "$CLAUDE/projects/-tmp-demo-code-web-app/$B.jsonl"

# What the OAuth session is waiting on: an edit, shown in full in Now.
printf '%s\n' '{"type":"assistant","requestId":"ask_1","timestamp":"'"$(iso $((NOW - 40)))"'","message":{"model":"claude-opus-5-5","usage":{"input_tokens":3,"output_tokens":120},"content":[{"type":"tool_use","id":"ask1","name":"Edit","input":{"file_path":"'"$DEMO"'/code/api-server/src/auth.rs","old_string":"pub fn login(user: &str, password: &str) -> Result<Session> {\n    let row = db.query(&format!(\"SELECT id FROM users WHERE name = '"'"'{user}'"'"'\"))?;","new_string":"pub fn login(user: &str, password: &str) -> Result<Session> {\n    let row = db.query(\"SELECT id, hash FROM users WHERE name = ?\", &[user])?;\n    verify_password(password, &row.hash)?;"}}]}}' \
    >> "$CLAUDE/projects/-tmp-demo-code-api-server/$A.jsonl"
touch -d "@$((NOW - 40))" "$CLAUDE/projects/-tmp-demo-code-api-server/$A.jsonl"

# Subagents of the OAuth session.
sub_dir="$CLAUDE/projects/-tmp-demo-code-api-server/$A/subagents"
mkdir -p "$sub_dir"
n=0
for agent in "Explore|Find every place that issues sessions|48000" \
    "test-writer|Write tests for the OAuth callback|126000" \
    "general-purpose|Review the token refresh flow|83000"; do
    kind=${agent%%|*} rest=${agent#*|} what=${rest%%|*} tokens=${rest#*|}
    n=$((n + 1))
    printf '{"type":"assistant","isSidechain":true,"requestId":"sub_%d","timestamp":"%s","message":{"model":"claude-sonnet-5-5","usage":{"input_tokens":12,"cache_creation_input_tokens":4000,"cache_read_input_tokens":%d,"output_tokens":2100},"content":[{"type":"tool_use","id":"s%d","name":"Grep","input":{"pattern":"session"}}]}}\n' \
        "$n" "$(iso $((NOW - 600 * n)))" "$tokens" "$n" > "$sub_dir/agent-$n.jsonl"
    printf '{"agentType":"%s","description":"%s","model":"sonnet"}\n' "$kind" "$what" > "$sub_dir/agent-$n.meta.json"
done

# claudash has been opened before: no welcome popup.
mkdir -p "$DEMO/.local/share/claudash"
: > "$DEMO/.local/share/claudash/welcomed"

# --- Prompt history ------------------------------------------------------------------
{
    n=0
    for entry in \
        "api-server|$A|Add OAuth login to the API with GitHub and Google" \
        "api-server|$A|commit the OAuth changes with a short message" \
        "web-app|$B|Redesign the pricing page, three tiers, annual toggle" \
        "web-app|$H|The cart total is off by one cent for some orders. Find out why and fix it." \
        "api-server|$C|Fix the flaky integration tests in tests/db.rs" \
        "api-server|$C|commit only the test fixes" \
        "data-pipeline|$D|Profile the nightly ETL job and speed up the slowest step" \
        "cli-tool|$E|Write the v2.0 changelog from the merged pull requests"; do
        project=${entry%%|*} rest=${entry#*|} sid=${rest%%|*} text=${rest#*|}
        n=$((n + 1))
        printf '{"display":"%s","pastedContents":{},"project":"%s","sessionId":"%s","timestamp":%d000}\n' \
            "$text" "$DEMO/code/$project" "$sid" $((NOW - 90000 + n * 12000))
    done
} > "$CLAUDE/history.jsonl"

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
  "--version ")
    echo "2.1.285 (Claude Code)"
    ;;
  "mcp list")
    sleep 1
    echo "Checking MCP server health…"
    echo
    echo "filesystem: npx -y @modelcontextprotocol/server-filesystem - ✔ Connected"
    echo "github: https://api.githubcopilot.com/mcp/ (HTTP) - ✔ Connected"
    echo "plugin:context7:context7: https://mcp.context7.com/mcp (HTTP) - ✔ Connected"
    if [ -e "$DEMO/.signed-in" ]; then
      echo "claude.ai Linear: https://mcp.linear.app/mcp - ✔ Connected"
    else
      echo "claude.ai Linear: https://mcp.linear.app/mcp - ! Needs authentication"
    fi
    echo "postgres: npx -y @modelcontextprotocol/server-postgres - ✘ Failed to connect — connection refused"
    ;;
  "mcp login")
    echo "Opening your browser to sign in to \$3…"
    sleep 2
    : > "$DEMO/.signed-in"
    echo "✔ Signed in to \$3"
    sleep 1
    ;;
  "-p --session-id")
    # A static branch review: reads the prompt, takes a while, answers with findings.
    cat > /dev/null
    sleep 3
    cat "$DEMO/review.json"
    ;;
  "agents --json")
    echo '[{"pid":4101,"kind":"interactive","sessionId":"$A","status":"waiting","waitingFor":"permission prompt","cwd":"$DEMO/code/api-server"},{"pid":4102,"kind":"interactive","sessionId":"$B","status":"busy","cwd":"$DEMO/code/web-app"},{"pid":4103,"kind":"background","id":"bg7k2","sessionId":"$C","state":"working","status":"busy","name":"Fix flaky integration tests","cwd":"$DEMO/code/api-server"}]'
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

# What the review of feat/rate-limit finds.
cat > "$DEMO/review.json" <<'EOF'
{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"summary":"Adds a sliding-window rate limiter per API key. The approach is sound, but the limit is off by one and a malformed Authorization header panics; fix both before merging.","findings":[{"file":"src/rate_limit.rs","line":21,"severity":"high","comment":"Off by one: `>` lets limit + 1 requests through in each window. With a limit of 100, the 101st request is still allowed.","suggestion":"if hits.len() as u32 >= self.limit {"},{"file":"src/rate_limit.rs","line":30,"severity":"high","comment":"`unwrap()` panics on a header with no space, such as `Authorization: abc`, so one malformed request takes the worker down. Return an Option and answer 401.","suggestion":"header.split_once(' ').map(|(_, key)| key.to_string())"},{"file":"src/rate_limit.rs","line":8,"severity":"medium","comment":"`hits` never drops keys that stopped sending requests, so the map grows with every API key ever seen. Remove a key when its list is empty after `retain`."},{"file":"tests/rate_limit.rs","line":12,"severity":"low","comment":"The test sleeps for the whole window. Inject the clock so it runs in milliseconds and can cover the boundary."}]}}
EOF

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
