# Phase 2 protocol — relay ↔ core tunnel

Status: design v1, 2026-10-24. Covers handbook section 9 items 1–4, 6, 7.
Delayed release (item 5) is in `delayed-release-spec.md`.

## 1. Pieces

```
 client app ──HTTP──▶ RELAY ══ tunnel ══▶ FRONT ──Tor──▶ GATEWAY ──unix──▶ continuwuity
 (Cinny,              (user's            (blind byte    (on the core,
  FluffyChat…)         device or home)    pipe)          no network)
```

- **Relay** (client side). Listens on `127.0.0.1`. Every TCP connection from
  the client app becomes a *stream* inside one encrypted tunnel to the gateway.
  Speaks to the front through a SOCKS5 proxy (C tor for v1; i2pd's SOCKS proxy
  for `.i2p` fronts later; embedded Arti later).
- **Front**. A Tor onion service whose port forwards raw bytes to the core's
  ingress onion. Needs no custom code (Tor + socat). Sees only fixed-size
  encrypted frames.
- **Gateway** (core side). Terminates the tunnel. Each stream becomes a
  connection to continuwuity's unix socket. Runs with no network
  (`PrivateNetwork=yes`), like continuwuity: Tor reaches it by unix socket.

The relay and gateway don't parse HTTP. They carry bytes. Any Matrix client
works.

## 2. Keys

| Key | Type | Where | Purpose |
|---|---|---|---|
| Gateway static key | X25519 | core | Noise responder identity. Its public half is in the signed front list. |
| Front-list key | Ed25519 | core (root only), backup offline | Signs the front list. **Pinned in every relay.** |
| Device key | X25519 | each relay | Noise initiator identity. Gateway accepts only listed device keys. |
| Front onion client-auth keys | X25519 | relay's tor, fronts | Optional. Hides fronts from scanners. |

Pinning only the front-list key lets the gateway key rotate: publish a new
signed list containing the new key.

## 3. Wire format

**Every byte on the wire after the TCP connect is part of a 2048-byte frame.**
No length prefixes, no variable sizes. Handshake messages are frames too.

- `FRAME = 2048` bytes on the wire.
- Transport frames: Noise ciphertext of a 2032-byte plaintext *cell* + 16-byte tag.

### 3.1 Handshake

Noise pattern `Noise_IK_25519_ChaChaPoly_BLAKE2s`, prologue
`b"optes-tunnel-v1"`.

- **Message 1** (relay → gateway): IK message 1 (ephemeral 32 + encrypted
  static 48 + encrypted payload). Payload = `HELLO` (below), zero-padded so
  the whole message is exactly 2048 bytes.
- The gateway reads exactly 2048 bytes, runs the handshake, checks the device
  key against its allowlist.
  - Unknown key or decryption failure: **send nothing**, wait a random
    5–30 s, close. Same for any malformed input. A prober learns nothing
    except "accepts TCP".
- **Message 2** (gateway → relay): IK message 2, payload = `WELCOME`, padded to
  2048 bytes.

`HELLO` (CBOR-free, fixed layout):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | protocol version = 1 |
| 1 | 1 | level: 1, 2 or 3 (4 is deployment, not protocol) |
| 2 | 8 | front-list version the relay holds (u64 BE) |

`WELCOME`:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | protocol version = 1 |
| 1 | 1 | level accepted (gateway may raise, never lower) |
| 2 | 4 | constant-rate tick, ms (u32 BE) — server-wide |
| 6 | 4 | window length, s (u32 BE) — server-wide, = release interval |
| 10 | 8 | current front-list version (u64 BE) |

### 3.2 Cells

Plaintext of every transport frame, exactly 2032 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type |
| 1 | 4 | stream id (u32 BE) |
| 5 | 2 | payload length n (u16 BE), ≤ 2025 |
| 7 | n | payload |
| 7+n | rest | zero |

Types:

| Type | Name | Meaning |
|---|---|---|
| 0 | PAD | ignored |
| 1 | OPEN | open stream id (relay → gateway only). Relay uses odd ids. |
| 2 | DATA | payload bytes for stream |
| 3 | CLOSE | stream finished (either side); no more DATA for it |
| 4 | LIST_GET | relay asks for the current signed front list |
| 5 | LIST_PART | gateway sends the list, split across cells, stream id = part index; last part has payload length 0 |

