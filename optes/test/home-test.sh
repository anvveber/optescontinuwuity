#!/usr/bin/env bash
# home-test.sh — try the whole system over real Tor on a machine you already
# use, without changing anything system-wide.
#
# Everything runs as your normal user inside one directory: continuwuity,
# optes-gateway and a private tor instance (own data dir, no SOCKS port, does
# not touch the system tor). No root, no firewall, no sysctl, no apt changes.
# Stop it and delete the directory and nothing is left behind.
#
# What it does NOT test: setup-core.sh itself (sandboxing, nftables, AppArmor).
# That needs a dedicated machine or VM.
#
# Server side (e.g. tvhub):
#   home-test.sh start --bin-dir <dir with continuwuity, optes-gateway, optes-keys>
#   home-test.sh status | stop | reset
#   home-test.sh renew          (re-sign the front list; runs daily by itself)
#
# Then copy the printed bundle to the client machine (e.g. your Fedora laptop)
# and run, from inside the unpacked bundle:
#   ./relay.sh
# and point a Matrix client at http://127.0.0.1:8008.

set -euo pipefail
umask 077

DIR=${OPTES_TEST_DIR:-$HOME/optes-test}
RUN=$DIR/run
RELEASE_SECS=15

die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "==> $*"; }

wait_for() {
  local f=$1 tries=${2:-90}
  for ((i = 0; i < tries; i++)); do [[ -e $f ]] && return 0; sleep 1; done
  return 1
}

running() { [[ -f $RUN/$1.pid ]] && kill -0 "$(cat "$RUN/$1.pid")" 2>/dev/null; }

launch() { # name, command...
  local name=$1; shift
  if running "$name"; then return 0; fi
  nohup "$@" >> "$DIR/logs/$name.log" 2>&1 &
  echo $! > "$RUN/$name.pid"
}

# x25519 keypair for onion client auth (base32, as Tor wants it).
gen_onion_auth() {
  local tmp
  tmp=$(mktemp -d)
  openssl genpkey -algorithm x25519 -out "$tmp/k.pem" 2>/dev/null
  AUTH_PRIV=$(openssl pkey -in "$tmp/k.pem" -outform DER | tail -c 32 | base32 | tr -d '=\n')
  AUTH_PUB=$(openssl pkey -in "$tmp/k.pem" -pubout -outform DER | tail -c 32 | base32 | tr -d '=\n')
  rm -rf "$tmp"
}

cmd_start() {
  local bin_dir=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --bin-dir) bin_dir=$(realpath "${2:-}"); shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  [[ $EUID -ne 0 ]] || die "run as your normal user"
  for b in continuwuity optes-gateway optes-keys; do
    [[ -x $bin_dir/$b ]] || die "--bin-dir must contain $b"
  done
  command -v tor >/dev/null || die "tor not installed (sudo apt install tor — the system service is not used)"
  command -v openssl >/dev/null || die "openssl not installed"

  mkdir -p "$DIR"/{run,logs,db,devices,tor/ingress/authorized_clients,bundle/auth}
  chmod 700 "$DIR/tor" "$DIR/tor/ingress"
  local keys=$bin_dir/optes-keys

  if [[ ! -f $DIR/config.toml ]]; then
    info "First start: keys and config"
    "$keys" gen-x25519 "$DIR/gateway" > /dev/null
    "$keys" gen-ed25519 "$DIR/list" > /dev/null
    "$keys" gen-x25519 "$DIR/bundle/device" > /dev/null
    cp "$DIR/bundle/device.pub" "$DIR/devices/laptop.pub"
    cp "$DIR/list.pub" "$DIR/bundle/list.pub"
    gen_onion_auth
    echo "descriptor:x25519:$AUTH_PUB" > "$DIR/tor/ingress/authorized_clients/laptop.auth"
    echo "$AUTH_PRIV" > "$DIR/bundle/onion_auth.key"
    head -c 24 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n' > "$DIR/registration_token"

    cat > "$DIR/config.toml" <<EOF
