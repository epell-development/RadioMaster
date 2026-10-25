#!/usr/bin/env bash
# Starts a built RadioMaster Server with a throwaway data folder, checks it answers over HTTP
# (and the web portal, and signing in), then stops it. Usage: tools/smoke_server.sh path/to/RadioMaster-Server
set -euo pipefail
bin="$1"
data="$(mktemp -d)"
port=19259
portal=19260
log="$data/server.log"
export RADIOMASTER_PASSWORD="ci-smoke-test-password"

"$bin" --version
"$bin" --setup --user ci --name "CI smoke test" --data "$data" --port "$port" --no-https --portal --portal-port "$portal"
"$bin" --data "$data" --no-audio >"$log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  curl -fsS "http://127.0.0.1:$port/api/v1/ping" >/dev/null 2>&1 && break
  kill -0 "$pid" 2>/dev/null || { echo "the server exited:"; cat "$log"; exit 1; }
  sleep 0.5
done
echo "ping:   $(curl -fsS "http://127.0.0.1:$port/api/v1/ping")"
login=$(curl -fsS -X POST -H 'Content-Type: application/json' \
  -d "{\"user\":\"ci\",\"password\":\"$RADIOMASTER_PASSWORD\"}" "http://127.0.0.1:$port/api/v1/login")
grep -q '"token"' <<<"$login" || { echo "sign-in failed: $login"; cat "$log"; exit 1; }
echo "login:  ok"
curl -fsS "http://127.0.0.1:$portal/" | grep -qi "<html" || { echo "the web portal didn't answer"; cat "$log"; exit 1; }
echo "portal: ok"
kill "$pid"
wait "$pid" 2>/dev/null || true
trap - EXIT
echo "RadioMaster Server smoke test passed"
