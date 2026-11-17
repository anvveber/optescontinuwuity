# optes — project state

Written 2026-11-09, updated 2026-11-10 (audit and its fixes: §4b), 2026-11-14 (kit
script: §4c; review of spaces, template tool and tests: §4d; first run over real Tor:
§4e) and 2026-11-15 (no build paths in the server binary: §4c; vanguards: §4b;
fuzzing on protocol v2: §3; clock over Tor: §4f; updates: §4g; backups: §4h;
relay after sleep and slow connects: §4i) and 2026-11-17 (upstream v26.9.4 and
dependency advisories: §4j). Read this first, then `optes/handbook.md` (design, threat model, §14 status).

`main` holds everything. This repository is a fork of continuwuity (https://github.com/continuwuity/continuwuity), based on tag
**v26.9.4** (`optes/UPSTREAM`; v26.9.1 until 2026-11-17, §4j). Upstream releases
are merged, not rebased: `main` is never rewritten. Every fork-specific file lives in `optes/`, except the server
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
| OS | Debian 13 (trixie) on servers. Relays: Debian (Ubuntu untested). Fedora dropped 2026-11-15: never run, not needed now. |
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
| `allow_room_retention`, `retention_purge_interval_secs` | MSC1763 `m.room.retention`: expired events are hidden at once and purged from the DB by a background job, then erased from the database files (flush, full compaction; no WAL archive). State events, forward extremities and the newest event are kept. | `src/service/retention/`, `retention-design.md` |
| `restrict_request_hosts`, `allowed_hosts`, `allowed_origins` | Browser guard: a website open in a user's browser can't use the server through the local relay (Host must be an IP, localhost or onion/i2p name; requests with an unlisted `Origin` are refused) | `src/router/request.rs` (`guard`), `src/router/layers.rs` |
| `allow_pushers` | Off: push registrations are answered with success and not stored | `src/api/client/push.rs` |
| `allow_openid_tokens` | Off: OpenID token requests are refused | `src/api/client/openid.rs` |
| `show_device_names_to_others` | Off: other users see a device's ID, not its name (client and OS); the owner still sees it | `src/api/client/keys.rs` |

Everything else:
- **Tunnel:** `optes/tunnel/`, with binaries `optes-relay`, `optes-gateway` and `optes-keys`.
  - Front-list renewal: the core re-signs weekly; the relay checks every 6 h.
  - A relay whose lists have all expired can still reach the gateway, but only to fetch a
    new list.
  - Fuzzing: 6 cargo-fuzz targets in `tunnel/fuzz/`. On protocol v2 (2026-11-15):
    15 minutes each, ~136M runs, no crashes (`tunnel/fuzz/README.md`). The
    gateway's replay check is in `src/replay.rs`, so a target can reach it.
- **Setup scripts:**
  - `setup-core.sh` (install, add-client, add-device, add-front, remove-front,
    renew-front-list, direct-ingress, allow-origin, remove-origin, lockdown, status,
    update, backup-key, backup, restore)
  - `setup-front.sh`
  - `optes-clock.sh`: the clock of core and fronts over Tor (§4f); installed by
    both setup scripts, so it must be next to them
  - `setup-relay.sh`
- **Tools:**
  - `tools/make-space.py`: builds a whole server (space, categories, channels) from a TOML file
  - `update-upstream.sh`: merges a new upstream tag on a branch and runs every test
  - `make-kit.sh`: builds the release binaries and packs the deploy kit (§4c)
- **Tests:** `optes/test/` (see §5)

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
  clock drift; Tor needs a roughly correct clock. Fixed 2026-11-15: `optes-clock.sh`
  (§4f).
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
  front after these changes; Fedora. The fuzz targets were only compiled then;
  they ran on 2026-11-15 (`tunnel/fuzz/README.md`).

Open after the audit (not fixed): low items L4–L7 (L7 partly done, §4c;
L5 done, §4g). L1 done 2026-11-15 except free disk blocks
(`retention-design.md`). Device names (12) are hidden since 2026-11-15
(`show_device_names_to_others`, `hardening.md`).
Finding 6 is accepted: the core uses Tor's built-in vanguards-lite only, not the
full add-on (handbook §4.6).

## 4c. Kit script (2026-11-14)

`optes/make-kit.sh` builds the release binaries and packs the kit. Layout and
options: the header of the script. What it does besides copying files:

- **No test build ships.** The tunnel is built without cargo features. Then the
  script asks the built `optes-keys` to sign a plain-TCP front and stops if it
  agrees. Checked both ways: a release build refuses, a `test-fronts` build signs.
- **Build paths.** Cargo's directory, the toolchain's directory and the checkout
  are mapped to `/cargo`, `/rust` and `/src`. The tunnel binaries then contain no
  path of the build machine.
  - The server's web pages are embedded by the crate `memory-serve`, which
    also wrote each file's absolute source path into the binary (10 paths).
    Since 2026-11-15, `src/web/build.rs` replaces them with paths relative to
    the crate when the files are embedded. If a new `memory-serve` writes
    the paths in another form, the build fails. If it marks embedded files
    differently, `build.rs` leaves the paths as they are without an error;
    then only the warning of `make-kit.sh` below shows them.
    The script still checks every binary and warns about any build path left.
    Tested: a kit build gives no warning, and the binaries contain neither the
    home directory nor the checkout path. A release server and a debug server
    both serve the files under `/_continuwuity/resources/` with the same bytes
    as the source; `run-hardening-e2e.sh` passes on the debug build (on, off).
  - Kit of 2026-11-15 (after this change and the replay check's move into the
    tunnel library) in two Debian 13 VMs over real Tor: core install from the
    README, relay straight to the core, `/versions` 200 through the relay
    (first after about 4 minutes, as in §4e), first account at level 2.
    Not run again: fronts, `lockdown`, levels 1 and 3, `make-space.py`.
    `update-upstream.sh --test-only` passed on the same commit.
- **No git remote, no commit in the server binary.** continuwuity's build script
  would record the URL of the remote `origin` and the commit. The kit build sets
  fixed values; the server reports its version as `<upstream version> (kit)`, e.g. `26.9.4 (kit)`. The kit's name
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

Open: level-1 padding on the wire, the test gaps in L10. `home-test.sh` (L11)
was removed on 2026-11-15.

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

## 4f. Clock over Tor (2026-11-15)

`optes-clock.sh` (rules in its header; installed by `setup-core.sh` and
`setup-front.sh` with `optes-clock.timer`, every 6 hours, first run 5–25 minutes
after boot). Sources: `/etc/optes-clock/sources`, four onions from
https://onion.debian.org/. `status` of both setup scripts shows the last result
and Tor's own clock complaints. What a deployer must know:

- **Forward up to 1 day and back up to 30 minutes happen by themselves.** A
  larger step back needs `optes-clock check --allow-back`. Reason (measured):
  an onion service numbers its descriptors by its clock, and Tor's directories
  refuse a lower number than one they hold. In a VM, with the clock 20 minutes
  ahead, Tor uploaded; after the step back every upload was refused until real
  time passed the time of that upload, then accepted again. The old
  descriptor keeps working, so only a Tor restart (or `remove-client`) in that
  time makes the onion unreachable. `status` prints until when.
- **2 or more days ahead, Tor itself fails** and nothing can answer. Set the
  clock on the provider's console. That is a large step back, so afterwards
  the host's onions can stay unreachable for a while (not measured how long).
  Measured with a plain Tor client started with a wrong clock: it bootstraps
  and reaches onions from 1 day behind to 1 day ahead; at 2 and 3 days ahead
  it does not finish bootstrapping. Not measured: more than 1 day behind, and
  whether a host's own onion stays reachable while its clock is that far off.
- **A refused check is not retried** (exit 3: a limit was hit, it waits for
  the operator). No usable answer or disagreeing sources (exit 1) is retried
  after 20 minutes.
- **A step moves the gateway's level-2 grid.** A relay connected before the step
  stays on the old grid until it reconnects. Tor also logs "clock just jumped"
  and rebuilds its circuits (seen after steps of 15 minutes and more); the
  relays in the test reconnected by themselves (except below).

How it was tested (Debian 13 VMs, real Tor, kit from `make-kit.sh` with this
change):
- Decision rule: `test/run-clock-decide-test.sh` (fixed samples: too few,
  slow, disagreeing, malformed, each limit).
- Sources: the four onions answered, mostly within 1–12 s once a circuit was
  warm, sometimes 30–120 s; the first request after a Tor start often takes
  longer. Hence a warm-up request per source, only answers under 6 s count,
  and all sources are asked at once.
- Core with a relay connected through a front: offsets 0 and 5 s left alone;
  5 min behind, 10 min ahead, 2 h and 1 day in both directions corrected to
  within 1 s; the relay got 200 after each. (This ran before the 30-minute
  limit existed; with it, 2 h and 1 day ahead need `--allow-back`.)
- 3 days ahead: no source answered (all at once: 3 minutes), clock untouched,
  `status` warned. Afterwards the core's onion stayed unreachable: all 30
  descriptor uploads in 5 minutes were refused. Most likely its uploads while
  the clock was 1 or 3 days ahead (earlier in the same run) carried higher
  numbers; which phase did it was not measured. This is the case the
  30-minute limit avoids. Not waited out.
- Front: 15 minutes behind corrected; 40 minutes ahead refused, then corrected
  with `--allow-back`; 10 minutes ahead corrected with the warning; timer
  active after a reboot and the check ran.
- The timer's unit, from the final kit on a fresh core: right after install
  only 1 of 4 sources answered fast enough (a fresh Tor is slow), exit 1, the
  unit set itself to retry; 40 minutes ahead: exit 3, unit failed, no retry; a
  made-up onion as the only source: exit 1, retry. Tor's consensus on disk
  started 1.5 hours before real time there. The consensus check never refused
  anything in these runs.
- Not tested: after `lockdown`; on a VPS; a VPS's own clock behaviour (drift,
  wrong time at boot).