[global]
server_name = "optes-test"
unix_socket_path = "$RUN/continuwuity.sock"
unix_socket_perms = 660
database_path = "$DIR/db"
allow_announcements_check = false
allow_federation = false
trusted_servers = []
forbidden_remote_server_names = [".*"]
allow_registration = true
registration_token_file = "$DIR/registration_token"
new_user_displayname_suffix = ""
allow_local_presence = false
allow_incoming_presence = false
allow_outgoing_presence = false
allow_local_read_receipts = false
allow_local_typing = false
require_auth_for_profile_requests = true
lockdown_public_room_directory = true
log = "warn"
log_colors = false
release_interval_secs = $RELEASE_SECS
record_device_activity = false
auto_join_space_children = true
allow_space_creation = false
restrict_room_creation_to_spaces = true
require_encryption = true
hide_server_identity = true
allow_room_retention = true
EOF

    cat > "$DIR/torrc" <<EOF
DataDirectory $DIR/tor
SocksPort 0
Log notice file $DIR/logs/tor-notice.log
HiddenServiceDir $DIR/tor/ingress
HiddenServicePort 443 unix:$RUN/gateway.sock
HiddenServicePort 80 unix:$RUN/continuwuity.sock
EOF
  fi

  info "continuwuity"
  CONTINUWUITY_CONFIG=$DIR/config.toml launch continuwuity "$bin_dir/continuwuity"
  wait_for "$RUN/continuwuity.sock" 60 || die "continuwuity did not start; see $DIR/logs/continuwuity.log"

  info "optes-gateway"
  launch gateway "$bin_dir/optes-gateway" --listen "unix:$RUN/gateway.sock" \
    --upstream "unix:$RUN/continuwuity.sock" --key "$DIR/gateway.key" \
    --devices "$DIR/devices" --front-list "$DIR/bundle/frontlist.signed" \
    --tick-ms 200 --window-secs "$RELEASE_SECS"
  wait_for "$RUN/gateway.sock" 30 || die "gateway did not start; see $DIR/logs/gateway.log"

  info "tor (private instance; publishing the onion can take a minute or two)"
  launch tor tor -f "$DIR/torrc"
  wait_for "$DIR/tor/ingress/hostname" 120 || die "tor did not create the onion; see $DIR/logs/tor-notice.log"
  local onion
  onion=$(cat "$DIR/tor/ingress/hostname")

  if [[ ! -f $DIR/bundle/frontlist.signed ]]; then
    # No front for this test: relays connect straight to the core's tunnel port.
    echo "$bin_dir" > "$DIR/bin_dir"
    echo "tor:$onion:443" > "$DIR/front"
    sign_list
    echo "${onion%.onion}:descriptor:x25519:$(cat "$DIR/bundle/onion_auth.key")" \
      > "$DIR/bundle/auth/core.auth_private"
    rm -f "$DIR/bundle/onion_auth.key"
    write_relay_script
  fi

  # Lists expire after 30 days: re-sign daily while the test runs. The
  # gateway serves the new list; relays pick it up on their own.
  launch renewer bash -c "while sleep 86400; do '$(realpath "$0")' renew; done"

  tar -C "$DIR" -czf "$DIR/optes-test-bundle.tar.gz" bundle
  chmod 600 "$DIR/optes-test-bundle.tar.gz"

  local first
  first=$(sed 's/\x1b\[[0-9;]*m//g' "$DIR/logs/continuwuity.log" | grep -o 'using the registration token [^ ]*' | awk '{print $NF}' | tail -1)

  cat <<EOF

=====================================================================
Server running (all inside $DIR).

Onion: $onion  (client auth required)
  port 443 = tunnel (relay)    port 80 = plain HTTP (Tor Browser, testing)

Client bundle: $DIR/optes-test-bundle.tar.gz
  It contains this test's device key and onion auth key: copy it over the
  LAN only, e.g.  scp tvhub:$DIR/optes-test-bundle.tar.gz .
  On the client:  tar xzf optes-test-bundle.tar.gz && cd bundle && ./relay.sh
  Needs on the client: tor installed, and optes-relay next to relay.sh
  (or pass its path as the first argument).

