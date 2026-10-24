#!/usr/bin/env bash
# setup-front.sh — a disposable Tor or I2P front for the private Matrix core.
# Debian 13. UNTESTED (no Tor in the build environment). Read before running.
#
# A front is a blind byte pipe: relays connect to the front's onion service;
# the front forwards the raw bytes over Tor to the core's ingress onion
# (port 443, the tunnel). It holds no user data and can't read the traffic:
# everything is encrypted relay-to-core. Burn and replace freely.
#
# Usage:
#   setup-front.sh install --core <ingress>.onion --core-auth <private key> [--type tor|i2p]
#   setup-front.sh add-client <name>        (tor fronts: onion client auth for a relay; optional)
#   setup-front.sh remove-client <name>
#   setup-front.sh lockdown --i-tested-onion-ssh
#   setup-front.sh status
#
# --core-auth: on the core run `setup-core.sh add-client <frontname>`; pass the
# 52-character private key it prints.
#
# Tor front: without any add-client, the front's onion is open to anyone who
# knows the address (they still can't get past the core's gateway without a
# device key). After the first add-client, only listed relays can reach it.
#
# I2P front: i2pd in hidden mode (no published router address, no transit
# traffic) hosts a server tunnel; the front still reaches the core over Tor.
# Note: an I2P router talks to other I2P routers directly, so they see this
# VPS's IP. That's inherent to I2P and why fronts are disposable.

set -euo pipefail
umask 077

TOR_USER=debian-tor
FWD_USER=optes-front
STATE_DIR=/etc/optes-front
HS_FRONT=/var/lib/tor/optes_front
HS_ADMIN=/var/lib/tor/admin_ssh
AUTH_DIR=/var/lib/tor/onion_auth
TORRC=/etc/tor/torrc
NFT_CONF=/etc/nftables.conf
SOCK_DIR=/run/optes-front
SOCK=$SOCK_DIR/front.sock
UNIT=/etc/systemd/system/optes-front.service
I2P_USER=i2pd
I2P_PORT=7443
I2P_KEYS=/var/lib/i2pd/optes-front.dat

die()  { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }
info() { echo "==> $*"; }
need_root() { [[ $EUID -eq 0 ]] || die "run as root"; }
valid_name() { [[ $1 =~ ^[A-Za-z0-9_-]{1,32}$ ]] || die "name must be 1-32 chars of A-Z a-z 0-9 _ -"; }

wait_for_file() {
  local f=$1 tries=${2:-60}
  for ((i = 0; i < tries; i++)); do
    [[ -s $f ]] && return 0
    sleep 1
  done
  return 1
}

