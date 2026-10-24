#!/usr/bin/env bash
# setup-core.sh — private Matrix core, phase 1a.
# Debian 13. Continuwuity with no network (systemd PrivateNetwork), reachable
# only through Tor onion services with client authorization.
#
# UNTESTED. Written 2026-10-24. Read the whole script before running it.
#
# Usage:
#   setup-core.sh install --server-name <56chars>.onion \
#                         --binary <path> --sha256 <hex> \
#                         [--media-retention 30d] [--io-uring off|on]
#   setup-core.sh add-client <name> [--service ingress|admin]
#   setup-core.sh remove-client <name> [--service ingress|admin]
#   setup-core.sh lockdown --i-tested-onion-ssh
#   setup-core.sh status
#
# Layout:
#   - continuwuity runs as user "continuwuity" in its own network namespace
#     containing only loopback. It listens on a unix socket. Unix sockets with a
#     filesystem path cross network namespaces, so Tor can reach it; nothing
#     else can, and continuwuity cannot open any outbound connection.
#   - Tor runs on the host. Two onion services, both requiring client auth:
#       ingress -> continuwuity socket (for fronts / test clients)
#       admin   -> sshd on 127.0.0.1:22
#   - nftables: outbound allowed only for the Tor user (plus loopback, DHCP,
#     IPv6 neighbour discovery). Everything else fails closed, including DNS.
#   - apt goes through Tor (tor+https) after install.

set -euo pipefail
umask 077

# ---- fixed paths -----------------------------------------------------------
SVC_USER=continuwuity
TOR_USER=debian-tor
BIN=/usr/local/bin/continuwuity
CONF_DIR=/etc/continuwuity
CONF=$CONF_DIR/continuwuity.toml
REG_TOKEN=$CONF_DIR/registration_token
SOCK_DIR=/run/continuwuity
SOCK=$SOCK_DIR/continuwuity.sock
STATE_DIR=/etc/matrix-core
HS_INGRESS=/var/lib/tor/matrix_ingress
HS_ADMIN=/var/lib/tor/admin_ssh
TORRC=/etc/tor/torrc
NFT_CONF=/etc/nftables.conf
SYSCTL_CONF=/etc/sysctl.d/90-matrix-core.conf
UNIT=/etc/systemd/system/continuwuity.service
RET_SVC=/etc/systemd/system/matrix-media-retention.service
RET_TIMER=/etc/systemd/system/matrix-media-retention.timer

# ---- helpers ---------------------------------------------------------------
die()  { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }
info() { echo "==> $*"; }

need_root() { [[ $EUID -eq 0 ]] || die "run as root"; }

valid_name() { [[ $1 =~ ^[A-Za-z0-9_-]{1,32}$ ]] || die "client name must be 1-32 chars of A-Z a-z 0-9 _ -"; }

hs_dir_for() {
  case "$1" in
    ingress) echo "$HS_INGRESS" ;;
    admin)   echo "$HS_ADMIN" ;;
    *) die "--service must be ingress or admin" ;;
  esac
}