## 4g. Updates (2026-11-15)

- **Programs:** from a new kit's `core/`, as root:
  `./setup-core.sh update --binary ./continuwuity --sha256 <hex from the kit README> --tunnel-bin-dir .`
  - In the session, before anything stops: checksum; `--version` / `--help` of
    the new programs; room for a copy of the database; packages that are
    missing (over Tor; installed packages are not upgraded here, since a tor
    upgrade would restart Tor in the middle); a copy of the current programs
    in `/var/lib/optes-update/previous`.
  - The rest runs as the systemd unit `optes-update`, so a dropped SSH session
    (the admin onion) cannot stop it halfway. Log:
    `/var/lib/optes-update/log`. It stops gateway and server, copies the
    database, installs continuwuity, optes-gateway, optes-keys, the script
    copy and optes-clock, starts, waits up to 5 minutes for the server to
    answer on its socket, then the gateway must stay up.
  - Any exit before the end rolls back: the old programs, and the database
    copy if the new server was started (a copy counts only once finished).
    Unit files and apt settings that the new kit wrote stay.
  - A new server version whose database migration takes more than 5 minutes
    is rolled back.
  - On success the copy is deleted. `rm` frees the disk blocks but does not
    erase them: the old copy, with events retention deletes later, stays in
    free disk space until it is overwritten (threat table, audit L1).
  - A leftover of an unfinished update (`in-progress`) blocks the next one.
  - Config: keys added to the server's config after the first kits are listed
    in `CONFIG_ADDED` in `setup-core.sh`; `update` adds those a config lacks,
    with the value `install` writes, before `[global.oauth]`. Nothing else in
    the config changes. A rollback restores the config too. (Since 2026-11-15:
    `show_device_names_to_others`.)
  - Not changed: keys, onions, Tor.
  - Every tunnel drops and reconnects (the gateway restarts).
