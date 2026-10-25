# Private Matrix Server — Build Handbook

A build spec for a hardened, metadata-minimizing Matrix homeserver reachable over
Tor, I2P, and Nym, with no public entry point and a modified client. Written to be
handed to another Claude instance (or any engineer) to build from.

Status (2026-10-24): phases 1 and 2 built and tested locally; nothing yet run on
real VPSes or real Tor. Section 14 summarizes what changed from the original
design and where each piece lives. Sections 1–2 (threat model, limitations) are
unchanged and still govern.

---

## 0. How to use this document

- Read sections 1–2 first. They define what the system does and does **not** protect.
  If a later instruction seems to contradict them, 1–2 win.
- The build is phased (section 11). Build a phase, use it, then start the next.
  Do not build everything before running anything.
- Treat every version number, port, config key, and API name as **unverified**.
  Section 10 says what to confirm. Confirm it, then build.
- Working style (decided): the assistant writes the code for now; the owner aims
  to reach Rust fluency later and maintain it.

---

## 1. Goals and threat model

Goal: a Matrix homeserver for a small private group that

- has no dependency on owning a domain (identity can be an onion address),
- is reachable over Tor, I2P, and Nym, with room to add more transports,
- minimizes metadata exposure to network observers and to the server's own host,
- hides the server's location,
- lets each user choose how much timing protection they want, against their own ISP.

Adversaries, in rough order of how much work they take to defeat:

