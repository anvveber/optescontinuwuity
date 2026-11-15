# optes — project state

Written 2026-11-09, updated 2026-11-10 (audit and its fixes: §4b) and 2026-11-14 (kit
script: §4c; review of spaces, template tool and tests: §4d; first run over real Tor:
§4e). Read this first, then `optes/handbook.md` (design, threat model, §14 status).

`main` holds everything. This repository is a fork of continuwuity (https://github.com/continuwuity/continuwuity), based on tag
**v26.9.1** (`optes/UPSTREAM`). Every fork-specific file lives in `optes/`, except the server
patches under `src/`.

## 1. What this is

A private Matrix homeserver for a small group. It hides as much metadata as possible from
network observers and from whoever hosts it. Federation is off.

```
Matrix client (any: Cinny, FluffyChat, Element)
  → http://127.0.0.1:8008  optes-relay   (user's own machine)
  → Tor/I2P → front        (disposable VPS, blind byte pipe, socat)
  → Tor → core's ingress onion :443
  → optes-gateway          (no network, unix sockets only)
  → continuwuity           (no network, unix socket only)
```

- **Core:** the VPS running continuwuity and optes-gateway, both under systemd
  `PrivateNetwork=yes`. Tor runs on the host. nftables allows outbound traffic only for
  the Tor user. Admin access is SSH over a second onion, with client auth.
- **Tunnel (`optes/tunnel/`, Rust):**
  - Noise IK (X25519, ChaCha20-Poly1305, BLAKE2s) between relay and gateway.
  - Fixed 2048-byte frames.
  - Privacy levels: 1 = bursts padded to a power of two, 2 = sending only at window
    boundaries, 3 = constant rate (one frame per 200 ms).
  - Relays find fronts through a front list signed with Ed25519. The list key is pinned
    in every relay. Relays dial only onion and b32.i2p addresses.
  - The handshake carries a counter; the gateway refuses a handshake sent twice.
  - Rekeying every 2^16 frames.
  - Full spec: `optes/phase2-protocol-spec.md`.
- **Fronts:** `setup-front.sh --type tor|i2p`. A front can't read the traffic and can be
  burned and replaced.

## 2. Design decisions

| Topic | Decision |
|---|---|
| Server name | A v3 onion address made with mkp224o. Its key stays offline and is never served; only the name is used. |
| Federation | Off |
| Nym | Ignored for now |
| OS | Debian 13 (trixie) on servers. Relays: Fedora or Debian. |
| Client | No client fork. The local relay makes any Matrix client work. |
| Delayed release | 15 s tick |
| Bouncer | Dropped, replaced by `record_device_activity = false` plus the relay (`bouncer-decision.md`) |
| Spaces (Discord-style) | Wanted: auto-join of space channels; space/room creation restricted by power level; template tool. Not wanted: onboarding auto-join, roles. |
| Retention | Per channel, as a room option. Whole events are deleted. Default: keep forever. Media: server-wide 30-day rule (it applies to permanent channels too). |
| Require encryption | Yes |
| Less fingerprinting | Yes (`hide_server_identity`) |
| Port 80 direct ingress | Closed by default when the tunnel is installed (`setup-core.sh direct-ingress on` opens it for testing) |

Parked:
- encryption at rest
- registration and login rate limits (not needed yet)
- stripping unsigned metadata (waiting for upstream's plan)
- a paywall for server creation

## 3. What's built (all on `main`, all e2e-tested locally without Tor)

Server patches (in `src/`, config keys in `src/core/config/mod.rs` and `conduwuit-example.toml`):

| Config key | What | Main code |
|---|---|---|
| `release_interval_secs` | Delayed release: events reach clients only at tick boundaries; timestamps are rounded up to the tick. Create events keep their real timestamp, because otherwise two rooms made in one tick would get the same v12 room ID. | `src/service/release/`, `src/service/sync/mod.rs`, `rooms/timeline/create.rs`, `rooms/state_accessor/user_can.rs` |
| `record_device_activity` | Off: no last-seen time or IP stored per device | |
| `auto_join_space_children`, `allow_space_creation`, `restrict_room_creation_to_spaces` | Discord-style servers | `src/service/space_autojoin/`, `src/api/client/room/create.rs` |
| `require_encryption` | New rooms are encrypted; unencrypted messages, stickers and polls are refused. Reactions and redactions are allowed. The admin room is exempt. | `src/api/client/send.rs`, `room/create.rs` |
| `hide_server_identity` | Removes the version endpoint, landing page, about page and debug pages; uses a neutral cookie name | `src/api/router.rs`, `src/web/mod.rs`, `api/client/unversioned.rs` |
| `allow_room_retention`, `retention_purge_interval_secs` | MSC1763 `m.room.retention`: expired events are hidden at once and purged from the DB by a background job. State events, forward extremities and the newest event are kept. | `src/service/retention/` |
| `restrict_request_hosts`, `allowed_hosts`, `allowed_origins` | Browser guard: a website open in a user's browser can't use the server through the local relay (Host must be an IP, localhost or onion/i2p name; requests with an unlisted `Origin` are refused) | `src/router/request.rs` (`guard`), `src/router/layers.rs` |
| `allow_pushers` | Off: push registrations are answered with success and not stored | `src/api/client/push.rs` |
| `allow_openid_tokens` | Off: OpenID token requests are refused | `src/api/client/openid.rs` |

Everything else:
- **Tunnel:** `optes/tunnel/`, with binaries `optes-relay`, `optes-gateway` and `optes-keys`.
  - Front-list renewal: the core re-signs weekly; the relay checks every 6 h.
  - A relay whose lists have all expired can still reach the gateway, but only to fetch a
    new list.
  - Fuzzing: 5 cargo-fuzz targets in `tunnel/fuzz/`, ~19M runs, no crashes.
- **Setup scripts:**
  - `setup-core.sh` (install, add-client, add-device, add-front, remove-front,
    renew-front-list, direct-ingress, allow-origin, remove-origin, lockdown, status)
  - `setup-front.sh`
  - `setup-relay.sh`
- **Tools:**
  - `tools/make-space.py`: builds a whole server (space, categories, channels) from a TOML file
  - `update-upstream.sh`: rebases onto a new upstream tag and runs every test
  - `make-kit.sh`: builds the release binaries and packs the deploy kit (§4c)
- **Tests:** `optes/test/` (see §5)
  - `home-test.sh` is a user-level real-Tor trial kit, no longer the plan (see §6).

Design notes for each feature: `delayed-release-spec.md`, `bouncer-decision.md`,
`phase2-protocol-spec.md`, `spaces.md`, `hardening.md`, `retention-design.md`,
`verification-notes.md`.

## 4. What has NOT been verified

- **Real Tor: in VMs only (§4e).** Core, relay and a Tor front ran in Debian 13
  VMs on the real Tor network. Nothing has run on a VPS (provider image, real IP
  address, memory limits), or on I2P.
- Still never exercised:
  - `setup-front.sh lockdown`; a front or core in daily use (T7)
  - level 1 padding on the wire
  - `setup-relay.sh` on Fedora (ran on Debian 13 only)
  - an I2P front on the real I2P network
  - a relay machine that sleeps and wakes (recovery of the relay not shown)
- **One audit (`audit-2026-11-10.md`), not independent. No outside review yet.**
- **The browser guard was tested with crafted requests only**, not with real clients or
  browsers. The origins that `setup-core.sh` allows for Element Desktop and Tauri apps
  (Cinny desktop) are from memory, not from a test.

## 4a. Dry-run of 2026-11-09 (Debian 13.7 nocloud image under QEMU, kernel 6.12, AppArmor on)

The VM could not reach the Tor network, so every Tor hop was replaced by a local stand-in.

Bugs found and fixed (the scripts on `main` have the fixes):

| Script | Bug | Fix |
|---|---|---|
| core | `continuwuity` links `liburing.so.2`; the package was not installed, so the service crash-looped | `liburing2` added to the apt line |
| core | `Group=debian-tor` replaced the service's own group, so it could not read `/etc/continuwuity` and failed with "missing field `server_name`" | `SupplementaryGroups=continuwuity` |
| core, front | An SSH session open when the firewall loaded froze, so the final summary (with keys that are shown once) never arrived | `tcp sport 22 accept` in the output chain while clearnet SSH is open; `lockdown` removes it |
| core, front | apt over Tor did nothing on Debian cloud images (sources are `mirror+file:/etc/apt/mirrors/*.list`); apt was simply blocked | the mirror list files get the `tor+` prefix too |
| core | client-auth files were written after Tor first started | written before Tor starts |
| core | `status` said `nftables inactive`; `direct-ingress off` exited with status 1 after succeeding | fixed |
| relay | the script noticed a missing `optes-keys` only after editing torrc | check moved to the start |
| deploy kit | `relay/` had no `optes-keys`; `setup-front.sh` and `setup-relay.sh` were not executable | scripts are now mode 755 in the repo. The kit was packaged by hand then; since 2026-11-14 `make-kit.sh` does it (§4c) |

What passed after the fixes:
- core: `install`, then continuwuity and optes-gateway running under their systemd
  sandboxes with `kernel.io_uring_disabled=2`; all subcommands; both timers' services;
  `lockdown`; reboot (everything came back, clearnet SSH stayed closed).
- relay binary → stand-in SOCKS proxy running as `debian-tor` → `gateway.sock` →
  continuwuity: first account registered with the one-time token, and
  `make-space.py example-server.toml` built the whole server.
- front: `install` for `--type tor` and `--type i2p`; the forwarder passes a connection
  to Tor's SOCKS port. The `.b32.i2p` address was computed, not checked against i2pd.
- relay: `setup-relay.sh install`, `front-auth`, `status` as a normal user on Debian 13.

Open points seen during the run:
- **I2P fronts and DNS:** on images that use systemd-resolved (`nameserver 127.0.0.53`),
  the firewall blocks all DNS, so i2pd cannot resolve its reseed hosts once the firewall
  is up. Needs a decision (allow DNS on I2P fronts, or reseed another way).
- **Clock:** time sync is off and NTP is blocked on core and fronts. Nothing corrects
  clock drift; Tor needs a roughly correct clock.
- **Relay started before `add-device`:** the gateway holds an unknown device about 30 s
  before closing, so the first connection takes that long. Add the device first.
- `allow_prometheus_metrics` in the generated config is unknown to continuwuity 26.9.1
  (ignored with a warning).
- The firewall assumes sshd on port 22.

## 4b. Audit of 2026-11-10 and its fixes

Findings and what was done about each: `audit-2026-11-10.md`. What a deployer must know:

- **Tunnel protocol is now version 2.** Relay, gateway and `optes-keys` must all be new;
  old and new don't talk to each other (the handshake fails silently).
- **`optes-gateway` needs `--state-dir`** (handshake counters; `/var/lib/optes-gateway`).
- **Relays dial only `<56 chars>.onion` and `<52 chars>.b32.i2p` fronts.** Plain TCP
  fronts exist only in test builds (`--features test-fronts`). Never ship such a build.
- **`setup-core.sh install` makes no ingress key.** Make one per front, and one for a
  test relay: `setup-core.sh add-client <name>`. Until then the ingress onion is locked
  with a key nobody holds (`locked.auth`). Reason: Tor opens an onion service to everyone
  when no client is listed. The same lock is applied when the last key is removed.
- **Tor fronts require a client key by default.** `setup-front.sh install` prints the key
  `relays` once; each relay needs `setup-relay.sh front-auth <front>.onion <key>`.
  `--open` gives the old behaviour.
- **Web-based clients need their origin allowed** or they get 403 "This origin may not
  use this server": `setup-core.sh allow-origin <origin>`. Native clients are not affected.
- **OAuth login is off** (password login only), pushers and OpenID tokens are off.
- **Logs are in memory only** on core and fronts (`journalctl` is empty after a reboot),
  and the gateway and relay log warnings only. To debug a relay:
  `systemctl --user edit optes-relay` → `Environment=RUST_LOG=info`.
- **continuwuity's socket** belongs to group `matrix-sock` while port 80 is closed;
  `direct-ingress on|off` now restarts continuwuity.

How the fixes were tested (2026-11-10, no Tor or I2P anywhere):
- Unit tests: tunnel (22, incl. a replayed handshake and the front address rules),
  router guard (2).
- e2e: every suite in §5, delayed release on and off; `run-browser-guard-e2e.sh` also
  against the release binary.
- By hand with the built binaries: a recorded handshake sent again gets no answer; a
  list with a `tcp` front is refused by `sign-list` and ignored by the relay.
- Debian 13 VM (as in §4a), with the release binaries of this commit:
  - core install; relay → stand-in SOCKS → gateway → continuwuity at levels 1 and 2;
    first account; `make-space.py`.
  - Through the relay: a request with a website `Origin` or a DNS-name `Host` gets 403,
    `vector://vector` and no `Origin` get 200, `auth_metadata` is 404.
  - Socket group and Tor's access to it in both `direct-ingress` modes.
  - `allow-origin` / `remove-origin`, the locked onion after removing the last ingress
    key, `lockdown`, reboot.
  - front install (default and `--open`), `add-client`, `remove-client`.
  - relay install as a normal user.
- Not tested: real Matrix clients against `allowed_origins`; a real browser; the I2P
  front after these changes; Fedora; the fuzz targets were only compiled.

Open after the audit (not fixed): full vanguards on the core (6), device names visible
to other members (12), low items L1 and L4–L7 (L7 partly done, §4c).

## 4c. Kit script (2026-11-14)

`optes/make-kit.sh` builds the release binaries and packs the kit. Layout and
options: the header of the script. What it does besides copying files:

- **No test build ships.** The tunnel is built without cargo features. Then the
  script asks the built `optes-keys` to sign a plain-TCP front and stops if it
  agrees. Checked both ways: a release build refuses, a `test-fronts` build signs.
- **Build paths.** Cargo's directory, the toolchain's directory and the checkout
  are mapped to `/cargo`, `/rust` and `/src`. The tunnel binaries then contain no
  path of the build machine.
  - Not solved: the server binary still names the checkout directory 10 times.
    Its web pages are embedded together with their source paths (crate
    `memory-serve`). The script warns. Build from a checkout in a directory with
    a neutral name if that matters. This belongs to reproducible builds (audit L7).
- **No git remote, no commit in the server binary.** continuwuity's build script
  would record the URL of the remote `origin` and the commit. The kit build sets
  fixed values; the server reports its version as `26.9.1 (kit)`. The kit's name
  and README give the commit. A commit that changes no source needs no rebuild.
- **Same input, same archive** (fixed order, owner and times in the tar file).
  Whether two machines produce the same binaries is not checked.

How it was tested (no Tor or I2P anywhere):
- Every e2e suite of §5 against the release server binary, delayed release on
  and off. `run-tunnel-e2e.sh` used the debug `test-fronts` tunnel build,
  `run-socks-path-e2e.sh` the release tunnel build. Tunnel unit tests (22).
- Debian 13.7 VM as in §4a, with a kit made by the script:
  - `sha256sum -c SHA256SUMS`; scripts and binaries have mode 755.
  - Core `install` with the command printed in the kit's README. `status`: all
    services active. After a reboot: all active again.
  - The kit's relay binary → SOCKS stand-in running as `debian-tor` →
    `gateway.sock` → continuwuity, at levels 1 and 2. First account with the
    one-time token. `make-space.py example-server.toml` through the tunnel
    (7 minutes at level 2).
  - Through the relay: a website `Origin` gets 403, a DNS-name `Host` gets 403,
    `auth_metadata` is 404.
  - `setup-relay.sh install`, `front-auth` and `status` as a normal user, from
    the kit's `relay/` directory.
- Not tested: `setup-front.sh` from the kit (the script has not changed since
  §4b); `lockdown`; Fedora; the server's unit tests (`release::`, `sync::`,
  `guard_tests::`), which need a debug build.

## 4d. Review of 2026-11-14

`space_autojoin`, `make-space.py` and the tests were read line by line. Findings
14–16 and L9–L11 are in `audit-2026-11-10.md` (last section). What a deployer must
know:

- **A room is a channel of a space only if the room names the space**
  (`m.space.parent`, or a restricted join rule for the space's members). Before,
  a space moderator could remove a member from any room on the server, or join
  members to rooms outside the space. Details and consequences: `spaces.md`.
  This is a server change: it needs the new server binary.
- **`make-space.py` never uses a proxy.** Before, `http_proxy` in the
  environment sent the login, password included, to that proxy.
- The tunnel test now checks the traffic shape of levels 2 and 3 over the
  whole session, not only while idle.

Tested: `run-spaces-e2e.sh` with new cases (they fail on the binary from before
the fix and pass after it), with and without `RELEASE=15`; every other e2e
suite again; `make-space.py` against a recording proxy. All with the release
binaries. Not tested: real clients ("add existing room", room creation inside a
space); the server's unit tests.

Open: level-1 padding on the wire, the test gaps in L10, `home-test.sh` (L11).

## 4e. First run over real Tor (2026-11-14)

Three Debian 13 VMs (`vm.py`, KVM) on one host, all on the real Tor network: core,
relay (normal user), Tor front. Kit from `make-kit.sh` at `fd04b47`; the script fixes
below were copied in by hand. Throwaway server name and keys, VMs deleted afterwards.

Passed, in plan order:
- Core `install` from the kit's README: all services active; Tor bootstraps; both
  onions published (the relay fetched the ingress descriptor). A fresh
  `apt-get update` fetched 16 index files over `tor+https`, a package download
  worked; clearnet HTTPS and DNS time out.
- Relay straight to the core (`add-client`, `add-front` of the core's own ingress,
  `front-auth`, `add-device`): `curl 127.0.0.1:8008/_matrix/client/versions` → 200.
  **First proof that Tor reaches the gateway socket through AppArmor.**
- First account with the one-time token (2 s at level 1); `make-space.py
  example-server.toml` (55 s at level 1).
- Level 2: each request takes one 15 s window. Room creation outside a space,
  joining the invite-only space and a plaintext message are refused, as configured.
- Level 3 after reinstalling the relay with `--level 3`.
- SSH over the admin onion with the admin client key (`torsocks ssh`); `lockdown`
  given over that SSH; clearnet SSH closed; reboot over onion SSH; everything came
  back, clearnet SSH still closed, the relay reconnected by itself.
- Front: `setup-front.sh install` with its own ingress key; `add-front`,
  `front-auth`, `remove-front` of the direct entry, `remove-client` of the relay's
  direct key. The relay then connects only through the front: its stream goes to
  the front's onion, a socat child on the front carries it.
- Traffic on the relay's link to its Tor guard (`tcpdump` in the relay VM):
  - level 3: 13.45 KB/s out and in, the same in idle seconds and during 50 s of
    large uploads; ±4% in 5 s bins. Payload is 10.24 KB/s; the rest is Tor.
  - level 2: one burst of exactly 8804 bytes out every 15 s, at second 0 of the
    window (gateway clock), whether or not a request was waiting; the same inbound
    one second later. Requests sent at other times left no trace of their own.
    Between bursts only Tor's own traffic (≤ 1.1 KB per second).
  - level 1: not checked.

Found and fixed (details: `audit-2026-11-10.md`, last section):
- `remove-client` did not cut off a client that already held the descriptor
  (finding 17). It now restarts Tor. **Deployers: after `remove-client`, every
  tunnel drops and reconnects.**
- `setup-relay.sh front-auth` right after `install`: Tor never read the key (L12).
- `setup-relay.sh install` again with another `--level` kept the old relay
  running (L13).
- The retention test failed on two harmless startup errors on this machine (§5).

Checked, not a code bug:
- Spec §6 was wrong: Tor puts all relay connections through one front on one
  circuit to the core (audit L4). Spec corrected.
- The first onion connection after a Tor start is often slower than the relay's
  120 s timeout; the relay retries and the next try works (audit, last section).
  Expect up to a few minutes after installing, restarting or `remove-client`.

Also run: `update-upstream.sh --test-only` on `fd04b47`. The server's
unit tests pass (`release::`, `sync::`: 11; `guard_tests::`: 2); all e2e suites
pass (the retention suite after its fix).

Still needed: a VPS run (§6), daily use (T7), level 1 on the wire, the front's
`lockdown`.

## 5. Building and testing

Build environments used so far: Ubuntu 24.04, and Debian 13 (LLVM 19), Rust toolchain
1.98.1. On machines with `/tmp` on tmpfs or
with 4+ cores, a debug server logs two harmless startup errors from upstream code
(`DEVNAME not found`, `mallctl: …`); the retention test skips them by name.
- System packages: `libclang-dev` (or a versioned one such as `libclang-18-dev`),
  `liburing-dev`, `clang`, `pkg-config`, `build-essential`
- `make-kit.sh` and `update-upstream.sh` use the newest `/usr/lib/llvm-*/lib`
  unless `LIBCLANG_PATH` is set
- Release builds use `CARGO_INCREMENTAL=0`; the disk allowance is small, and
  `target/debug` may have to be deleted.
- Binaries built here need glibc ≥ 2.39, so they run on Debian 13 but not Debian 12.
- The e2e tests need `python3`, `curl` and `socat`. Behind an HTTP proxy, set
  `no_proxy=127.0.0.1,localhost`.
- A debug build plus the unit-test build needs about 25 GB in `target/`.

```
optes/make-kit.sh                             # release build of everything + the deploy kit in target/kit/
cargo build --release -p conduwuit            # → target/release/conduwuit (deploy as "continuwuity")
(cd optes/tunnel && cargo build --release && cargo test)
optes/update-upstream.sh --test-only          # builds debug + runs every e2e suite
```

Binaries for a server come from `make-kit.sh`, not from a plain `cargo build`: a
plain build records the build machine's paths and the git remote URL in the
binaries (§4c). The release build alone needs about 3 GB in `target/` and took
about 40 minutes on 2 cores.

The tunnel has a second, test-only build (`cargo build --features test-fronts --target-dir
target/test-fronts`): its relay can dial a plain TCP front. `run-tunnel-e2e.sh` needs it;
`run-socks-path-e2e.sh` needs the normal build. `update-upstream.sh` builds both.

e2e suites (each prints PASS/FAIL lines and ends with ALL PASSED):
- `run-e2e.sh` (delayed release, at 15 s and at 0)
- `run-spaces-e2e.sh` and `run-retention-e2e.sh` (with and without `RELEASE=15`)
- `run-hardening-e2e.sh` (on and off)
- `run-browser-guard-e2e.sh` (on and off)
- `run-tunnel-e2e.sh` (levels 1–3)
- `run-socks-path-e2e.sh` (tor and i2p)

Test VMs for the setup scripts: `optes/test/vm.py` (header: usage). `create` once
downloads the Debian 13 nocloud image, checks it against Debian's SHA512SUMS and
prepares a clean disk (SSH key, larger disk). `start` boots a fresh copy each time;
`--name` runs a second VM next to it (e.g. core and relay). Uses KVM when available.
Tested 2026-11-14 without KVM: `create`, two VMs at once, `ssh`, `copy`, `stop`; and
with KVM: `create` (4 minutes), three VMs at once (§4e). The setup scripts change firewall, sysctls, journald and apt sources:
run them only in such a VM.

**Run them with delayed release on as well as off.** A real bug, the room-ID collision,
only appeared with release on.

## 6. Next step: test on a VPS

This tests the real `setup-core.sh`.

VPS requirements:
- **KVM** (not OpenVZ or LXC)
- **Debian 13** image
- IPv4
- 1–2 vCPU, 1–2 GB RAM, 20–40 GB SSD
- terms of service that allow Tor onion services

A front needs only the smallest KVM plan. Traffic at level 3 is about 53 GB per month per
relay (both directions), plus Tor overhead.

A test VPS is **throwaway**. If it was paid in a traceable way, don't reuse its keys,
onions or the server name for the real deployment. Choosing the real host and an
anonymous payment method ("decision E") is still open.

Test order:
1. **Done 2026-11-09 (§4a):** dry-run of all three scripts in a local Debian 13 VM.
   **Done 2026-11-14 (§4e):** steps 2–4 below in Debian 13 VMs over real Tor; on a
   VPS they are still to do.
2. **Core** (the kit's README has this command with the checksum filled in):
   ```
   setup-core.sh install --server-name <mkp224o>.onion --binary continuwuity \
     --sha256 <hex> --tunnel-bin-dir <dir>
   ```
   Then test SSH over the admin onion, then `lockdown --i-tested-onion-ssh`.
3. **Relay straight to the core (test only, no front).** This needs no code change:
   - On the core:
     - `setup-core.sh add-client laptop` gives an ingress onion auth key (install no
       longer makes one).
     - `setup-core.sh add-front tor:<core-ingress>.onion:443` puts the core itself in
       the front list.
   - On the laptop: `setup-relay.sh install …`, then
     `setup-relay.sh front-auth <core-ingress>.onion <key>`.
   - On the core: `setup-core.sh add-device laptop <device key>`.
   - Register the first account with the one-time token from
     `journalctl -u continuwuity | grep -o 'using the registration token [^ ]*'`.
     The first account becomes server admin. (Logs are in memory only: after a reboot
     continuwuity prints a new token at start, as long as no account exists.)
   - If the client gets 403 "This origin may not use this server", it is a web-based
     client: `setup-core.sh allow-origin <its origin>`.
4. **Real front:** `setup-front.sh install --core <ingress>.onion --core-auth <key from
   setup-core.sh add-client <frontname>>`.
   - Then `setup-core.sh add-front tor:<front>.onion:443` and
     `remove-front tor:<core-ingress>.onion:443`.
   - On the laptop: `setup-relay.sh front-auth <front>.onion <relay key the front
     printed>`.
   - A front on the same VPS tests the mechanics but hides nothing.
5. Try every privacy level, the spaces template, retention, and the 15 s tick in a real
   client.
6. Fix what breaks. Then the **audit**:
   - reviewers who haven't seen how the code was written
   - fuzzing extended where useful
   - traffic checks on the tunnel's sizes and timing
   - ideally a human cryptographer for the Noise glue
7. Decision E, then the real deployment.

Possible later work:
- a post-quantum hybrid (ML-KEM) in the tunnel, after the audit
- a frame-size histogram from the relay to tune the 2048-byte frame size

## 7. Repository conventions

- Feature branches are merged into `main` with `--ff-only`.
- Keep `optes/handbook.md` §14 (status map) and `optes/README.md` current when features
  land.
- Say plainly what is untested.
- Nothing about a particular operator, their machines or their deployment belongs in
  this repository: no host names, addresses, device models, providers or names.
