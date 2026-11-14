#!/usr/bin/env bash
# make-kit.sh — build the release binaries and pack a deploy kit.
#
# Usage (from anywhere inside the repository):
#   optes/make-kit.sh                # build, then pack
#   optes/make-kit.sh --no-build     # pack what is already in target/release
#   optes/make-kit.sh --allow-dirty  # also accept uncommitted changes
#   optes/make-kit.sh --out <dir>    # where the kit goes (default: target/kit)
#
# Result:
#   <out>/optes-kit-<commit>/        the kit
#   <out>/optes-kit-<commit>.tar.gz  the same, packed
#
# Kit layout:
#   core/   continuwuity optes-gateway optes-keys setup-core.sh
#   front/  setup-front.sh
#   relay/  optes-relay optes-keys setup-relay.sh
#   tools/  make-space.py example-server.toml
#   README  SHA256SUMS
#
# Rules the script enforces:
#   - The tunnel is built without cargo features. The feature `test-fronts`
#     lets a relay connect without Tor; such a build must never ship. After
#     the build the script asks the built optes-keys to sign a plain-TCP
#     front and stops if it agrees.
#   - Build paths (the home directory of whoever builds, the checkout path)
#     are mapped to /cargo, /src and /rust, so they do not end up in the
#     binaries. The script lists what is left and warns. One known rest: the
#     server binary names the checkout directory (see the check below).
#   - The server binary does not record the git remote URL or the commit.
#
# Build needs: the Rust toolchain from rust-toolchain.toml, libclang and
# liburing headers (HANDOFF.md §5). The first build takes long.

set -euo pipefail
umask 022

# libclang for bindgen: the newest installed LLVM unless LIBCLANG_PATH is set
# (Ubuntu 24.04: llvm-18, Debian 13: llvm-19).
find_libclang() {
  local d
  for d in $(printf '%s\n' /usr/lib/llvm-*/lib | sort -t- -k2 -n -r); do
    compgen -G "$d/libclang*.so*" > /dev/null && { echo "$d"; return; }
  done
}

die()  { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }
info() { echo "==> $*"; }

build=yes
allow_dirty=no
out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)    build=no; shift ;;
    --allow-dirty) allow_dirty=yes; shift ;;
    --out)         out=${2:-}; [[ -n $out ]] || die "--out needs a directory"; shift 2 ;;
    -h|--help)     sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel) || die "not inside the repository"
[[ -n $out ]] && out=$(realpath -m "$out")
cd "$repo"

for tool in git cargo sha256sum tar gzip install; do
  command -v "$tool" > /dev/null || die "missing tool: $tool"
done

commit=$(git rev-parse --short=12 HEAD)
commit_time=$(git log -1 --format=%ct HEAD)
upstream=$(cat optes/UPSTREAM)
name=optes-kit-$commit
if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
  [[ $allow_dirty == yes ]] || die "uncommitted changes; commit them or pass --allow-dirty"
  warn "uncommitted changes: the kit will not match commit $commit"
  name=$name-dirty
fi
out=${out:-$repo/target/kit}
kit=$out/$name

# ---- build -----------------------------------------------------------------
if [[ $build == yes ]]; then
  cargo_home=${CARGO_HOME:-$HOME/.cargo}
  export LIBCLANG_PATH=${LIBCLANG_PATH:-$(find_libclang)}
  [[ -n $LIBCLANG_PATH ]] || die "libclang not found: install libclang-dev or set LIBCLANG_PATH"
  export CARGO_INCREMENTAL=0
  # Keep build paths out of the binaries (panic messages and C asserts carry
  # source file names).
  # The toolchain path is in there too when the rust-src component is installed.
  sysroot=$(rustc --print sysroot)
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$repo=/src --remap-path-prefix=$sysroot=/rust"
  cmap="-ffile-prefix-map=$cargo_home=/cargo -ffile-prefix-map=$repo=/src"
  export CFLAGS="${CFLAGS:+$CFLAGS }$cmap"
  export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$cmap"

  # continuwuity's build script records the commit and the URL of the git
  # remote "origin" in the binary. The URL names the account of whoever
  # builds; the commit would force a rebuild after every commit, even one
  # that changes only documents. Fixed values instead: the kit's name and
  # README say which commit it is. The server then reports its version as
  # "<upstream version> (kit)".
  export GIT_COMMIT_HASH=kit GIT_COMMIT_HASH_SHORT=kit
  export GIT_REMOTE_URL=not-recorded GIT_REMOTE_COMMIT_URL=not-recorded

  info "build continuwuity (release)"
  cargo build --release --locked -p conduwuit
  info "build the tunnel (release, no features)"
  (cd optes/tunnel && cargo build --release --locked)
fi

server=target/release/conduwuit
tunnel=optes/tunnel/target/release
for f in "$server" "$tunnel/optes-relay" "$tunnel/optes-gateway" "$tunnel/optes-keys"; do
  [[ -x $f ]] || die "missing binary: $f (run without --no-build)"
done