1. A network observer at one point (the user's ISP, local Wi-Fi). Sees that a
   privacy network is in use, plus timing and volume.
2. An observer with two vantage points — e.g. watches a user's line AND is in a
   room with them, or runs a federating server. Can correlate "burst on the line"
   with "event in the room."
3. Someone who has the server's address and tries to locate or fingerprint it.
4. The server's own VPS host (can read disk and RAM).
5. A global observer watching large parts of the network at once.

What is in scope: 1, 2, 3, and partial defense against 5. Item 4 is only partly
mitigable on rented hardware (see 2 and 4.1).

---

## 2. Accepted limitations — read before building

These are structural. No amount of code removes them. If any is unacceptable, the
design is wrong for the use case and that should be settled before building.

1. **The server name is permanent.** It is baked into every user ID (`@u:name`) and
   every event. Choosing an onion name as the server name means no domain anywhere,
   but federation then only works with servers that can reach Tor (almost none).
   Decide the identity once (section 12, decision A).
2. **Matrix leaks metadata by design.** End-to-end encryption covers message
   *content*. Room membership, who is in a room, event timestamps, and who talks to
   whom are stored on the server and copied to every server sharing a room. If
   hiding *that* from the server is the goal, Matrix is the wrong base — SimpleX is
   structurally better. Matrix is chosen here for rooms, bots, bridges, clients.
3. **The VPS host sees the core.** It can read the core's disk and memory. Content
   stays E2E-encrypted, but accounts, rooms, and metadata are exposed to the host.
   Hardware-encrypted VMs (AMD SEV-SNP) raise the bar; they don't eliminate it.
4. **Online/offline intersection.** When a user's device connects only while they're
   active, an observer who can see "this line was online" vs "this account was
   active" narrows it down over weeks. Only an always-on connection (level 4) defeats
   this. Everything below level 4 reduces it, doesn't remove it.
5. **Anonymity scales with the number of users, not with mechanism cleverness.** All
   batching/mixing hides a user among others active in the same window. With 5–20
   users, that crowd is small; at 3 a.m. with one person active, it's nonexistent.
6. **Custom code is attack surface.** A server fork, a client fork, and a custom
   tunnel protocol each add code that can be wrong. A homeserver that falls behind on
   room-state and federation fixes becomes a security problem itself. Keep forks
   minimal; prefer upstream config flags over patches; plan to track upstream.

---

## 3. Server choice

**Continuwuity.** Rationale:

- Actively maintained; calendar versioning (26.x). Fork pinned to **v26.9.1**
  (fork: `github.com/anvveber/optescontinuwuity`). The official community
  continuation of conduwuit (which continued conduit).
- Single Rust binary with an embedded database (RocksDB). No Postgres. Good for a
  one-script install.
- Synapse is heavy (Python + Postgres); Dendrite is effectively maintenance-only.

Source: `forgejo.ellis.link/continuwuation/continuwuity` (verify current canonical
URL — it has moved before, and a GitHub mirror exists).

Keep the fork a thin patch set on top of a pinned upstream release. Rebase on
upstream regularly.

---

## 4. Architecture

```
                ┌───────────── clients (modified) ─────────────┐
                │ encrypted tunnel to CORE, core key pinned     │
                └──────┬──────────────┬──────────────┬─────────┘
                       │ Tor          │ I2P          │ Nym
                ┌──────▼─────┐ ┌──────▼─────┐ ┌──────▼─────┐   ┌─────────────┐
   fronts       │ Tor front  │ │ I2P front  │ │ Nym front  │   │ Federation  │
   (disposable) │ onion      │ │ hidden-mode│ │ proxy srv  │   │ front (opt) │
                └──────┬─────┘ └──────┬─────┘ └──────┬─────┘   └──────┬──────┘
                       └──────────────┴──── Tor ─────┴────────────────┘
                              authorized onions only, constant rate
                                         │
                ┌────────────────────────▼─────────────────────────┐
   CORE         │ gateway VM (Tor only) ── service VM (no network)  │
                │ Continuwuity fork + bouncer + release scheduler   │
                └───────────────────────────────────────────────────┘
```

Principle: **fronts are disposable and learn almost nothing; the core has no public
entry point; clients encrypt all the way to the core.**

### 4.1 Core (one VPS)

- **No public listener.** Reachable only via onion services that require client
  authorization. Only the fronts hold the authorization keys. Without a key an
  attacker cannot connect at all, so connection-dependent attacks (guard discovery,
  probing) have no target.
- **Isolation (built):** continuwuity and optes-gateway each run in their own
  network namespace containing only loopback (systemd `PrivateNetwork=yes`).
  They talk to Tor and to each other over unix sockets only, so neither can
  open any network connection. Tor runs on the host; nftables lets only the
  Tor user reach the internet. Upgrade path: two VPSes at one provider with a
  private-only network for the service machine (no public IP) — same configs.
- **Keys live only here:** the Matrix signing key; the key that signs the front list
  (section 4.4). Back them up encrypted and offline.
- **All outbound over Tor, fail closed:** federation, media fetch, time sync, package
  updates. If Tor is down, nothing leaves. No clearnet DNS, ever.
- **Minimize stored metadata.** Media deleted after 30 days. No per-device
  last-seen time or IP (`record_device_activity = false`). Logs at warn level.

Whonix-style gateway/workstation split is the reference model. NixOS or a single
reproducible script so the core rebuilds from encrypted backup + offline keys.

### 4.2 Fronts (one VPS each)

- One per network: Tor, I2P (hidden mode), Nym. Adding a network = adding a front
  type. The core does not change.
- **Blind relays.** The client opens an encrypted tunnel (TLS or Noise) *to the core*,
  with the core's key pinned in the app. The front forwards ciphertext it cannot read:
  no access tokens, no API calls, no message sizes. Padding is inside the tunnel, so
  the front can't distinguish padding from payload.
- **No user data, no state.** Rebuilt from the setup script; burn and replace freely.
- **Front→core link (decided):** one Tor circuit per relay connection, no
  multiplexing at the front. A network observer at the core can't separate
  circuits inside the Tor connection anyway, and the host terminates the tunnels;
  so the front stays a plain socat pipe with no custom code
  (`phase2-protocol-spec.md` §6).

### 4.3 Client-to-core tunnel

**Built** — see `phase2-protocol-spec.md` for the exact protocol. Summary: Noise
IK, fixed 2048-byte frames for every byte including the handshake, levels 1–3 on
the wire, signed front list. The core end is a separate process,
`optes-gateway`, not a patch to continuwuity.

- Carried *inside* whatever transport the front speaks. The front terminates the
  transport (onion / I2P / Nym) and forwards the inner tunnel bytes to the core.
- Inner tunnel = an authenticated encrypted channel from client to core. Noise
  (e.g. Noise_IK) is a good fit: core's static key pinned in the client; forward
  secrecy; small. TLS 1.3 with a pinned cert is the fallback if Noise tooling is
  heavier than warranted.
- The HTTP(S) Matrix traffic rides inside this tunnel. So the front and the transport
  network both see only opaque tunnel frames.

### 4.4 Front list (how clients find changing fronts)

- The core signs a short list: `[{network, address}, …]`, a version number, a
  timestamp.
- Clients fetch it through any working front and verify the signature against the
  pinned core key.
- A compromised front cannot redirect users (bad list fails signature check).
  Retiring a burned front = drop it from the list, re-sign. Users do nothing.
- Bootstrap: ship the client with an initial list (or at least one front address +
  the pinned core key). Chicken-and-egg: the first fetch must go through a front
  that's in the shipped list.

### 4.5 Federation front (optional)

- Separate front, own onion address = the server name, if federation is wanted.
- That onion key is the server's permanent identity → backed up offline. Any VPS
  holding it can serve the address, so the front is still replaceable.
- Closed mode: this front does not exist; `allow_federation` off.

### 4.6 What each compromise yields

| Compromised                | Attacker learns                                              |
|----------------------------|-------------------------------------------------------------|
| A front                    | someone connected, when, how much. No accounts, no content. |
| Observer at the core       | constant-rate Tor traffic.                                  |
| Observer at a user         | that user's level 0–4 traffic pattern.                      |
| Core's VPS host            | accounts, rooms, metadata. Content stays E2E-encrypted.     |

Last row is the residual weak point (limitation 3).

---

## 5. Privacy levels (client-selectable, except where noted)

Each level includes the ones below it.

- **0 — Off.** Normal Matrix behavior.
- **1 — Remove obvious signals.** ~free.
  - Typing notices, read receipts, automatic media downloads off.
  - Pad every request/response up to fixed buckets (e.g. 4/16/64 KB).
  - Media up/downloaded in fixed-size chunks.
- **2 — Break action↔traffic link.** costs delay.
  - *Fixed-interval sync* (client): poll on a fixed schedule (e.g. ~30 s ± jitter)
    instead of the server answering `/sync` the instant an event arrives. Download
    timing stops depending on when messages were posted.
  - *Delayed release* (**server**, affects everyone on that server): hold outgoing
    events and publish on a fixed tick (e.g. 1–5 min). The event's visible origin
    timestamp no longer matches the sender's upload. Window is a server setting.
- **3 — Constant-rate link.** costs bandwidth + battery.
  - Constant stream of fixed-size frames, both directions, while the app is open.
    Real data replaces dummy frames; dummies fill the gaps. Every user's stream looks
    identical to the ISP.
  - Server-side bouncer keeps every account syncing constantly regardless.
- **4 — Always on.** hides online/offline (the only level that does).
  - Run the constant-rate link from an always-on machine at home, not the phone.
  - Phone reaches that machine over WireGuard (owner already runs a WG server at home).
  - Home ISP sees a constant stream 24/7 → online times vanish. Carrier sees only WG
    to home; pad that tunnel too.

Cost table:

| Level | Delay            | Bandwidth                         | Battery        |
|-------|------------------|-----------------------------------|----------------|
| 1     | none             | small padding overhead            | none           |
| 2     | 30 s–few min     | small                             | ≤ normal       |
| 3     | adds latency     | ~10 KB/s per direction while open | high           |
| 4     | same as 3        | ~26 GB/mo per direction (home line)| normal if phone unpadded |

Recommendation: level 2 is the best value — it kills the "message posted → burst on
your line" correlation, which is the realistic 2-vantage-point attack. 3 and 4 are
for users facing a near-global observer.

---

## 6. Continuwuity fork — server changes

Keep minimal. Prefer config flags; upstream them where possible.

Config-only (no patch) if current continuwuity supports them — **verify exact keys
against the current example config, names below are intent not gospel**:

- `trusted_servers = []` (default fetches signing keys from matrix.org, which tells
  matrix.org who you federate with).
- URL previews off (server must not fetch posted links).
- Presence off (incoming and outgoing).
- Device-name federation off.
- Registration by token only; no email/phone capture.
- Room directory not published; directory not shared over federation.
- Logging at warn; no IP logging.
- Media retention limit.
- Listener bound to localhost / the gateway only; never a public interface.
- No TURN server (voice/video calls leak IP). Add self-hosted TURN later if calls are
  wanted, and document that calls break the model.

Patches (only if not achievable by config):

1. **Delayed release scheduler** — hold outgoing events, publish on a fixed tick.
   Server-wide; window configurable. This is the level-2 server half. Biggest patch;
   design it so it can be a clean upstream feature (an "egress delay" option).
2. **Strip per-device last-seen IP storage** — don't persist device IP/last-seen.
3. **Compile-out URL preview** code if config-disabling it still leaves fetch paths.
4. **Onion-peer TLS handling** (only if federating onion↔onion): accept/skip cert
   validation for `.onion` peers, since onion already authenticates the endpoint and
   most onions can't get a CA cert (HARICA sells onion certs; still awkward).
5. **Fail-closed outbound proxy** enforced in code, not just config, so federation and
   media fetch cannot bypass Tor.

**Bouncer** (can be a separate process next to the core rather than a patch):

- Keeps every account's `/sync` loop running at a fixed cadence whether or not the
  user's device is connected. To the homeserver every account looks equally, always
  connected → removes the "which accounts are actively syncing, when" leak.
- Runs on the core (same trust domain). Clients still do their own E2E encryption, so
  the bouncer passes only ciphertext.
- Interaction with E2E/device keys needs care: the bouncer must not need to be a
  logged-in device holding room keys. Prefer having it drive sync for already-logged-in
  client sessions, not impersonate them. **Open design problem — resolve in phase 1.**

---

## 7. Client — superseded by the local relay

**Decision (2026-10-24):** no client fork. All transport and wire-level privacy
lives in `optes-relay`, a local program; any Matrix client (Cinny, FluffyChat,
Element) points at `http://127.0.0.1:8008`. Level 4 = the same relay on an
always-on machine at home, phone over WireGuard. The original plan follows for
reference.

### 7.0 Original plan (not pursued)

Base: **matrix-rust-sdk** (the library under Element X). Two options:

- Fork **Element X** (Android) → usable app soonest, more code to audit.
- Minimal app directly on matrix-rust-sdk → less to audit, more UI work.

Decide per platform. Owner's phone is a Pixel 9 Pro on GrapheneOS; desktop is Fedora.

Add a **transport module** with the inner tunnel + front-list verification:

- **Tor:** embed Arti (Rust Tor). No Orbot dependency.
- **I2P:** talk to an i2pd router via SAM (I2P's app bridge). Needs a router reachable
  from the device; owner runs i2pd on `tvhub` already.
- **Nym:** Nym Rust SDK **Stream module** (not the deprecated TcpProxy — see 10).
- Pin the core's static key; fetch + verify the signed front list (4.4).

Add the **privacy levels** (section 5): fixed-interval sync, padding/bucketing,
receipts/typing/auto-download off, constant-rate mode, optional home-relay for L4.

---

## 8. Setup scripts (spec)

Two scripts. Idempotent. Debian or Fedora. Each prints exactly what the operator
needs next (addresses, keys to back up).

### 8.1 `setup-front.sh`

`--type tor|i2p|nym` (extensible), `--core <core-onion-addr>`, `--core-key <pinned>`.

Common:
- Dedicated system user; systemd sandboxing (`ProtectSystem`, `PrivateTmp`,
  `NoNewPrivileges`, syscall filter); default-deny firewall.
- Install + configure the forwarder that terminates the chosen transport and relays
  inner-tunnel bytes to the core over Tor (fail closed).

Per type:
- **tor:** v3 onion service with client authorization → prints onion address + the
  client-auth keypair. Backs up the onion key.
- **i2p:** i2pd in hidden/firewalled mode (does not publish IP, does not relay) →
  server tunnel; prints `.b32.i2p`.
- **nym:** Nym proxy server (Stream-module based, see 10) → prints Nym address;
  persists its identity dir.

Output: the front's public address (to add to the core's signed front list) and any
key to back up.

### 8.2 `setup-core.sh`

`--mode onion | onion+domain | i2p | closed` (closed = no federation).

- gateway VM/namespace: Tor only, fail closed.
- service VM/namespace: no network except via gateway.
- Install the Continuwuity fork + bouncer + release-scheduler config.
- Generate: Matrix signing key, front-list signing key. Print fingerprints; instruct
  to back up encrypted + offline.
- Create onion services (client-auth required) for admin + front ingress.
- Apply all section-6 privacy settings.
- Time sync over Tor; updates over Tor; no clearnet DNS.
- Subcommands: `add-front`, `remove-front` (edit + re-sign the front list),
  `update`, `backup`, `restore`, `uninstall`.
- Print: core ingress onion(s), the pinned core key for clients, the registration
  token, initial front list.

Media: owner has Swiss addresses/homes to receive hardware and can host at home;
the core is assumed on a VPS. Backups encrypted with age, pushed over Tor to
separate storage.

---

## 9. Custom protocol details to specify

Write these down precisely before coding; they're the parts most likely to leak.

1. **Inner tunnel.** Noise pattern (suggest Noise_IK), rekey policy, framing.
2. **Padding / bucketing.** Exact bucket sizes; how requests and responses map to
   buckets; media chunk size. Must not vary with content.
3. **Constant-rate scheduler (L3).** Frame size, rate, how real data preempts dummies,
   behavior on backpressure. Rate must be identical for all users and not adapt.
4. **Fixed-interval sync (L2 client).** Interval, jitter distribution. Jitter must not
   encode activity.
5. **Delayed release (L2 server).** Tick length, how origin timestamps are set,
   ordering guarantees within a tick.
6. **Front list.** Serialization, signature scheme (tie to the pinned core key),
   version/rollback protection, fetch + bootstrap flow.
7. **Multiplexing front→core.** One constant-rate Tor connection carrying many user
   tunnels; per-user framing inside it.

---

## 10. Verify before writing code

Do not build on any of these unverified:

- **Continuwuity:** current canonical repo URL; latest stable version to pin; exact
  config keys for every setting in section 6; default listen port (assumed 6167 —
  confirm). Whether delayed-release/egress-delay or per-device-IP suppression already
  exist upstream (don't patch what exists).
- **Nym:** the TcpProxy module is **deprecated/unmaintained**; use the **Stream
  module**. Nym has also warned of a **breaking SDK upgrade + protocol change** — check
  status, it may force a rewrite. Confirm whether raw mixnet access needs **paid
  bandwidth credentials** (reports of ~$2/mo) and whether that's acceptable given one
  company gates access. Check if the **improved incoming cover-traffic** scheme (from
  Nym's own fingerprinting paper) has shipped; current cover traffic is weak on the
  receive side.
- **Nym on Android/GrapheneOS:** no ready-made app for this; likely the client runs
  in Termux, or the transport is built into the forked client. Verify feasibility.
- **I2P:** i2pd SAM API specifics; exact hidden/firewalled-mode config so the router
  neither publishes its IP nor relays.
- **Tor:** Arti maturity for embedding in the client (onion-service *client* support
  in particular). Confirm v3 client-authorization config for the fronts.
- **matrix-rust-sdk / Element X:** current transport-injection points; whether a
  custom HTTP transport can be slotted under the SDK cleanly.
- **Bouncer vs E2E:** confirm a design where the bouncer drives sync without holding
  room keys or impersonating devices.
- **AMD SEV-SNP VPS:** which providers offer it, paid anonymously, if limitation 3
  matters.

---

## 11. Build phases

Build, use, then proceed. Phase 1 alone beats almost any existing Matrix deployment.

1. **Core + Tor front.** Continuwuity fork with section-6 privacy settings, the
   bouncer, the release scheduler. `setup-core.sh` + `setup-front.sh --type tor`.
   Reach it with an unmodified client over Tor to validate.
2. **I2P + Nym fronts.** The inner encrypted client↔core tunnel and the signed front
   list. `setup-front.sh --type i2p|nym`.
3. **Client fork.** Transport module (Arti / SAM / Nym Stream) + privacy levels 0–4.

Testing limits: a cloud build environment can't exercise real Tor/I2P/Nym
federation (restricted outbound). The owner runs those tests on their own
machines/VPSes.

---

## 12. Decisions (originally open)

Resolved 2026-10-24:
- **A. Identity:** onion address as server name (made with mkp224o, key offline;
  not served while federation is off).
- **B. Federation:** off.
- **C. Code:** assistant writes it for now.
- **D. Nym:** deferred; Tor first, I2P next.
- **E. Hosting:** still open — provider and payment method for the core VPS
  (and fronts) not chosen yet.

Original wording:

- **A. Identity.** Onion-only server name (no domain, essentially a private island /
  Tor-only federation) **or** domain name with onion+I2P+Nym as access paths
  (federates publicly, depends on DNS + domain)? Permanent choice — section 2.1.
- **B. Federation.** On (needs the federation front + a reachable server name) or
  closed? 
- **C. Who writes the code** this time (owner writes + assistant reviews, as on the
  TypeScript repo, vs assistant builds)? Rust-heavy, new project.
- **D. Nym.** Acceptable to depend on paid credentials + one company gating access?
  If not, ship Tor + I2P only and leave Nym as a stub front type.
- **E. Hosting.** All VPS (assumed). Against a long-term observer of the owner's home
  line, uptime correlation is hard to beat for any home-hosted box — so the core
  stays on a VPS. Confirm providers + anonymous payment.

---

## 13. One honest summary line

This buys strong metadata resistance against network observers and makes the server
hard to locate, at real cost in latency, bandwidth, battery, and maintenance. It does
**not** hide metadata from the core itself or from the core's VPS host, and it does
not make a 10-user server anonymous at 3 a.m. Build it in phases and keep the forks
thin.

---

## 14. Status and map (2026-10-24)

| Piece | State | Where |
|---|---|---|
| Verification of section 10 | done for phases 1–2 | `verification-notes.md` |
| Core setup (Tor, sandboxing, firewall, config) | written, not run on a VPS | `setup-core.sh` |
| Delayed release patch | built, e2e tested | `delayed-release-spec.md`, `src/service/release/` |
| `record_device_activity` (replaces the bouncer) | built, e2e tested | `bouncer-decision.md` |
| Bouncer | **dropped** (covered by the above + relay) | `bouncer-decision.md` |
| Tunnel: relay, gateway, keys tool | built, e2e tested at levels 1–3 (no Tor) | `tunnel/`, `phase2-protocol-spec.md` |
| Tor front setup | written; its socat path tested via a SOCKS stand-in | `setup-front.sh --type tor` |
| Relay setup (user machine) | written, not run | `setup-relay.sh` |
| Space auto-join, creation limits, template tool | built, e2e tested | `spaces.md` |
| `require_encryption` (plaintext messages refused, rooms encrypted by default) | built, e2e tested | `hardening.md` |
| `hide_server_identity` | built, e2e tested | `hardening.md` |
| Front-list renewal (weekly re-sign; relay refresh) | built, e2e tested | `phase2-protocol-spec.md` §5 |
| Per-channel retention (MSC1763; whole events deleted) | built, e2e tested (incl. restart on purged DB) | `retention-design.md` |
| Fuzzing of tunnel parsers (5 targets) | ~19M runs, no crashes (2026-10-25) | `tunnel/fuzz/` |
| I2P front | written; i2pd config and address computation checked against i2pd 2.49; relay I2P path tested via a SOCKS stand-in | `setup-front.sh --type i2p` |
| Nym | deferred | — |
| Client fork | not needed (relay) | section 7 |

Changes from the original design worth knowing:
- Continuwuity's default port is 8008, not 6167; the core uses unix sockets.
- Continuwuity phones home to continuwuity.org by default
  (`allow_announcements_check`); turned off.
- On first start, continuwuity ignores the configured registration token; the
  first account uses a one-time token printed in the log.
- The server has no network, so push notifications can't reach phones; clients
  must keep syncing in the background.
- Patches dropped as unnecessary: device-IP stripping (now a config key), URL
  preview removal (off by default), onion-peer TLS (no federation), fail-closed
  proxy in code (network isolation does it).

Tests (run from the repo root after `cargo build -p conduwuit` and
`cargo build` in `optes/tunnel`):
- `optes/test/run-e2e.sh target/debug/conduwuit 15` — delayed release
- `optes/test/run-tunnel-e2e.sh target/debug/conduwuit optes/tunnel/target/debug <1|2|3>`
- `optes/test/run-socks-path-e2e.sh target/debug/conduwuit optes/tunnel/target/debug <tor|i2p>`