- **Fronts** have no `update`: they hold no data. Replace a front with one
  built from the new kit.
- **OS security updates:** unattended-upgrades on core and fronts, with
  Debian's default origins (security updates and stable point releases), once
  a day by Debian's apt-daily timers, over apt's `tor+https` sources. No
  automatic reboot: `status` warns when one is needed. A tor update restarts
  Tor (tunnels drop and reconnect). It brings Python 3 onto the core
  (unattended-upgrades is written in it). Its logs are on disk
  (`/var/log/unattended-upgrades/`): package names and times, nothing about
  users.

How it was tested (Debian 13 VMs over real Tor, 2026-11-15; each time: a
relay straight to the core at level 1, first account, `make-space.py
example-server.toml` = 14 rooms; 17 fingerprinted files: onion keys and
names, client keys, gateway and list keys, device keys, front list,
registration token, config):
- Update from a kit without these changes (`0159f38`) to one with them, and
  update on a core with `unattended-upgrades` removed (missing package
  installed over Tor). Both: done in 8–18 s; all fingerprints unchanged; the
  relay reconnected; the same 14 rooms. The gateway's replay counter for the
  relay went up, as it should after a reconnect.
- Rollback, broken server (answers `--version`, writes a file into the
  database directory, exits): the SSH session was killed 40 s in; the unit
  went on, waited 5 minutes, rolled back: old binary, the file gone (database
  copy restored), fingerprints unchanged, relay back, 14 rooms.