Rekey: both sides call Noise `rekey` on a direction after every 2^16 frames
sent in it. Deterministic, no message needed.

## 4. Privacy levels on the wire

Server-wide constants (from `WELCOME`): `TICK` = 200 ms, `WINDOW` = release
interval (15 s now).

| Level | Rule | Line observer sees | Cost |
|---|---|---|---|
| 1 | Send when there's data. After the send queue drains, add PAD frames until the burst is a **power of two, minimum 4** frames. | When the user is active; burst sizes rounded up. | ~0 |
| 2 | Send only at window boundaries (wall-clock multiples of `WINDOW`, +1 s for gateway → relay so released data is included). **Every window**, each side sends a burst padded as in level 1, even when idle. | A fixed 8 KB+ burst every window, same for every user. Burst size rounded. | delay up to 2×`WINDOW`; idle = 8 KB per 15 s ≈ 0.55 KB/s → ~1.4 GB/month per direction |
| 3 | Exactly one frame every `TICK`, both directions, while connected. Data replaces PAD. Never bursts, never skips ahead to catch up. | A flat 10 KB/s stream. | 10 KB/s each way (~26 GB/month if 24/7); throughput capped at 10 KB/s |

Level 2 and the server's delayed release use the same tick grid, so every
level-2 user's bursts happen at the same moments: the crowd is everyone
connected, not just everyone active.

Level 4 = level 3 from an always-on relay at home; the phone reaches it over
WireGuard. The tunnel then never goes up or down with the user's activity.

Throughput note (level 3): a 1 MB image takes ~100 s. Accepted (handbook
section 5 cost table).

## 5. Front list

```json
{
  "version": 7,
  "issued_at": 1792800000,
  "expires_at": 1795392000,
  "gateway_key": "<base64 X25519 public key>",
  "fronts": [
    { "transport": "tor", "address": "abc…xyz.onion:443" }
  ]
}
```

Signed file = the JSON bytes, then a line `-----SIG-----`, then the base64
Ed25519 signature over the JSON bytes exactly as stored.

Relay rules:
- Verify the signature against the pinned front-list key. Reject on failure.
- Reject if `version` < highest version it has accepted before (rollback).
- Lists expire (default 30 days). An expired list is still used to *reach* the
  gateway (its identity is checked by the Noise handshake anyway), and the
  relay then fetches a fresh one. The core re-signs weekly (timer).
- While connected, the relay asks for a newer list every 6 h, so tunnels that
  stay up for weeks (level 4) don't fall behind.
- After each connect, if the gateway's version (from `WELCOME`) is higher,
  fetch it with `LIST_GET`, verify, store.
- Bootstrap: the relay is configured with an initial signed list file and the
  pinned key, both produced by `setup-core.sh`.

A compromised front can't redirect a relay: a forged list fails the
signature; a replayed old list fails the version check.

## 6. Front → core link

v1: **one Tor circuit per relay connection**. No multiplexing at the front.

Why no front multiplexing: a network observer at the core sees one TLS
connection to its Tor guard carrying all circuits; it can't separate them.
Separating circuits requires being the guard relay or the host. Against the
host, multiplexing doesn't help (it terminates the tunnels). So the front stays
a dumb pipe with no code, which is worth more than the small gain.

What this leaves visible to the core's guard relay or host: how many relays are
connected and when they connect. Level 4 removes the "when".

## 7. Not covered

- The core host still sees everything after the gateway (limitation 3).
- I2P fronts: same protocol. The relay reaches them through i2pd's SOCKS proxy
  (`--socks-i2p`); front list transport `i2p`, address `<b32>.b32.i2p:443`.
  The front runs i2pd in hidden mode with a server tunnel to a local socat,
  which forwards over Tor to the core (`setup-front.sh --type i2p`).
- Embedded Arti in the relay: later; v1 uses an external SOCKS proxy so it
  works with C tor's onion client auth today.
