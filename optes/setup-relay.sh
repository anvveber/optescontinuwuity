#!/usr/bin/env bash
# setup-relay.sh — run optes-relay for your own user (Debian; Ubuntu untested).
# 2026-11-09: install dry-run on Debian 13 without a working Tor network.
# 2026-11-14: on Debian 13 in a VM over real Tor, straight to a core and through
# a front (HANDOFF.md §4e). 2026-11-15: Tor control port for the relay, tested
# with sleep and core restarts (HANDOFF.md §4i). Read before running.
#
# The relay gets Tor's control port (127.0.0.1:9051, with a random password
# that only this user can read) for one command: after two failed connects in
# a row it tells Tor to forget circuits and cached onion descriptors, so the
# next try does not wait for Tor's own retry (up to 15 minutes).
#
# Usage (as your normal user; uses sudo only to install and configure tor):
#   setup-relay.sh install --relay-bin <path/to/optes-relay> \
#       --front-list <frontlist.signed> --list-key <base64> [--level 1|2|3] [--i2p]
#   (--i2p also installs i2pd so the relay can use I2P fronts)
#   setup-relay.sh front-auth <front>.onion <52-char private key>   (tor fronts require it
#                                       unless they were installed with --open)
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

tor_auth_dir() { echo /var/lib/tor/onion_auth; }

tor_user() { echo debian-tor; }

CONTROL_BEGIN="# optes-relay: control port (begin)"
CONTROL_END="# optes-relay: control port (end)"

# Adds a control port with a password hash to /etc/tor/torrc (once) and keeps
# the password in $CONF_DIR/tor-control.pass. Prints the relay's arguments, or
# nothing if the torrc already has a control port of its own.
setup_control_port() {
  if sudo sed "/^$CONTROL_BEGIN\$/,/^$CONTROL_END\$/d" /etc/tor/torrc | grep -q '^ControlPort'; then
    echo "WARNING: /etc/tor/torrc has its own ControlPort; the relay will not use one" >&2
    return 0
  fi
  if [[ ! -s $CONF_DIR/tor-control.pass ]]; then
    head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$CONF_DIR/tor-control.pass"
    chmod 600 "$CONF_DIR/tor-control.pass"
  fi
  local hash
  hash=$(tor --hash-password "$(cat "$CONF_DIR/tor-control.pass")" | grep -E '^16:[0-9A-F]+$' | tail -n 1)
  [[ -n $hash ]] || die "tor --hash-password gave no hash"
  sudo sed -i "/^$CONTROL_BEGIN\$/,/^$CONTROL_END\$/d" /etc/tor/torrc
  printf '%s\nControlPort 127.0.0.1:9051\nHashedControlPassword %s\n%s\n' \
    "$CONTROL_BEGIN" "$hash" "$CONTROL_END" | sudo tee -a /etc/tor/torrc >/dev/null
  echo " --tor-control 127.0.0.1:9051 --tor-control-password-file $CONF_DIR/tor-control.pass"
}

cmd_install() {
  local bin="" list="" key="" level=2 i2p=no
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --relay-bin)  bin=${2:-}; shift 2 ;;
      --front-list) list=${2:-}; shift 2 ;;
      --list-key)   key=${2:-}; shift 2 ;;
      --level)      level=${2:-}; shift 2 ;;
      --i2p)        i2p=yes; shift ;;
      *) die "unknown option: $1" ;;
    esac
  done
  [[ $EUID -ne 0 ]] || die "run as your normal user, not root"
  [[ -x $bin ]] || die "--relay-bin: not an executable"
  [[ -f $list ]] || die "--front-list: file not found"
  [[ $key =~ ^[A-Za-z0-9+/]{43}=$ ]] || die "--list-key must be the base64 key setup-core.sh printed"
  [[ $level =~ ^[123]$ ]] || die "--level must be 1, 2 or 3"
  if [[ ! -f $CONF_DIR/device.key && ! -x $(dirname "$bin")/optes-keys ]]; then
    die "optes-keys must sit next to optes-relay to create the device key"
  fi

  info "Tor (system package; the relay uses its SOCKS and control ports)"
  command -v tor >/dev/null || sudo apt-get install -y tor
  if ! sudo grep -q '^ClientOnionAuthDir' /etc/tor/torrc; then
    echo "ClientOnionAuthDir $(tor_auth_dir)" | sudo tee -a /etc/tor/torrc >/dev/null
  fi
  sudo install -d -o "$(tor_user)" -g "$(tor_user)" -m 0700 "$(tor_auth_dir)"
  mkdir -p "$CONF_DIR"
  local control_arg
  control_arg=$(setup_control_port)
  sudo systemctl enable --now tor
  sudo systemctl restart tor

  local i2p_arg=""
  if [[ $i2p == yes ]]; then
    info "i2pd (its SOCKS proxy on 127.0.0.1:4447 reaches .i2p fronts)"
    command -v i2pd >/dev/null || sudo apt-get install -y i2pd
    sudo systemctl enable --now i2pd
    i2p_arg=" --socks-i2p 127.0.0.1:4447"
  fi

  mkdir -p "$CONF_DIR" "$STATE_DIR" "$BIN_DIR" "$UNIT_DIR"
  install -m 0755 "$bin" "$BIN_DIR/optes-relay"
  # The same file on a reinstall: nothing to copy.
  [[ $(realpath "$list") == "$(realpath -m "$CONF_DIR/frontlist.signed")" ]] || cp "$list" "$CONF_DIR/frontlist.signed"
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
# Warnings only: at "info" the relay logs every connect, a record of when
# this machine was online. For troubleshooting: RUST_LOG=info (debug also
# shows front addresses).
Environment=RUST_LOG=warn
ExecStart=$BIN_DIR/optes-relay --listen 127.0.0.1:8008 --socks 127.0.0.1:9050$i2p_arg$control_arg \\
  --front-list $CONF_DIR/frontlist.signed --list-key $CONF_DIR/list.pub \\
  --device-key $CONF_DIR/device.key --level $level --state-dir $STATE_DIR
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
EOF
  systemctl --user daemon-reload
  systemctl --user enable optes-relay.service
  # restart, not start: a second install (e.g. another --level) must replace
  # a relay that is already running
  systemctl --user restart optes-relay.service

  cat <<EOF

=====================================================================
Relay installed (level $level).

Device key — give this to the core admin:
  $(cat "$CONF_DIR/device.pub")
  (on the core: setup-core.sh add-device <name> <that key>)

Matrix client homeserver:  http://127.0.0.1:8008
Tor fronts need a key before the relay can reach them:
  $0 front-auth <front>.onion <key printed by setup-front.sh>
Logs (warnings only):  journalctl --user -u optes-relay -f

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
  # Restart, not reload. On Debian "reload tor" is passed on to tor@default,
  # and is lost if tor@default is still starting (for example right after
  # install). Tor then never reads the key and cannot decrypt the front's
  # descriptor. A restart always reads it.
  sudo systemctl restart tor
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
  *) sed -n '2,23p' "$0"; exit 1 ;;
esac