# Generates an x25519 keypair for v3 onion client authorization.
# Sets globals CLIENT_PRIV and CLIENT_PUB (base32, no padding).
gen_x25519() {
  local tmp
  tmp=$(mktemp -d)
  openssl genpkey -algorithm x25519 -out "$tmp/k.pem" 2>/dev/null
  # PKCS#8 DER of an x25519 private key is 48 bytes; the raw key is the last 32.
  CLIENT_PRIV=$(openssl pkey -in "$tmp/k.pem" -outform DER | tail -c 32 | base32 | tr -d '=\n')
  # SPKI DER of the public key is 44 bytes; the raw key is the last 32.
  CLIENT_PUB=$(openssl pkey -in "$tmp/k.pem" -pubout -outform DER | tail -c 32 | base32 | tr -d '=\n')
  shred -u "$tmp/k.pem" 2>/dev/null || rm -f "$tmp/k.pem"
  rmdir "$tmp"
  [[ ${#CLIENT_PRIV} -eq 52 && ${#CLIENT_PUB} -eq 52 ]] || die "key generation produced unexpected output"
}

wait_for_file() {
  local f=$1 tries=${2:-60}
  for ((i = 0; i < tries; i++)); do
    [[ -s $f ]] && return 0
    sleep 1
  done
  return 1
}

# ---- nftables --------------------------------------------------------------
write_nft() {
  local keep_ssh=$1 ssh_rule=""
  [[ $keep_ssh == yes ]] && ssh_rule="    tcp dport 22 accept comment \"clearnet ssh, remove with: setup-core.sh lockdown\""
  cat > "$NFT_CONF" <<EOF
#!/usr/sbin/nft -f
# Written by setup-core.sh. Fail closed: only Tor may talk to the internet.
flush ruleset

table inet matrixcore {
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

  chain forward {
    type filter hook forward priority 0; policy drop;
  }

  chain output {
    type filter hook output priority 0; policy drop;
    oif lo accept
    ct state invalid drop
    ct state established,related accept
    meta skuid "$TOR_USER" accept
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

# ---- install ---------------------------------------------------------------
cmd_install() {
  local server_name="" binary="" sha="" retention="30d" io_uring="off"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --server-name)     server_name=${2:-}; shift 2 ;;
      --binary)          binary=${2:-}; shift 2 ;;
      --sha256)          sha=${2:-}; shift 2 ;;
      --media-retention) retention=${2:-}; shift 2 ;;
      --io-uring)        io_uring=${2:-}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done

  [[ $server_name =~ ^[a-z2-7]{56}\.onion$ ]] || die "--server-name must be a v3 onion address (56 chars + .onion)"
  [[ -f $binary ]] || die "--binary: file not found"
  [[ $sha =~ ^[0-9a-f]{64}$ ]] || die "--sha256 must be 64 lowercase hex chars"
  [[ $retention =~ ^[0-9]+[dwy]$ ]] || die "--media-retention must look like 30d, 8w or 1y"
  [[ $io_uring == on || $io_uring == off ]] || die "--io-uring must be on or off"

  local actual
  actual=$(sha256sum "$binary" | cut -d' ' -f1)
  [[ $actual == "$sha" ]] || die "sha256 mismatch: got $actual"

  # shellcheck disable=SC1091
  . /etc/os-release
  [[ ${VERSION_CODENAME:-} == trixie ]] || warn "written for Debian 13 (trixie); this is ${PRETTY_NAME:-unknown}"

  # A server_name can never change without wiping the database.
  if [[ -f $CONF ]] && ! grep -q "^server_name = \"$server_name\"$" "$CONF"; then
    die "existing config has a different server_name; refusing to change it"
  fi

  mkdir -p "$STATE_DIR"
  chmod 700 "$STATE_DIR"

  info "Installing packages (clearnet apt, first and only time)"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends tor apt-transport-tor nftables openssl coreutils apparmor-utils

  info "Kernel hardening sysctls"
  {
    echo "# Written by setup-core.sh"
    echo "user.max_user_namespaces = 0"
    echo "kernel.unprivileged_bpf_disabled = 1"
    echo "net.core.bpf_jit_harden = 2"
    echo "kernel.kptr_restrict = 2"
    echo "kernel.dmesg_restrict = 1"
    echo "kernel.kexec_load_disabled = 1"
    echo "kernel.yama.ptrace_scope = 2"
    echo "net.ipv4.ip_forward = 0"
    echo "net.ipv6.conf.all.forwarding = 0"
    if [[ $io_uring == off ]]; then
      echo "kernel.io_uring_disabled = 2"
    else
      echo "kernel.io_uring_disabled = 0"
    fi
  } > "$SYSCTL_CONF"
  sysctl --system > /dev/null

  info "Service user"
  if ! id "$SVC_USER" &>/dev/null; then
    adduser --system --group --home /var/lib/continuwuity --no-create-home "$SVC_USER"
  fi

  info "Binary"
  install -o root -g root -m 0755 "$binary" "$BIN"

  info "Config"
  mkdir -p "$CONF_DIR"
  chown root:"$SVC_USER" "$CONF_DIR"
  chmod 750 "$CONF_DIR"
  if [[ ! -s $REG_TOKEN ]]; then
    head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n' > "$REG_TOKEN"
    echo >> "$REG_TOKEN"
  fi
  chown root:"$SVC_USER" "$REG_TOKEN"
  chmod 640 "$REG_TOKEN"

  cat > "$CONF" <<EOF
# Written by setup-core.sh. Keys verified against the continuwuity config
# reference (main branch, Aug 2026). Check against the pinned tag.
[global]
server_name = "$server_name"

# Listen on a unix socket only. "address" and "port" must stay unset.
unix_socket_path = "$SOCK"
unix_socket_perms = 660
database_path = "/var/lib/continuwuity"

# No phone-home.
allow_announcements_check = false

# Closed server.
allow_federation = false
trusted_servers = []
forbidden_remote_server_names = [".*"]
allow_public_room_directory_over_federation = false
allow_inbound_profile_lookup_federation_requests = false
allow_device_name_federation = false

# Registration by token only. No [global.smtp] section, so no email capture.
allow_registration = true
registration_token_file = "$REG_TOKEN"
new_user_displayname_suffix = ""

# Metadata signals off server-wide.
allow_local_presence = false
allow_incoming_presence = false
allow_outgoing_presence = false
allow_local_read_receipts = false
allow_incoming_read_receipts = false
allow_outgoing_read_receipts = false
allow_local_typing = false
allow_incoming_typing = false
allow_outgoing_typing = false

# Privacy of directory and profiles.
require_auth_for_profile_requests = true
lockdown_public_room_directory = true

# URL previews stay off: the allowlists default to empty. Do not set them.

# Delayed release (optescontinuwuity patch): release events to clients every
# 15 s, timestamps rounded to the tick. Requires the patched binary; upstream
# continuwuity ignores the key.
release_interval_secs = 15

# Never store when or from where a device was last active
# (optescontinuwuity patch).
record_device_activity = false

# Logging and telemetry.
log = "warn"
log_colors = false
sentry = false
allow_otlp = false
allow_prometheus_metrics = false

# Media retention: run on SIGUSR2 (sent daily by matrix-media-retention.timer).
# Deletes ALL media (local too) older than the window. TEST the syntax.
admin_signal_execute = ["media delete-past-remote-media -b $retention --yes-i-want-to-delete-local-media"]
EOF
  chown root:"$SVC_USER" "$CONF"
  chmod 640 "$CONF"

  info "systemd units"
  # Group=$TOR_USER makes the socket's group debian-tor, so Tor can connect.
  # /var/lib/tor and /etc/tor are made inaccessible to continuwuity.
  cat > "$UNIT" <<EOF
[Unit]
Description=Continuwuity Matrix homeserver (no network, unix socket only)
After=local-fs.target

[Service]
Type=simple
User=$SVC_USER
Group=$TOR_USER
Environment=CONTINUWUITY_CONFIG=$CONF
ExecStart=$BIN
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

StateDirectory=continuwuity
StateDirectoryMode=0700
RuntimeDirectory=continuwuity
RuntimeDirectoryMode=0750

PrivateNetwork=yes
InaccessiblePaths=/var/lib/tor /etc/tor $STATE_DIR
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
RemoveIPC=yes
CapabilityBoundingSet=
AmbientCapabilities=
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @mount @debug @cpu-emulation @obsolete @raw-io @reboot @swap @module @clock

[Install]
WantedBy=multi-user.target
EOF

  cat > "$RET_SVC" <<EOF
[Unit]
Description=Trigger continuwuity media retention (SIGUSR2)

[Service]
Type=oneshot
ExecStart=/bin/systemctl kill --signal=SIGUSR2 continuwuity.service
EOF

  cat > "$RET_TIMER" <<EOF
[Unit]
Description=Daily continuwuity media retention

[Timer]
OnCalendar=daily
RandomizedDelaySec=1h
Persistent=true

[Install]
WantedBy=timers.target
EOF

  info "Tor"
  [[ -f $TORRC.orig ]] || cp -a "$TORRC" "$TORRC.orig" 2>/dev/null || true
  cat > "$TORRC" <<EOF
# Written by setup-core.sh. Original saved as torrc.orig.
SocksPort 127.0.0.1:9050
Log notice syslog

# Ingress for fronts / test clients. Client authorization required.
HiddenServiceDir $HS_INGRESS
HiddenServicePort 80 unix:$SOCK

# Admin SSH. Client authorization required.
HiddenServiceDir $HS_ADMIN
HiddenServicePort 22 127.0.0.1:22
EOF
  for d in "$HS_INGRESS" "$HS_ADMIN"; do
    install -d -o "$TOR_USER" -g "$TOR_USER" -m 0700 "$d" "$d/authorized_clients"
  done

  # Debian's tor runs under an AppArmor profile; allow it to reach the socket.
  if [[ -f /etc/apparmor.d/system_tor ]]; then
    if grep -q "local/system_tor" /etc/apparmor.d/system_tor; then
      mkdir -p /etc/apparmor.d/local
      cat > /etc/apparmor.d/local/system_tor <<EOF
# Written by setup-core.sh
$SOCK_DIR/ r,
$SOCK rw,
EOF
      apparmor_parser -r /etc/apparmor.d/system_tor || warn "apparmor reload failed"
    else
      warn "tor AppArmor profile has no local include; if tor gets 'Permission denied' on the socket, edit /etc/apparmor.d/system_tor"
    fi
  fi

  info "Starting services"
  systemctl daemon-reload
  systemctl enable --now continuwuity.service
  systemctl enable --now matrix-media-retention.timer
  systemctl enable tor.service
  systemctl restart tor@default.service

  wait_for_file "$HS_INGRESS/hostname" 60 || die "ingress onion hostname not created; check: journalctl -u tor@default"
  wait_for_file "$HS_ADMIN/hostname" 60 || die "admin onion hostname not created"
  wait_for_file "$SOCK" 30 || warn "continuwuity socket not present yet; check: journalctl -u continuwuity"

  info "First client keys"
  local admin_out="" ingress_out=""
  [[ -e $HS_ADMIN/authorized_clients/admin1.auth ]] || admin_out=$(add_client_key admin admin1)
  [[ -e $HS_INGRESS/authorized_clients/test1.auth ]] || ingress_out=$(add_client_key ingress test1)
  systemctl reload tor@default.service

  info "Switching apt to Tor"
  apt_via_tor

  info "Disabling clearnet time sync (it would be blocked anyway)"
  systemctl disable --now systemd-timesyncd.service 2>/dev/null || true

  info "Firewall (clearnet SSH still open)"
  write_nft yes
  systemctl enable nftables.service
  nft -f "$NFT_CONF"

  local ingress admin
  ingress=$(cat "$HS_INGRESS/hostname")
  admin=$(cat "$HS_ADMIN/hostname")

  cat <<EOF

=====================================================================
Core installed. Copy what you need below; client private keys are
NOT stored on this server and won't be shown again.

Server name (identity):   $server_name
Ingress onion (clients):  $ingress
Admin SSH onion:          $admin

First account: continuwuity ignores the configured token until the first
account exists. Get its one-time token with:
  journalctl -u continuwuity | grep -o 'using the registration token [^ ]*'
Later accounts use: $(cat "$REG_TOKEN")   (also in $REG_TOKEN)

$admin_out

$ingress_out

Next:
 1. Test SSH through the admin onion from your machine.
 2. Then close clearnet SSH:  $0 lockdown --i-tested-onion-ssh
 3. Register the first account through the ingress onion with the
    one-time token above. The first account becomes server admin.
 4. Back up /var/lib/continuwuity (contains the Matrix signing key),
    $HS_INGRESS and $HS_ADMIN — encrypted, offline.
=====================================================================
EOF
}

apt_via_tor() {
  local f
  for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    [[ -f $f ]] || continue
    cp -a "$f" "$STATE_DIR/$(basename "$f").pre-tor" 2>/dev/null || true
    # deb822: "URIs: https://..." ; one-line: "deb https://..."
    sed -i -E \
      -e 's#^(URIs:[[:space:]]*)(https?://)#\1tor+\2#' \
      -e 's#^(deb(-src)?([[:space:]]+\[[^]]*\])?[[:space:]]+)(https?://)#\1tor+\4#' \
      "$f"
  done
}

# Writes the public half into authorized_clients and prints the private half.
# Args: service name
add_client_key() {
  local service=$1 name=$2 dir onion=""
  dir=$(hs_dir_for "$service")
  [[ -e $dir/authorized_clients/$name.auth ]] && die "client '$name' already exists for $service"
  gen_x25519
  echo "descriptor:x25519:$CLIENT_PUB" > "$dir/authorized_clients/$name.auth"
  chown "$TOR_USER:$TOR_USER" "$dir/authorized_clients/$name.auth"
  chmod 600 "$dir/authorized_clients/$name.auth"
  [[ -s $dir/hostname ]] && onion=$(cut -d. -f1 < "$dir/hostname")
  cat <<EOF
--- client '$name' for $service ---
Private key (Orbot / Tor Browser asks for this):
  $CLIENT_PRIV
Line for a C tor ClientOnionAuthDir file ($name.auth_private):
  ${onion:-<onion-address-without-.onion>}:descriptor:x25519:$CLIENT_PRIV
EOF
}

cmd_add_client() {
  local name=${1:-} service=ingress
  shift || true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --service) service=${2:-}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  valid_name "$name"
  add_client_key "$service" "$name"
  systemctl reload tor@default.service
}