gen_x25519() {
  local tmp
  tmp=$(mktemp -d)
  openssl genpkey -algorithm x25519 -out "$tmp/k.pem" 2>/dev/null
  CLIENT_PRIV=$(openssl pkey -in "$tmp/k.pem" -outform DER | tail -c 32 | base32 | tr -d '=\n')
  CLIENT_PUB=$(openssl pkey -in "$tmp/k.pem" -pubout -outform DER | tail -c 32 | base32 | tr -d '=\n')
  shred -u "$tmp/k.pem" 2>/dev/null || rm -f "$tmp/k.pem"
  rmdir "$tmp"
  [[ ${#CLIENT_PRIV} -eq 52 && ${#CLIENT_PUB} -eq 52 ]] || die "key generation failed"
}

write_nft() {
  local keep_ssh=$1 ssh_rule="" i2p_rule=""
  [[ $keep_ssh == yes ]] && ssh_rule="    tcp dport 22 accept"
  [[ $(cat "$STATE_DIR/type" 2>/dev/null) == i2p ]] && i2p_rule="    meta skuid \"$I2P_USER\" accept"
  cat > "$NFT_CONF" <<EOF
#!/usr/sbin/nft -f
# Written by setup-front.sh. Only Tor talks to the internet.
flush ruleset

table inet optesfront {
  chain input {
    type filter hook input priority 0; policy drop;
    iif lo accept
    ct state invalid drop
    ct state established,related accept
    icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert, nd-router-advert } accept
    udp sport 67 udp dport 68 accept
    udp sport 547 udp dport 546 accept
$ssh_rule
  }
  chain forward { type filter hook forward priority 0; policy drop; }
  chain output {
    type filter hook output priority 0; policy drop;
    oif lo accept
    ct state invalid drop
    ct state established,related accept
    meta skuid "$TOR_USER" accept
$i2p_rule
    icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert, nd-router-solicit } accept
    udp sport 68 udp dport 67 accept
    udp sport 546 udp dport 547 accept
  }
}
EOF
  chmod 600 "$NFT_CONF"
  nft -c -f "$NFT_CONF" || die "nftables config failed validation"
  echo "$keep_ssh" > "$STATE_DIR/keep_clearnet_ssh"
}

cmd_install() {
  local core="" core_auth="" type=tor
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --type)      type=${2:-}; shift 2 ;;
      --core)      core=${2:-}; shift 2 ;;
      --core-auth) core_auth=${2:-}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  [[ $core =~ ^[a-z2-7]{56}\.onion$ ]] || die "--core must be the core's ingress onion address"
  [[ $core_auth =~ ^[A-Z2-7]{52}$ ]] || die "--core-auth must be the 52-char base32 private key from setup-core.sh add-client"
  [[ $type == tor || $type == i2p ]] || die "--type must be tor or i2p"

  mkdir -p "$STATE_DIR"
  chmod 700 "$STATE_DIR"
  echo "$type" > "$STATE_DIR/type"

  info "Packages"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends tor apt-transport-tor socat nftables openssl coreutils apparmor-utils python3
  [[ $type == i2p ]] && apt-get install -y --no-install-recommends i2pd

  if ! id "$FWD_USER" &>/dev/null; then
    adduser --system --group --no-create-home --home /nonexistent "$FWD_USER"
  fi

  info "Tor"
  [[ -f $TORRC.orig ]] || cp -a "$TORRC" "$TORRC.orig" 2>/dev/null || true
  cat > "$TORRC" <<EOF
# Written by setup-front.sh
SocksPort 127.0.0.1:9050
Log notice syslog
ClientOnionAuthDir $AUTH_DIR

$(if [[ $type == tor ]]; then printf '# The front: relays connect here.\nHiddenServiceDir %s\nHiddenServicePort 443 unix:%s\n' "$HS_FRONT" "$SOCK"; fi)

# Admin SSH. Client authorization required.
HiddenServiceDir $HS_ADMIN
HiddenServicePort 22 127.0.0.1:22
EOF
  [[ $type == i2p ]] && write_i2pd
  for d in "$HS_FRONT" "$HS_ADMIN" "$AUTH_DIR"; do
    install -d -o "$TOR_USER" -g "$TOR_USER" -m 0700 "$d"
  done
  install -d -o "$TOR_USER" -g "$TOR_USER" -m 0700 "$HS_FRONT/authorized_clients" "$HS_ADMIN/authorized_clients"
  echo "${core%.onion}:descriptor:x25519:$core_auth" > "$AUTH_DIR/core.auth_private"
  chown "$TOR_USER:$TOR_USER" "$AUTH_DIR/core.auth_private"
  chmod 600 "$AUTH_DIR/core.auth_private"

  if [[ -f /etc/apparmor.d/system_tor ]] && grep -q "local/system_tor" /etc/apparmor.d/system_tor; then
    mkdir -p /etc/apparmor.d/local
    printf '# Written by setup-front.sh\n%s/ r,\n%s rw,\n' "$SOCK_DIR" "$SOCK" > /etc/apparmor.d/local/system_tor
    apparmor_parser -r /etc/apparmor.d/system_tor || warn "apparmor reload failed"
  fi

  info "Forwarder"
  # socat: accept on the unix socket Tor delivers to, connect onward through
  # Tor's SOCKS port to the core's tunnel port. Bytes only; no parsing.
  cat > "$UNIT" <<EOF
