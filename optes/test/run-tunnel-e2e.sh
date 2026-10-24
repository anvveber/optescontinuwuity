#!/usr/bin/env bash
# End-to-end test of relay -> front -> gateway -> continuwuity, no Tor.
# Usage: run-tunnel-e2e.sh <continuwuity-binary> <tunnel-target-dir> <level>
set -euo pipefail

server_bin=$(realpath "$1")
tdir=$(realpath "$2")
level=$3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
base=$((20000 + RANDOM % 5000))
mport=$base; gport=$((base + 1)); fport=$((base + 2)); rport=$((base + 3))
window=5; tick=200
token=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null; rm -rf "$work"; }
trap cleanup EXIT

keys="$tdir/optes-keys"
mkdir -p "$work/devices" "$work/relay-state"
"$keys" gen-x25519 "$work/gateway" >/dev/null
"$keys" gen-x25519 "$work/device" >/dev/null
"$keys" gen-ed25519 "$work/list" >/dev/null
cp "$work/device.pub" "$work/devices/test.pub"
# Relay starts with v1; the gateway serves v2, which the relay must fetch.
"$keys" sign-list --key "$work/list.key" --version 1 --gateway-key "$work/gateway.pub" \
  --front "tcp:127.0.0.1:$fport" --out "$work/list-v1.signed" >/dev/null
"$keys" sign-list --key "$work/list.key" --version 2 --gateway-key "$work/gateway.pub" \
  --front "tcp:127.0.0.1:$fport" --out "$work/list-v2.signed" >/dev/null

cat > "$work/config.toml" <<EOF
[global]
server_name = "tunnel.test"
address = ["127.0.0.1"]
port = $mport
database_path = "$work/db"
allow_registration = true
registration_token = "$token"
allow_federation = false
trusted_servers = []
allow_announcements_check = false
log = "warn"
EOF

CONTINUWUITY_CONFIG="$work/config.toml" "$server_bin" > "$work/server.log" 2>&1 & pids+=($!)
for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$mport/_matrix/client/versions" >/dev/null && break; sleep 1; done
first=$(sed 's/\x1b\[[0-9;]*m//g' "$work/server.log" | grep -o 'using the registration token [^ ]*' | awk '{print $NF}' | head -1)

"$tdir/optes-gateway" --listen "tcp:127.0.0.1:$gport" --upstream "tcp:127.0.0.1:$mport" \
  --key "$work/gateway.key" --devices "$work/devices" --front-list "$work/list-v2.signed" \
  --tick-ms $tick --window-secs $window > "$work/gateway.log" 2>&1 & pids+=($!)
python3 "$here/front_recorder.py" "$fport" "127.0.0.1:$gport" "$work/wire.log" & pids+=($!)
sleep 1
"$tdir/optes-relay" --listen "127.0.0.1:$rport" --front-list "$work/list-v1.signed" \
  --list-key "$work/list.pub" --device-key "$work/device.key" --level "$level" \
  --state-dir "$work/relay-state" > "$work/relay.log" 2>&1 & pids+=($!)

status=0
python3 "$here/tunnel_e2e.py" "http://127.0.0.1:$rport" "$token" "$first" "$level" \
  "$work/wire.log" "$window" "$tick" || status=1

stored=$(cat "$work/relay-state/frontlist.version" 2>/dev/null || echo none)
if [[ $stored == 2 ]]; then echo "PASS relay fetched newer front list (v2)"; else echo "FAIL relay front list version: $stored"; status=1; fi

# A device the gateway doesn't know must not get a tunnel.
"$keys" gen-x25519 "$work/stranger" >/dev/null
"$tdir/optes-relay" --listen "127.0.0.1:$((base + 4))" --front-list "$work/list-v1.signed" \
  --list-key "$work/list.pub" --device-key "$work/stranger.key" --level 1 \
  --state-dir "$work/stranger-state" > "$work/stranger.log" 2>&1 & pids+=($!)
sleep 8
if grep -q "tunnel up" "$work/stranger.log"; then echo "FAIL unknown device got a tunnel"; status=1
else echo "PASS unknown device gets no tunnel"; fi

if [[ $status != 0 ]]; then
  echo "--- relay log ---"; tail -15 "$work/relay.log"
  echo "--- gateway log ---"; tail -15 "$work/gateway.log"
fi
exit $status