- Rollback, broken gateway (answers `--help`, exits): rolled back after 14 s,
  same checks.
- A leftover `in-progress` file: `update` refuses.
- Security updates by hand on the core: 9 packages over Tor, among them the
  kernel; `status` asked for a reboot; after it the new kernel and all
  services. `apt-get install --reinstall tor`: torrc and the AppArmor override
  kept, Tor up, the relay reconnected (after about 5 minutes). On a front:
  install from the new kit, upgrades over Tor.
- 2026-11-15, with the device-name change: update from the kit of `da72e35`
  (old server binary, config without `show_device_names_to_others`) to the
  new kit. A real server version change: the new binary runs on the old
  database. The option was added to the config; before, another user saw the
  device name, after, the device ID; the owner still the name; every other
  fingerprinted file unchanged. A broken-server update on a config without
  the option: rolled back, config byte for byte as before.
- Not tested: a new server version with a database migration (no kit so far
  changes the schema); `update` after `lockdown`; on a VPS;
  a tor package that ships a changed torrc; the server's own signing key
  (its endpoint is off without federation; it lives in the database).
- Seen once: a relay needed 36 minutes after the core's install to reach its
  onion (the core's descriptor uploads were accepted; Tor network delay).

## 4h. Backups (2026-11-15)

Without a backup, a lost VPS loses everything: accounts, rooms, the server's
signing key, both onions, the gateway and front-list keys, the relays' device
keys.

- **Key, once, on the operator's own machine:** `age-keygen -o core-backup.key`
  (Debian/Ubuntu package `age`). Keep `core-backup.key` offline. Give the
  core the public key it prints: `setup-core.sh backup-key age1...`. The core
  can encrypt backups but never decrypt them.
- **Backup, from the operator's machine:**
  `torsocks ssh root@<admin>.onion 'optes-setup-core backup --out -' > core-$(date +%F).age`
  The server stops while the files are read (about a second for a small
  database); Tor and the gateway keep running. The encrypted file exists on
  the core only while it is sent, then it is deleted.
- **What is in it:** `/var/lib/continuwuity` (database, signing key),
  `/etc/continuwuity` (config, registration token), both onion directories
  (keys, client lists), `/etc/optes` (gateway and list keys, devices, fronts,
  front list), `/var/lib/optes-gateway` (replay counters), allowed origins and
  the direct-ingress setting. Not in it: programs and units (from the kit),
  firewall, apt and clearnet-SSH state (from the new machine).
- **Restore onto a new VPS:**
  1. `setup-core.sh install` from a kit with the **same** `--server-name`.
  2. From the operator's machine:
     `age -d -i core-backup.key core-….age | torsocks ssh root@<new admin>.onion 'cd <kit>/core && ./setup-core.sh restore -'`
  3. It checks the backup (only the expected paths, the same server name),
     then runs as the systemd unit `optes-restore`: stops gateway, server and
     Tor, puts the backup's files in place, starts everything, checks that
     the server answers. The new machine then has the **old** onions: the SSH
     session over the new admin onion drops; log in again over the old admin
     onion with the old admin key. Fronts and relays need no change.
  4. If the server does not come up, the new machine's own files go back.
  - The old core must be off: two cores with the same onion keys compete.
  - Replay counters are as old as the backup. A front that recorded a
    relay's handshake after the backup could replay it until that relay
    connects again (its counter is then higher). Low risk; relays reconnect
    often.
- **Not automatic.** There is no timer: the encrypted file must reach the
  operator anyway, so the operator runs the command (or a cron job on their
  machine).

How it was tested (Debian 13 VMs over real Tor, 2026-11-15): core A with a
relay straight to it and two users; `backup-key`; `backup --out -` over SSH
(1 s, 1.4 MB, no user name readable in it); decrypted with `age` on another
machine; core B installed with the same server name; `restore -` over SSH: 7
s; B then had A's ingress onion, both users (device names still hidden from
others), and all 16 fingerprinted key and config files equal to A's. A shut
down: the relay, which knew only A's onion, reached B after 3 minutes, and a
user logged in through it. A restore of a backup with a broken database:
rolled back after 5 minutes, B unchanged. A backup for another server name:
refused before anything changed. Found and fixed during the test:
`backup-key` changed the group of `/etc/optes`, which would have stopped the
gateway at its next start.