cmd_remove_client() {
  local name=${1:-} service=ingress dir
  shift || true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --service) service=${2:-}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  valid_name "$name"
  dir=$(hs_dir_for "$service")
  [[ -e $dir/authorized_clients/$name.auth ]] || die "no client '$name' for $service"
  rm -f "$dir/authorized_clients/$name.auth"
  systemctl reload tor@default.service
  info "removed '$name' from $service"
}

cmd_lockdown() {
  [[ ${1:-} == --i-tested-onion-ssh ]] || die "first confirm SSH works through the admin onion, then pass --i-tested-onion-ssh"
  write_nft no
  nft -f "$NFT_CONF"
  info "clearnet SSH closed. Admin access is through the admin onion only."
}

cmd_status() {
  local u
  for u in continuwuity tor@default nftables matrix-media-retention.timer; do
    printf '%-32s %s\n' "$u" "$(systemctl is-active "$u" 2>/dev/null || true)"
  done
  printf '%-32s %s\n' "socket" "$([[ -S $SOCK ]] && echo present || echo MISSING)"
  printf '%-32s %s\n' "ingress onion" "$(cat "$HS_INGRESS/hostname" 2>/dev/null || echo -)"
  printf '%-32s %s\n' "admin onion" "$(cat "$HS_ADMIN/hostname" 2>/dev/null || echo -)"
  printf '%-32s %s\n' "clearnet ssh open" "$(cat "$STATE_DIR/keep_clearnet_ssh" 2>/dev/null || echo -)"
  printf '%-32s %s\n' "ingress clients" "$(find "$HS_INGRESS/authorized_clients" -name '*.auth' -printf '%f ' 2>/dev/null)"
  printf '%-32s %s\n' "admin clients" "$(find "$HS_ADMIN/authorized_clients" -name '*.auth' -printf '%f ' 2>/dev/null)"
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
    *) sed -n '2,16p' "$0"; exit 1 ;;
  esac
}

main "$@"
