#!/usr/bin/env bash
# The deployed connection path, with a SOCKS stand-in instead of Tor:
#
#   relay --SOCKS5--> proxy --> [shim: Tor delivering to the front's socket]
#     --> front socat (exact setup-front.sh command) --SOCKS4a--> proxy
#     --> gateway --> continuwuity
#
# Usage: run-socks-path-e2e.sh <continuwuity-binary> <tunnel-target-dir> [tor|i2p]
# (i2p: the relay reaches the front through --socks-i2p, as with i2pd)
set -euo pipefail

server_bin=$(realpath "$1")
tdir=$(realpath "$2")
transport=${3:-tor}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
base=$((26000 + RANDOM % 5000))
mport=$base; gport=$((base + 1)); fport=$((base + 2)); rport=$((base + 3)); pport=$((base + 4))
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
# "localhost" goes to the proxy as a hostname, like a .onion would.
"$keys" sign-list --key "$work/list.key" --version 1 --gateway-key "$work/gateway.pub" \
  --front "$transport:localhost:$fport" --out "$work/list.signed" >/dev/null

cat > "$work/config.toml" <<EOF
[global]
server_name = "socks.test"
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

python3 "$here/socks_proxy.py" "$pport" "$work/socks.log" & pids+=($!)
"$tdir/optes-gateway" --listen "tcp:127.0.0.1:$gport" --upstream "tcp:127.0.0.1:$mport" \
  --key "$work/gateway.key" --devices "$work/devices" --window-secs 5 > "$work/gateway.log" 2>&1 & pids+=($!)
# Front: the same socat line setup-front.sh installs (core = localhost:$gport).
socat "UNIX-LISTEN:$work/front.sock,fork,unlink-early" \
  "SOCKS4A:127.0.0.1:localhost:$gport,socksport=$pport" & pids+=($!)
# Shim: Tor delivering onion connections to the front's unix socket.
socat "TCP-LISTEN:$fport,bind=127.0.0.1,fork,reuseaddr" "UNIX-CONNECT:$work/front.sock" & pids+=($!)
sleep 1
if [[ $transport == i2p ]]; then proxy_flag=--socks-i2p; else proxy_flag=--socks; fi
"$tdir/optes-relay" --listen "127.0.0.1:$rport" "$proxy_flag" "127.0.0.1:$pport" \
  --front-list "$work/list.signed" --list-key "$work/list.pub" --device-key "$work/device.key" \
  --level 1 --state-dir "$work/relay-state" > "$work/relay.log" 2>&1 & pids+=($!)

status=0
: > "$work/wire.log"
python3 "$here/tunnel_e2e.py" "http://127.0.0.1:$rport" "$token" "$first" 1 "$work/wire.log" 5 200 || status=1

grep -q "socks5 localhost:$fport" "$work/socks.log" && echo "PASS relay used SOCKS5 with a hostname" || { echo "FAIL relay SOCKS5"; status=1; }
grep -q "socks4 localhost:$gport" "$work/socks.log" && echo "PASS front used SOCKS4a with a hostname" || { echo "FAIL front SOCKS4a"; status=1; }

if [[ $status != 0 ]]; then
  echo "--- socks log ---"; cat "$work/socks.log"
  echo "--- relay log ---"; tail -10 "$work/relay.log"
  echo "--- gateway log ---"; tail -10 "$work/gateway.log"
fi
exit $status
