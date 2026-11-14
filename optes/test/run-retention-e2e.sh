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

RELEASE=${RELEASE:-0} python3 "$here/retention_e2e.py" "http://127.0.0.1:$port" "$token" "$first" "$work/server.log" || {
  echo "--- server log ---"; grep -iE "warn|error|retention" "$work/server.log" | tail -20; exit 1;
}

# Restart on the purged database: startup checks must not trip over it.
kill "$pid"; wait "$pid" 2>/dev/null || true
CONTINUWUITY_CONFIG="$work/config.toml" "$bin" > "$work/server2.log" 2>&1 &
pid=$!
ok=no
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$port/_matrix/client/versions" >/dev/null && { ok=yes; break; }
  sleep 1
done
sleep 2
# Warnings and errors at startup, minus two that come from the machine, not
# from the database (both upstream code):
#  - "DEVNAME not found": the database is on a filesystem with no block
#    device (/tmp on tmpfs); the storage probe gives up.
#  - "mallctl: ...": jemalloc tuning of worker threads (4+ cores) failed;
#    logged at ERROR in debug builds only.
startup_problems() {
  sed 's/\x1b\[[0-9;]*m//g' "$work/server2.log" | grep -v "config::check" \
    | grep -vE "DEVNAME not found|ERROR conduwuit_core::error::log: mallctl: " \
    | grep -E " (ERROR|WARN) |panicked"
}
if [[ $ok == yes ]] && ! startup_problems >/dev/null; then
  echo "PASS server restarts cleanly on the purged database"
else
  echo "FAIL restart after purge (ok=$ok)"; startup_problems | head -10; exit 1
fi