# ---- checks on the built binaries ------------------------------------------
info "check: the tunnel build refuses plain-TCP fronts"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$tunnel/optes-keys" gen-ed25519 "$tmp/list" > /dev/null
"$tunnel/optes-keys" gen-x25519 "$tmp/gw" > /dev/null
onion=$(printf 'a%.0s' {1..56}).onion
"$tunnel/optes-keys" sign-list --key "$tmp/list.key" --version 1 --gateway-key "$tmp/gw.pub" \
  --front "tor:$onion:443" --out "$tmp/good" > /dev/null \
  || die "optes-keys could not sign a list with an onion front"
if "$tunnel/optes-keys" sign-list --key "$tmp/list.key" --version 1 --gateway-key "$tmp/gw.pub" \
  --front tcp:127.0.0.1:443 --out "$tmp/bad" > /dev/null 2>&1; then
  die "this tunnel build accepts plain-TCP fronts (feature test-fronts). It must not ship. Run 'cargo clean' in optes/tunnel and build again."
fi

# Known: continuwuity embeds its web pages together with their full source
# paths (crate memory-serve), which the flags above do not reach. So the
# server binary names the checkout directory about ten times. To avoid that,
# build from a checkout in a directory with a neutral name, as a neutral user.
info "check: build paths in the binaries"
for f in "$server" "$tunnel/optes-relay" "$tunnel/optes-gateway" "$tunnel/optes-keys"; do
  found=$(grep -a -o -E "($HOME|$repo)/[A-Za-z0-9_./-]{0,60}" "$f" || true)
  [[ -z $found ]] || warn "$f contains $(wc -l <<< "$found") build path(s), for example: $(head -n 1 <<< "$found")"
done

# ---- assemble --------------------------------------------------------------
info "assemble $kit"
rm -rf "$kit"
mkdir -p "$kit/core" "$kit/front" "$kit/relay" "$kit/tools"
install -m 0755 "$server"               "$kit/core/continuwuity"
install -m 0755 "$tunnel/optes-gateway" "$kit/core/optes-gateway"
install -m 0755 "$tunnel/optes-keys"    "$kit/core/optes-keys"
install -m 0755 optes/setup-core.sh     "$kit/core/setup-core.sh"
install -m 0755 optes/setup-front.sh    "$kit/front/setup-front.sh"
install -m 0755 "$tunnel/optes-relay"   "$kit/relay/optes-relay"
install -m 0755 "$tunnel/optes-keys"    "$kit/relay/optes-keys"
install -m 0755 optes/setup-relay.sh    "$kit/relay/setup-relay.sh"
install -m 0755 optes/tools/make-space.py        "$kit/tools/make-space.py"
install -m 0644 optes/tools/example-server.toml  "$kit/tools/example-server.toml"

server_sha=$(sha256sum "$kit/core/continuwuity" | cut -d' ' -f1)

cat > "$kit/README" <<EOF
optes deploy kit
================

Source commit:   $commit
Upstream base:   continuwuity $upstream
Tunnel protocol: version 2. Relay, gateway and optes-keys must all come from
                 the same kit.
Binaries:        x86_64 Linux, built without the test-fronts feature.

Check the files first, in this directory:

    sha256sum -c SHA256SUMS

Contents
--------
core/    for the core server (Debian 13): continuwuity, optes-gateway,
         optes-keys, setup-core.sh
front/   for a front server (Debian 13): setup-front.sh (needs no binary)
relay/   for your own machine (Fedora or Debian): optes-relay, optes-keys,
         setup-relay.sh
tools/   make-space.py and an example server layout

Order
-----
core -> relay straight to the core (test) -> front -> relay through the front.
Every script prints its usage at the top of the file. Read it before running.

1. Core. Copy core/ to the server, then as root:

    ./setup-core.sh install --server-name <your 56 chars>.onion \\
      --binary ./continuwuity \\
      --sha256 $server_sha \\
      --tunnel-bin-dir .

   Test SSH over the admin onion it prints. Only then:

    ./setup-core.sh lockdown --i-tested-onion-ssh

2. Front. Copy front/ to the front server, then as root:

    ./setup-front.sh install --core <ingress>.onion --core-auth <key>

   The key comes from the core: setup-core.sh add-client <front name>.
   Then on the core: setup-core.sh add-front tor:<front>.onion:443

3. Relay. Copy relay/ to your machine, then as your normal user:

    ./setup-relay.sh install --relay-bin ./optes-relay \\
      --front-list <frontlist.signed> --list-key <key the core printed>

   Then on the core: setup-core.sh add-device <name> <device key>
   Point the Matrix client at http://127.0.0.1:8008

If a step fails on the core, collect this before any reboot (logs are kept in
memory only):

    setup-core.sh status
    journalctl -u tor@default -u continuwuity -u optes-gateway
EOF
chmod 0644 "$kit/README"

sums=$(cd "$kit" && find . -type f -printf '%P\n' | LC_ALL=C sort | xargs sha256sum)
echo "$sums" > "$kit/SHA256SUMS"
chmod 0644 "$kit/SHA256SUMS"

# Same input, same archive: fixed order, owner and times.
find "$kit" -exec touch -h -d "@$commit_time" {} +
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$commit_time" \
  -C "$out" -cf - "$name" | gzip -n -9 > "$out/$name.tar.gz"

info "done"
echo "kit:     $kit"
echo "archive: $out/$name.tar.gz"
echo "sha256:  $(sha256sum "$out/$name.tar.gz" | cut -d' ' -f1)"