Not tested: backup and restore after `lockdown` (over the admin onions; the
commands are the same), on a VPS, a large database, restore with fronts in
use.

## 4i. Relay after sleep, and slow connects (2026-11-15)

Two problems from the VM runs: a relay that seemed stuck after its machine
slept, and connects that took 20–36 minutes.

What changed in the relay (no protocol change; gateway unchanged):
- **Dead tunnel noticed.** At levels 2 and 3 the gateway sends something every
  window or tick; a tunnel silent for 3 windows + 30 s (level 2) or 30 s
  (level 3) is closed and rebuilt. On every level, if the wall clock runs more
  than 30 s ahead of the monotonic clock (which stops during suspend), the
  machine slept or its clock was set forward: the tunnel is rebuilt at once.
  Level 1 has no silence limit (an idle level-1 tunnel carries nothing).
- **Quicker retry** after a tunnel that was up ends (2 s, then backing off to a
  minute as before for failed attempts).
- **Tor told to forget, after two failed attempts in a row.** `setup-relay.sh`
  gives Tor a control port (127.0.0.1:9051) with a random password, stored in
  the relay's config directory (mode 600); the relay uses it for one command,
  `SIGNAL NEWNYM`, which drops Tor's circuits and its cache of onion
  descriptors. Not after the first failure: if the cached descriptor was still
  right, clearing it makes the next try slower (seen in the VM).
- A session that is dropped now also stops its reader and writer
  (`session::run`; before, they kept running with the socket).
- `setup-relay.sh` is Debian only (Fedora branches removed), and a reinstall
  with the installed front list no longer fails.

What the VM runs showed about Tor (Debian 13 VMs on this machine, real Tor):
- A real suspend (S3) of the relay VM for 5 minutes with the old relay: it
  answered 7 s after waking. Tor logged "clock just jumped … assuming
  established circuits no longer work". Longer real suspends could not be
  tested: after the first suspend and wake, QEMU woke the VM again at once
  (`vm.py wake`, new, sends the wake-up).
- The long waits for a connection come from Tor's onion descriptors, not from
  the relay:
  - After the core's Tor restarted, the relay's Tor kept the old descriptor
    (revision 2148142102, the core published 4984990535) and a directory
    server still handed it out. Tor asks one directory server for the same
    onion at most every 15 minutes. `SIGNAL NEWNYM` emptied the cache; the
    relay connected a few minutes later.
  - Twice the directory servers for the core's onion did not answer at all,
    for 20–30 minutes, to the relay's Tor and to the core's own `HSFETCH`;
    meanwhile Debian's onions answered in about 50 s. Nothing on either side
    helps then. More fronts help: each has its own onion address and so its
    own directory servers.
