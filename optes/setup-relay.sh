#!/usr/bin/env bash
# setup-relay.sh — run optes-relay for your own user (Fedora or Debian).
# UNTESTED end to end (no Tor in the build environment). Read before running.
#
# Usage (as your normal user; uses sudo only to install tor):
#   setup-relay.sh install --relay-bin <path/to/optes-relay> \
#       --front-list <frontlist.signed> --list-key <base64> [--level 1|2|3]
#   setup-relay.sh front-auth <front>.onion <52-char private key>   (if the front requires it)
#   setup-relay.sh status
#
# After install, give the printed device key to the core admin:
#   setup-core.sh add-device <name> <device key>
# Then point your Matrix client at http://127.0.0.1:8008.

set -euo pipefail
umask 077

CONF_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/optes-relay
STATE_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/optes-relay
BIN_DIR=$HOME/.local/bin
UNIT_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user

die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "==> $*"; }

tor_auth_dir() {
  # Fedora and Debian both read /etc/tor/torrc; Debian runs tor as debian-tor,
  # Fedora as toranon.
  echo /var/lib/tor/onion_auth
}

tor_user() {
  if id debian-tor &>/dev/null; then echo debian-tor; else echo toranon; fi
}

cmd_install() {
  local bin="" list="" key="" level=2
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --relay-bin)  bin=${2:-}; shift 2 ;;
      --front-list) list=${2:-}; shift 2 ;;
      --list-key)   key=${2:-}; shift 2 ;;
      --level)      level=${2:-}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  [[ $EUID -ne 0 ]] || die "run as your normal user, not root"
  [[ -x $bin ]] || die "--relay-bin: not an executable"
  [[ -f $list ]] || die "--front-list: file not found"
  [[ $key =~ ^[A-Za-z0-9+/]{43}=$ ]] || die "--list-key must be the base64 key setup-core.sh printed"
  [[ $level =~ ^[123]$ ]] || die "--level must be 1, 2 or 3"

  info "Tor (system package; the relay uses its SOCKS port)"
  if ! command -v tor >/dev/null; then
    if command -v dnf >/dev/null; then sudo dnf install -y tor; else sudo apt-get install -y tor; fi
  fi
  if ! sudo grep -q '^ClientOnionAuthDir' /etc/tor/torrc; then
    echo "ClientOnionAuthDir $(tor_auth_dir)" | sudo tee -a /etc/tor/torrc >/dev/null
  fi
  sudo install -d -o "$(tor_user)" -g "$(tor_user)" -m 0700 "$(tor_auth_dir)"
  sudo systemctl enable --now tor
  sudo systemctl restart tor

  mkdir -p "$CONF_DIR" "$STATE_DIR" "$BIN_DIR" "$UNIT_DIR"
  install -m 0755 "$bin" "$BIN_DIR/optes-relay"
  cp "$list" "$CONF_DIR/frontlist.signed"
  echo "$key" > "$CONF_DIR/list.pub"

  if [[ ! -f $CONF_DIR/device.key ]]; then
    local keys_bin
    keys_bin="$(dirname "$bin")/optes-keys"
    [[ -x $keys_bin ]] || die "optes-keys must sit next to optes-relay to create the device key"
    "$keys_bin" gen-x25519 "$CONF_DIR/device" >/dev/null
  fi

  cat > "$UNIT_DIR/optes-relay.service" <<EOF
[Unit]
Description=optes relay (Matrix client endpoint on 127.0.0.1:8008)
After=network-online.target

[Service]
ExecStart=$BIN_DIR/optes-relay --listen 127.0.0.1:8008 --socks 127.0.0.1:9050 \\
  --front-list $CONF_DIR/frontlist.signed --list-key $CONF_DIR/list.pub \\
  --device-key $CONF_DIR/device.key --level $level --state-dir $STATE_DIR
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
EOF
  systemctl --user daemon-reload
  systemctl --user enable --now optes-relay.service

  cat <<EOF

=====================================================================
Relay installed (level $level).

Device key — give this to the core admin:
  $(cat "$CONF_DIR/device.pub")
  (on the core: setup-core.sh add-device <name> <that key>)

Matrix client homeserver:  http://127.0.0.1:8008
Logs:  journalctl --user -u optes-relay -f

For level 4 (always on), install this on a machine that stays on and
reach it from your phone over WireGuard; then use --listen <wg-ip>:8008.
=====================================================================
EOF
}

cmd_front_auth() {
  local onion=${1:-} priv=${2:-}
  [[ $onion =~ ^[a-z2-7]{56}\.onion$ ]] || die "first argument: the front's .onion address"
  [[ $priv =~ ^[A-Z2-7]{52}$ ]] || die "second argument: the 52-char key from setup-front.sh add-client"
  local file
  file="$(tor_auth_dir)/${onion%%.*}.auth_private"
  echo "${onion%.onion}:descriptor:x25519:$priv" | sudo tee "$file" >/dev/null
  sudo chown "$(tor_user):$(tor_user)" "$file"
  sudo chmod 600 "$file"
  sudo systemctl reload tor
  info "front auth stored"
}

cmd_status() {
  systemctl --user --no-pager status optes-relay.service | head -5 || true
  echo "device key: $(cat "$CONF_DIR/device.pub" 2>/dev/null || echo -)"
  echo "front list version accepted: $(cat "$STATE_DIR/frontlist.version" 2>/dev/null || echo initial)"
}

case "${1:-}" in
  install)    shift; cmd_install "$@" ;;
  front-auth) shift; cmd_front_auth "$@" ;;
  status)     cmd_status ;;
  *) sed -n '2,15p' "$0"; exit 1 ;;
esac
