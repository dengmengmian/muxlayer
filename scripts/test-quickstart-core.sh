#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${AGENTGATE_QUICKSTART_BIN:-$ROOT/src-tauri/target/debug/agentgate-serve}"
TOKEN="${AGENTGATE_QUICKSTART_TOKEN:-ag_local_quickstart_smoke_token_1234567890abcdef}"
USE_REAL="${AGENTGATE_SMOKE_REAL:-0}"
SMOKE_BASE_URL="${AGENTGATE_SMOKE_BASE_URL:-}"
SMOKE_API_KEY="${AGENTGATE_SMOKE_API_KEY:-}"
SMOKE_MODEL="${AGENTGATE_SMOKE_MODEL:-mock-model}"
SMOKE_PROVIDER_NAME="${AGENTGATE_SMOKE_PROVIDER_NAME:-Quickstart Mock}"
SMOKE_PROVIDER_TYPE="${AGENTGATE_SMOKE_PROVIDER_TYPE:-custom_openai_compatible}"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/agentgate-quickstart.XXXXXX")"
mock_pid=""
gateway_pid=""

cleanup() {
  if [[ -n "$gateway_pid" ]]; then
    kill "$gateway_pid" >/dev/null 2>&1 || true
    wait "$gateway_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$mock_pid" ]]; then
    kill "$mock_pid" >/dev/null 2>&1 || true
    wait "$mock_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 2
  fi
}

need curl
need node

if [[ "$USE_REAL" == "1" ]]; then
  if [[ -z "$SMOKE_BASE_URL" || -z "$SMOKE_API_KEY" || -z "$SMOKE_MODEL" ]]; then
    echo "Real smoke mode requires AGENTGATE_SMOKE_BASE_URL, AGENTGATE_SMOKE_API_KEY, and AGENTGATE_SMOKE_MODEL." >&2
    exit 2
  fi
  SMOKE_PROVIDER_NAME="${AGENTGATE_SMOKE_PROVIDER_NAME:-Quickstart Real}"
fi

free_port() {
  node -e 'const net=require("node:net"); const s=net.createServer(); s.listen(0, "127.0.0.1", () => { console.log(s.address().port); s.close(); });'
}

if [[ ! -x "$BIN" || "${AGENTGATE_QUICKSTART_BUILD:-1}" == "1" ]]; then
  need cargo
  echo "==> Building headless agentgate-serve"
  cargo build \
    --manifest-path "$ROOT/src-tauri/Cargo.toml" \
    --no-default-features \
    --features cli \
    --bin agentgate-serve
fi

mock_port=""
gateway_port="$(free_port)"
db_dir="$tmp_dir/db"
mkdir -p "$db_dir"

run_agentgate_cli() {
  local stdout_file="$tmp_dir/agentgate-cli.out"
  local stderr_file="$tmp_dir/agentgate-cli.err"
  : >"$stdout_file"
  : >"$stderr_file"
  if ! AGENTGATE_DB_PATH="$db_dir" AGENTGATE_TOKEN="$TOKEN" "$BIN" "$@" >"$stdout_file" 2>"$stderr_file"; then
    cat "$stdout_file" >&2
    cat "$stderr_file" >&2
    exit 1
  fi
  if [[ "${AGENTGATE_QUICKSTART_VERBOSE:-0}" == "1" ]]; then
    cat "$stdout_file"
    cat "$stderr_file" >&2
  else
    grep -v '"database is locked"' "$stdout_file" || true
  fi
}

if [[ "$USE_REAL" != "1" ]]; then
  mock_port="$(free_port)"
  SMOKE_BASE_URL="http://127.0.0.1:${mock_port}/v1"
  SMOKE_API_KEY="mock-upstream-key"

  # Local smoke must hit the in-process mock directly. Some developer
  # environments inject a system HTTP proxy that reqwest honors even when the
  # shell does not show HTTP_PROXY, so make localhost bypass explicit.
  export NO_PROXY="${NO_PROXY:+${NO_PROXY},}127.0.0.1,localhost"
  export no_proxy="${no_proxy:+${no_proxy},}127.0.0.1,localhost"

  cat >"$tmp_dir/mock-openai-compatible.mjs" <<'EOF'
import http from "node:http";

const port = Number(process.env.MOCK_PORT);
const server = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => {
    body += chunk;
  });
  req.on("end", () => {
    if (req.method === "GET" && req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true }));
      return;
    }

    if (req.method === "POST" && req.url === "/v1/chat/completions") {
      const auth = req.headers.authorization ?? "";
      if (!auth.startsWith("Bearer ")) {
        res.writeHead(401, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { message: "missing upstream bearer token" } }));
        return;
      }

      const request = JSON.parse(body || "{}");
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "chatcmpl-quickstart-smoke",
          object: "chat.completion",
          created: Math.floor(Date.now() / 1000),
          model: request.model ?? "mock-model",
          choices: [
            {
              index: 0,
              message: {
                role: "assistant",
                content: "AgentGate quickstart smoke passed.",
              },
              finish_reason: "stop",
            },
          ],
          usage: {
            prompt_tokens: 8,
            completion_tokens: 6,
            total_tokens: 14,
          },
        }),
      );
      return;
    }

    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: { message: `unexpected ${req.method} ${req.url}` } }));
  });
});

