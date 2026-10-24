#!/usr/bin/env bash
# Runs the delayed-release end-to-end test against a throwaway local server.
# Usage: run-e2e.sh <path-to-continuwuity-binary> <interval_secs>
set -euo pipefail

bin=$(realpath "$1")
interval=$2
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
port=$((18000 + RANDOM % 1000))
token=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')

cat > "$work/config.toml" <<EOF
[global]
server_name = "e2e.test"
address = ["127.0.0.1"]
port = $port
database_path = "$work/db"
allow_registration = true
registration_token = "$token"
allow_federation = false
trusted_servers = []
allow_announcements_check = false
log = "warn"
release_interval_secs = $interval
EOF

CONTINUWUITY_CONFIG="$work/config.toml" "$bin" > "$work/server.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$work"' EXIT

for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$port/_matrix/client/versions" >/dev/null && break
  sleep 1
done

python3 "$here/delayed_release_e2e.py" "http://127.0.0.1:$port" "$token" "$interval" || {
  echo "--- server log ---"; tail -30 "$work/server.log"; exit 1;
}