[Unit]
Description=optes front forwarder (unix socket -> Tor -> core)
After=tor@default.service

[Service]
Type=simple
User=$FWD_USER
Group=$TOR_USER
UMask=0007
ExecStart=/usr/bin/socat $(listen_spec "$type") SOCKS4A:127.0.0.1:$core:443,socksport=9050
Restart=always
RestartSec=2
RuntimeDirectory=optes-front
RuntimeDirectoryMode=0750

# Only localhost: Tor's SOCKS port. Nothing else.
IPAddressDeny=any
IPAddressAllow=localhost
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
NoNewPrivileges=yes
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
CapabilityBoundingSet=
SystemCallArchitectures=native
SystemCallFilter=@system-service

[Install]
WantedBy=multi-user.target
EOF

  info "First admin key"
  gen_x25519
  local admin_priv=$CLIENT_PRIV
  echo "descriptor:x25519:$CLIENT_PUB" > "$HS_ADMIN/authorized_clients/admin1.auth"
  chown "$TOR_USER:$TOR_USER" "$HS_ADMIN/authorized_clients/admin1.auth"
  chmod 600 "$HS_ADMIN/authorized_clients/admin1.auth"

  info "Starting"
  systemctl daemon-reload
  systemctl enable tor.service
  systemctl restart tor@default.service
  systemctl enable --now optes-front.service
  if [[ $type == i2p ]]; then
    systemctl enable i2pd.service
    systemctl restart i2pd.service
    wait_for_file "$I2P_KEYS" 60 || die "i2pd did not create the tunnel keys; check: journalctl -u i2pd"
  else
    wait_for_file "$HS_FRONT/hostname" 60 || die "front onion not created; check: journalctl -u tor@default"
  fi
  wait_for_file "$HS_ADMIN/hostname" 60 || die "admin onion not created"

  info "apt through Tor"
  local f
  for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    [[ -f $f ]] || continue
    sed -i -E -e 's#^(URIs:[[:space:]]*)(https?://)#\1tor+\2#' \
      -e 's#^(deb(-src)?([[:space:]]+\[[^]]*\])?[[:space:]]+)(https?://)#\1tor+\4#' "$f"
  done
  systemctl disable --now systemd-timesyncd.service 2>/dev/null || true

  info "Firewall (clearnet SSH still open)"
  write_nft yes
  systemctl enable nftables.service
  nft -f "$NFT_CONF"

  local front admin
  front=$(front_address)
  admin=$(cat "$HS_ADMIN/hostname")
  cat <<EOF

=====================================================================
Front installed.

Front address:    $front  ($type)
Admin SSH onion:  $admin
Admin key (Orbot / Tor Browser / ClientOnionAuthDir):
  ${admin%.onion}:descriptor:x25519:$admin_priv

Next:
 1. On the core:  setup-core.sh add-front $type:$front:443
 2. Optional (tor fronts): restrict who can reach this front:  $0 add-client <relayname>
 3. Test SSH over the admin onion, then:  $0 lockdown --i-tested-onion-ssh
=====================================================================
EOF
}

cmd_add_client() {
  local name=${1:-}
  [[ $(cat "$STATE_DIR/type" 2>/dev/null) == tor ]] || die "add-client is for tor fronts only"
  valid_name "$name"
  [[ -e $HS_FRONT/authorized_clients/$name.auth ]] && die "client '$name' exists"
  gen_x25519
  echo "descriptor:x25519:$CLIENT_PUB" > "$HS_FRONT/authorized_clients/$name.auth"
  chown "$TOR_USER:$TOR_USER" "$HS_FRONT/authorized_clients/$name.auth"
  chmod 600 "$HS_FRONT/authorized_clients/$name.auth"
  systemctl reload tor@default.service
  local onion
  onion=$(cut -d. -f1 < "$HS_FRONT/hostname")
  echo "Line for the relay machine's tor ClientOnionAuthDir ($name.auth_private):"
  echo "  $onion:descriptor:x25519:$CLIENT_PRIV"
}

