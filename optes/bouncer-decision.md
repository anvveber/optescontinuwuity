# Bouncer — decision

Status: decided 2026-10-24. Replaces the bouncer in handbook section 6.

## Decision

No server-side bouncer. Replaced by:

1. **`record_device_activity = false`** (new config key, this fork). The server
   never stores when or from which IP a device was last active.
2. **The local relay's constant-rate tunnel** (level 3/4). At level 4 the
   tunnel from an always-on machine at home never goes down, so network
   observers see no online/offline at all. (The relay doesn't hold the user's
   login, so it can't sync on its own; the client app still decides when to
   make requests.)

## Why

The bouncer's job: make every account look like it syncs constantly, so the
pattern "which accounts are active, when" disappears. Who could see that
pattern, and does a bouncer stop them?

| Observer | Sees sync activity? | Bouncer helps? | What does help |
|---|---|---|---|
| Other users | No. Presence, typing, receipts are off; last-seen is shown only to the account's owner. | — | already handled |
| Network observer near the core | Phase 2: one constant-rate stream per front. Before that: Tor traffic. | No | front→core constant rate |
| Core process / VPS host, live | Yes: the real client's requests still arrive when the user is online. A bouncer adds fake requests *next to* them; it can't remove the real ones. | No | relay constant rate (L3/L4) makes the real requests uniform |
| Core disk (seizure, backup leak) | Yes: `last_seen_ts` / `last_seen_ip` per device, updated every 10 s while active. | No: the record is written by real requests. | `record_device_activity = false` |

So the bouncer protects against nobody that the relay plus the new config key
don't already cover. A bouncer *would* hide app-level activity from the core
process itself — but that observer is the host, which also sees the real
requests and all metadata (handbook limitation 3). It would also have needed the unsolved design: driving
`/sync` for a session without holding room keys or impersonating the device.

## Cost of the replacement

- Session lists in clients show no "last active" time for any device.
- Without level 4, an account's online pattern is still visible to the core's
  host in memory while it runs. That was true with a bouncer too.