- The sleep detector in the VM: the relay VM's clock set 90 s forward (time
  sync stopped): within 3 s the relay logged "this machine slept or its clock
  jumped; reconnecting".
  - With a first version that sent `NEWNYM` after every failure: no tunnel
    within 5 minutes (the cached descriptor was gone, the directory servers
    were slow).
  - With the final rule (`NEWNYM` only after two failures in a row): the
    tunnel was back 8 s after the jump, with no `NEWNYM`.

Tests: tunnel unit tests (28, among them: a silent peer ends a session after
the limit; a dropped session closes the connection; a wall-clock jump counts
as sleep, a steady clock not), e2e tunnel suites levels 1–3 and SOCKS tor/i2p.

Not shown: that `NEWNYM` shortens outages in general (once it was followed by
a connect a few minutes later; the network's own delays made a clean
comparison impossible); a long real sleep with the new relay; level 2 and 3
silence limits on real Tor.

## 4j. Upstream v26.9.4 and dependency advisories (2026-11-17)

- **Server moved from upstream v26.9.1 to v26.9.4** by a merge (no history
  rewrite). Upstream's six security fixes (SEC15, 31, 34, 36, 39, 45) are
  described from the code in `audit-2026-11-10.md`, section "Dependency
  advisories". Upstream changed no dependency, no config key and no
  database format; `CONFIG_ADDED` in `setup-core.sh` needs nothing new.
- One conflict, in `user_can_see_event`: the delayed-release and retention
  checks stay first; upstream's new rule follows (an event without computed
  state is visible only to members who are joined now; before, to anyone).
- **Dependencies:** `cargo audit` on both lockfiles. The tunnel has no
  advisory. The server had 23; 20 are fixed by patch updates of 13 crates
  (`Cargo.lock` only). The 3 left (rsa, crypto-bigint, webpage) are not
  reachable here. Every advisory with its answer: audit L6.
- **`update-upstream.sh` now merges instead of rebasing**, commits in UTC
  and adds the same `Co-Authored-By` trailer as the rest of the history.
  Its final `git merge --ff-only` now works, because the update branch
  starts from `main`.
- **New finding 18 (open):** with delayed release on, a room member sees a
  thread reply up to one window early in the thread root's stored summary
  (since v26.9.4 with sender and event ID). Audit finding 18, handbook
  §4.6, `delayed-release-spec.md` §10.

How it was tested:
- `update-upstream.sh --test-only` on the merge alone, and again after
  the crate updates: tunnel unit tests (26 + 2), the server's unit tests
  (`release::`, `sync::`: 11; `guard_tests::`: 2), all 16 e2e runs. All
  passed both times.
- The script's new merge path, in a throwaway clone with a shell in a
  non-UTC time zone (tests skipped): it stopped at the conflict with the
  hint; both commits got UTC dates and the trailer.
- `make-kit.sh`: no build path in the binaries; the server reports
  `26.9.4 (kit)`.
- Debian 13 VM: core installed from the kit of `e3fa54f` (v26.9.1); one
  front entry, one ingress client, one device; three users registered over
  the server's socket, device keys and one-time keys uploaded, an encrypted
  direct room with a thread; a plaintext message refused. Then
  `setup-core.sh update` to the new kit: 8 s. Afterwards: version 26.9.4;
  all services active; 18 fingerprinted key and config files unchanged
  (onion keys and names, client keys, gateway and list keys, device key,
  front list, registration token, config); everything the users can read
  (devices, key query from the other user, one-time key count, rooms,
  members, messages, the thread root) equal to before, except the `age`
  field; device names still hidden from the other user. No migration ran.
  In the old thread a new reply counted, and its redaction took it out of
  the summary again (upstream SEC39).
- The leak of finding 18: a local debug server, on v26.9.1 (kit binary) and
  on v26.9.4.

Not tested: the update with a relay connected over real Tor (done for
earlier updates, §4g); a rollback (no code change since §4g); `update`
after `lockdown`; on a VPS. A real database migration still has not
happened, because v26.9.4 has none.

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
- `run-clock-decide-test.sh` (the clock tool's decision rule, no network)
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
