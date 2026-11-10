#!/usr/bin/env bash
# restrict_request_hosts, allowed_origins, allow_pushers, allow_openid_tokens
# and oauth off, against a throwaway server.
# Usage: run-browser-guard-e2e.sh <continuwuity-binary> <on|off>
set -euo pipefail

bin=$(realpath "$1")
mode=$2
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
port=$((22000 + RANDOM % 1000))
token=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')

cat > "$work/config.toml" <<EOF
[global]
server_name = "guard.test"
address = ["127.0.0.1"]
port = $port
database_path = "$work/db"
allow_registration = true
registration_token = "$token"
allow_federation = false
trusted_servers = []
allow_announcements_check = false
log = "warn"
EOF
if [[ $mode == on ]]; then
  cat >> "$work/config.toml" <<EOF
restrict_request_hosts = true
allowed_hosts = ["matrix.listed.example"]
allowed_origins = ["vector://vector", "https://client.listed.example"]
allow_pushers = false
allow_openid_tokens = false

[global.oauth]
compatibility_mode = "disabled"
EOF
fi

CONTINUWUITY_CONFIG="$work/config.toml" "$bin" > "$work/server.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$work"' EXIT
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$port/_matrix/client/versions" >/dev/null && break
  sleep 1
done
first=$(sed 's/\x1b\[[0-9;]*m//g' "$work/server.log" | grep -o 'using the registration token [^ ]*' | awk '{print $NF}' | head -1)

python3 "$here/browser_guard_e2e.py" "127.0.0.1:$port" "$token" "$first" "$mode" || {
  echo "--- server log ---"; tail -20 "$work/server.log"; exit 1;
}
