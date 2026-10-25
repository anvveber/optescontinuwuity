#!/usr/bin/env bash
# require_encryption and hide_server_identity against a throwaway server.
# Usage: run-hardening-e2e.sh <continuwuity-binary> <on|off>
set -euo pipefail

bin=$(realpath "$1")
mode=$2
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
port=$((21000 + RANDOM % 1000))
token=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')
if [[ $mode == on ]]; then flag=true; else flag=false; fi

cat > "$work/config.toml" <<EOF
[global]
server_name = "hardening.test"
address = ["127.0.0.1"]
port = $port
database_path = "$work/db"
allow_registration = true
registration_token = "$token"
allow_federation = false
trusted_servers = []
allow_announcements_check = false
log = "warn"
require_encryption = $flag
hide_server_identity = $flag
EOF

CONTINUWUITY_CONFIG="$work/config.toml" "$bin" > "$work/server.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$work"' EXIT
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$port/_matrix/client/versions" >/dev/null && break
  sleep 1
done
first=$(sed 's/\x1b\[[0-9;]*m//g' "$work/server.log" | grep -o 'using the registration token [^ ]*' | awk '{print $NF}' | head -1)

python3 "$here/hardening_e2e.py" "http://127.0.0.1:$port" "$token" "$first" "$mode" || {
  echo "--- server log ---"; tail -20 "$work/server.log"; exit 1;
}
