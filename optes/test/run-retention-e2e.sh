#!/usr/bin/env bash
# Per-room retention against a throwaway server.
# Usage: run-retention-e2e.sh <continuwuity-binary>
set -euo pipefail

bin=$(realpath "$1")
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
port=$((22000 + RANDOM % 1000))
token=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')

cat > "$work/config.toml" <<EOF
[global]
server_name = "retention.test"
address = ["127.0.0.1"]
port = $port
database_path = "$work/db"
allow_registration = true
registration_token = "$token"
allow_federation = false
trusted_servers = []
allow_announcements_check = false
log = "info"
allow_room_retention = true
retention_purge_interval_secs = 3
release_interval_secs = ${RELEASE:-0}
EOF

CONTINUWUITY_CONFIG="$work/config.toml" "$bin" > "$work/server.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$work"' EXIT
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$port/_matrix/client/versions" >/dev/null && break
  sleep 1
done
first=$(sed 's/\x1b\[[0-9;]*m//g' "$work/server.log" | grep -o 'using the registration token [^ ]*' | awk '{print $NF}' | head -1)

python3 "$here/retention_e2e.py" "http://127.0.0.1:$port" "$token" "$first" "$work/server.log" || {
  echo "--- server log ---"; grep -iE "warn|error|retention" "$work/server.log" | tail -20; exit 1;
}