cmd_remove_client() {
  local name=${1:-}
  valid_name "$name"
  [[ -e $HS_FRONT/authorized_clients/$name.auth ]] || die "no client '$name'"
  rm -f "$HS_FRONT/authorized_clients/$name.auth"
  systemctl reload tor@default.service
  info "removed '$name'"
}

# Tor fronts: Tor delivers to a unix socket. I2P fronts: i2pd server tunnels
# need an IP address, so socat listens on localhost.
listen_spec() {
  if [[ $1 == i2p ]]; then
    echo "TCP-LISTEN:$I2P_PORT,bind=127.0.0.1,fork,reuseaddr"
  else
    echo "UNIX-LISTEN:$SOCK,fork,unlink-early"
  fi
}

write_i2pd() {
  [[ -f /etc/i2pd/i2pd.conf.orig ]] || cp -a /etc/i2pd/i2pd.conf /etc/i2pd/i2pd.conf.orig 2>/dev/null || true
  cat > /etc/i2pd/i2pd.conf <<EOF
# Written by setup-front.sh: hidden router, no transit, no local proxies.
notransit = true
nat = true
ipv6 = false
bandwidth = O
share = 0
tunconf = /etc/i2pd/tunnels.conf

[ntcp2]
published = false
[ssu2]
published = false
[trust]
hidden = true
[http]
enabled = false
[httpproxy]
enabled = false
[socksproxy]
enabled = false
[sam]
enabled = false
[i2pcontrol]
enabled = false
[reseed]
verify = true
EOF
  cat > /etc/i2pd/tunnels.conf <<EOF
# Written by setup-front.sh
[optes-front]
type = server
host = 127.0.0.1
port = $I2P_PORT
inport = 443
keys = optes-front.dat
EOF
}

# The front's public address: the onion, or the .b32.i2p computed from the
# i2pd keys file (sha256 of the destination; checked against i2pd 2.49).
front_address() {
  if [[ $(cat "$STATE_DIR/type" 2>/dev/null) == i2p ]]; then
    python3 - "$I2P_KEYS" <<'EOF'
import base64, hashlib, sys
d = open(sys.argv[1], "rb").read()
dest = d[:387 + int.from_bytes(d[385:387], "big")]
print(base64.b32encode(hashlib.sha256(dest).digest()).decode().lower().rstrip("=") + ".b32.i2p")
EOF
  else
    cat "$HS_FRONT/hostname"
  fi
}

cmd_lockdown() {
  [[ ${1:-} == --i-tested-onion-ssh ]] || die "first confirm SSH works through the admin onion, then pass --i-tested-onion-ssh"
  write_nft no
  nft -f "$NFT_CONF"
  info "clearnet SSH closed"
}

cmd_status() {
  local u
  local units=(tor@default optes-front nftables)
  [[ $(cat "$STATE_DIR/type" 2>/dev/null) == i2p ]] && units+=(i2pd)
  for u in "${units[@]}"; do
    printf '%-24s %s\n' "$u" "$(systemctl is-active "$u" 2>/dev/null || true)"
  done
  printf '%-24s %s\n' "type" "$(cat "$STATE_DIR/type" 2>/dev/null || echo -)"
  printf '%-24s %s\n' "front address" "$(front_address 2>/dev/null || echo -)"
  printf '%-24s %s\n' "admin onion" "$(cat "$HS_ADMIN/hostname" 2>/dev/null || echo -)"
  printf '%-24s %s\n' "relay clients" "$(find "$HS_FRONT/authorized_clients" -name '*.auth' -printf '%f ' 2>/dev/null)"
}

main() {
  local cmd=${1:-}
  shift || true
  case "$cmd" in
    install)       need_root; cmd_install "$@" ;;
    add-client)    need_root; cmd_add_client "$@" ;;
    remove-client) need_root; cmd_remove_client "$@" ;;
    lockdown)      need_root; cmd_lockdown "$@" ;;
    status)        need_root; cmd_status ;;
    *) sed -n '2,29p' "$0"; exit 1 ;;
  esac
}

main "$@"