Matrix client homeserver:  http://127.0.0.1:8008
First account token (one time):  ${first:-see $DIR/logs/continuwuity.log}
Later accounts:                   $(cat "$DIR/registration_token")

Messages appear on 15-second ticks — that's the delayed release working.
Stop:  $0 stop      Wipe everything:  $0 reset
=====================================================================
EOF
}

write_relay_script() {
  cat > "$DIR/bundle/relay.sh" <<'EOF'
#!/usr/bin/env bash
# Runs a private tor instance and optes-relay from this folder, as your user.
# Usage: ./relay.sh [path/to/optes-relay] [level 1|2|3]
set -euo pipefail
umask 077
here=$(cd "$(dirname "$0")" && pwd)
relay=${1:-$here/optes-relay}
level=${2:-2}
[[ -x $relay ]] || { echo "optes-relay not found (give its path as first argument)" >&2; exit 1; }
command -v tor >/dev/null || { echo "install tor first (dnf install tor / apt install tor)" >&2; exit 1; }
mkdir -p "$here/tor" "$here/state"
chmod 700 "$here/tor" "$here/auth"
cat > "$here/torrc" <<T
DataDirectory $here/tor
SocksPort 127.0.0.1:19050
ClientOnionAuthDir $here/auth
Log notice file $here/tor.log
T
tor -f "$here/torrc" &
tor_pid=$!
trap 'kill $tor_pid 2>/dev/null' EXIT
echo "tor starting (log: $here/tor.log); relay will retry until Tor is ready"
"$relay" --listen 127.0.0.1:8008 --socks 127.0.0.1:19050 \
  --front-list "$here/frontlist.signed" --list-key "$here/list.pub" \
  --device-key "$here/device.key" --level "$level" --state-dir "$here/state"
EOF
  chmod 700 "$DIR/bundle/relay.sh"
}

sign_list() {
  local version
  version=$(( $(cat "$DIR/list.version" 2>/dev/null || echo 0) + 1 ))
  "$(cat "$DIR/bin_dir")/optes-keys" sign-list --key "$DIR/list.key" --version "$version" \
    --gateway-key "$DIR/gateway.pub" --front "$(cat "$DIR/front")" \
    --out "$DIR/bundle/frontlist.signed.tmp" > /dev/null
  mv "$DIR/bundle/frontlist.signed.tmp" "$DIR/bundle/frontlist.signed"
  echo "$version" > "$DIR/list.version"
}

cmd_renew() {
  [[ -f $DIR/front ]] || die "not set up yet; run start first"
  sign_list
  info "front list v$(cat "$DIR/list.version") signed"
}

cmd_stop() {
  local name
  for name in renewer tor gateway continuwuity; do
    if running "$name"; then
      kill "$(cat "$RUN/$name.pid")" && info "stopped $name"
    fi
    rm -f "$RUN/$name.pid"
  done
}

cmd_status() {
  local name
  for name in continuwuity gateway tor renewer; do
    printf '%-14s %s\n' "$name" "$(running "$name" && echo running || echo stopped)"
  done
  printf '%-14s %s\n' "onion" "$(cat "$DIR/tor/ingress/hostname" 2>/dev/null || echo -)"
  printf '%-14s %s\n' "logs" "$DIR/logs/"
}

cmd_reset() {
  cmd_stop
  [[ -d $DIR ]] || exit 0
  read -r -p "Delete $DIR (database, keys, everything)? [y/N] " answer
  [[ $answer == y ]] && rm -rf "$DIR" && info "deleted $DIR"
}

case "${1:-}" in
  start)  shift; cmd_start "$@" ;;
  stop)   cmd_stop ;;
  status) cmd_status ;;
  renew)  cmd_renew ;;
  reset)  cmd_reset ;;
  *) sed -n '2,23p' "$0"; exit 1 ;;
esac