server.listen(port, "127.0.0.1", () => {
  console.error(`mock upstream listening on ${port}`);
});
EOF

  echo "==> Starting mock OpenAI-compatible upstream"
  MOCK_PORT="$mock_port" \
    node "$tmp_dir/mock-openai-compatible.mjs" >"$tmp_dir/mock.out" 2>"$tmp_dir/mock.err" &
  mock_pid="$!"

  for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:${mock_port}/health" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  curl -fsS "http://127.0.0.1:${mock_port}/health" >/dev/null
fi

echo "==> Adding provider through the CLI"
run_agentgate_cli provider-add \
  --type "$SMOKE_PROVIDER_TYPE" \
  --name "$SMOKE_PROVIDER_NAME" \
  --api-key "$SMOKE_API_KEY" \
  --base-url "$SMOKE_BASE_URL" \
  --model "$SMOKE_MODEL"

run_agentgate_cli provider-set-active "$SMOKE_PROVIDER_NAME"

echo "==> Starting AgentGate gateway"
AGENTGATE_DB_PATH="$db_dir" AGENTGATE_TOKEN="$TOKEN" \
  "$BIN" serve --host 127.0.0.1 --port "$gateway_port" \
  >"$tmp_dir/gateway.out" 2>"$tmp_dir/gateway.err" &
gateway_pid="$!"

for _ in $(seq 1 100); do
  if curl -fsS "http://127.0.0.1:${gateway_port}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
curl -fsS "http://127.0.0.1:${gateway_port}/health" >/dev/null

echo "==> Sending a Chat Completions request through AgentGate"
response_file="$tmp_dir/response.json"
status_file="$tmp_dir/response.status"
curl -sS -o "$response_file" -w "%{http_code}" \
    -X POST "http://127.0.0.1:${gateway_port}/v1/chat/completions" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    -H "User-Agent: codex-cli/quickstart-smoke" \
    -d '{
      "model": "agentgate",
      "messages": [
        { "role": "user", "content": "Say hello in one short sentence." }
      ],
      "stream": false
    }' >"$status_file"

if [[ "$(cat "$status_file")" != "200" ]]; then
  echo "Gateway request failed with HTTP $(cat "$status_file"):" >&2
  cat "$response_file" >&2
  echo >&2
  echo "Gateway stderr:" >&2
  tail -100 "$tmp_dir/gateway.err" >&2 || true
  echo "Mock upstream stderr:" >&2
  tail -100 "$tmp_dir/mock.err" >&2 || true
  exit 1
fi

node - "$tmp_dir/response.json" <<'NODE'
const fs = require("node:fs");
const response = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const content = response?.choices?.[0]?.message?.content;
const useReal = process.env.AGENTGATE_SMOKE_REAL === "1";
if (typeof content !== "string" || content.length === 0) {
  console.error("Unexpected gateway response:", JSON.stringify(response, null, 2));
  process.exit(1);
}
if (!useReal && content !== "AgentGate quickstart smoke passed.") {
  console.error("Unexpected mock gateway response:", JSON.stringify(response, null, 2));
  process.exit(1);
}
NODE

echo "==> Stopping gateway before reading logs"
# CLI `logs` opens the DB and re-runs init_database (write txn / migrations).
# Doing that while the live gateway still holds writers races under CI load.
if [[ -n "$gateway_pid" ]]; then
  kill "$gateway_pid" >/dev/null 2>&1 || true
  wait "$gateway_pid" >/dev/null 2>&1 || true
  gateway_pid=""
fi

echo "==> Verifying the request was logged"
logs=""
for attempt in $(seq 1 5); do
  stdout_file="$tmp_dir/logs.out"
  stderr_file="$tmp_dir/logs.err"
  : >"$stdout_file"
  : >"$stderr_file"
  if AGENTGATE_DB_PATH="$db_dir" AGENTGATE_TOKEN="$TOKEN" "$BIN" logs \
    --limit 5 \
    --provider "$SMOKE_PROVIDER_NAME" >"$stdout_file" 2>"$stderr_file"; then
    logs="$(cat "$stdout_file")"
    break
  fi
  if [[ "$attempt" -lt 5 ]]; then
    sleep 0.2
  else
    echo "Failed to read logs after ${attempt} attempts:" >&2
    cat "$stdout_file" >&2 || true
    cat "$stderr_file" >&2 || true
    exit 1
  fi
done
printf '%s\n' "$logs"

if grep -q "(no logs match)" <<<"$logs"; then
  echo "Expected request log for provider '$SMOKE_PROVIDER_NAME'." >&2
  exit 1
fi
if ! grep -q "200" <<<"$logs"; then
  echo "Expected request log to include HTTP 200." >&2
  exit 1
fi

echo "Quickstart core smoke passed."
