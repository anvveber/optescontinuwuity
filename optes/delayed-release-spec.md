# Delayed release — spec

Status: design. Base: upstream v26.9.1. Implements handbook section 5, level 2
(server half), and section 9 item 5.

## 1. What it defends against

An observer with two vantage points: one on the sender's line (or the sender's
room), one on the receivers' lines (or inside the room). Without this feature:

- the event's `origin_server_ts` equals the moment the sender uploaded it;
- every receiver's `/sync` returns the event seconds after the upload.

Both let the observer match "upload on line A" to "event in room".

## 2. Mechanism

Two parts. Both are off when `release_interval_secs = 0`.

### 2.1 Release watermark

- New config key `release_interval_secs` (u64). `0` = disabled. Suggested: `60`.
- Ticks fall on wall-clock multiples of the interval since the Unix epoch
  (interval 60 → every minute at :00).
- New service holds the **release watermark** `W` (u64) in a
  `tokio::sync::watch` channel.
- A ticker task wakes at each tick and sets `W = globals.current_count()`.
- On startup `W = globals.current_count()` (anything pending is released).
- Disabled: `W` always reads as `globals.current_count()`. No behaviour change.

Every count-ordered item (PDUs, to-device messages, account data, key changes)
gets its count from the same global counter. So bounding client reads by `W`
gates all of them at once, and they stay consistent: an encrypted message and
the to-device key share for it are released in the same tick.

### 2.2 Coarse timestamps

- Local events get `origin_server_ts = next tick boundary` at creation:
  `ceil(now / I) * I` in ms.
- Site: `create.rs` default branch of `origin_server_ts` (it already accepts an
  explicit timestamp for appservices; keep that path unchanged).
- Result: every event released at tick T carries timestamp T. Order inside a
  tick comes from timeline position, not timestamps.

## 3. Client-facing gate

Rule: **a client never sees anything with count > W.** Server-internal reads
(state resolution, auth checks, admin) are not gated.

| Path | Site (v26.9.1) | Change |
|---|---|---|
| `/sync` v3 | `src/api/client/sync/v3/mod.rs:233` | `current_count = min(globals.current_count(), W)` |
| `/sync` long-poll wake (v3 + v5) | `src/service/sync/mod.rs` | Per-user wake-ups are held while enabled and fired right after each tick. Handlers unchanged. |
| sliding sync v5 | `src/api/client/sync/v5.rs:211` | `next_batch = min(…, W).max(1)` |
| `/messages` | `src/api/client/message.rs` | skip PDUs with count > W |
| `/context` | `src/api/client/context.rs` | 404 if target count > W; skip later PDUs |
| `/event/{id}` | `src/api/client/room/event.rs` | 404 if count > W |
| `/relations`, threads | `src/api/client/relations.rs` (+ threads) | skip count > W |
| `/search` | `src/api/client/search.rs` | skip count > W |
| `/state`, members | `src/api/client/state.rs`, membership | serve state as of W where cheap; see 6.3 |
| `/initialSync` (room) | `src/api/client/room/initial_sync.rs` | bound by W |
| `/report` | `src/api/client/report.rs` | 404 if count > W |

Not gated: the sender's `PUT /send` response. It returns the event ID at once,
as it does now.

Exact list to be re-checked by grepping client handlers at implementation time.

## 4. What it hides and what it doesn't

Hides:
- the exact send time inside a window of `I`, from room members and from
  anyone watching receivers' lines;
- which of the senders in a tick sent first, from timestamps.

Does not hide:
- the sender's own upload time from the sender's ISP (needs client-side
  constant rate, level 3);
- how many events were released in a tick;
- order inside a tick (timeline position is visible);
- anything when only one person is active (handbook limitation 5).

## 5. Costs

- Every message appears up to `I` seconds late for everyone, including the
  sender's own other devices.
- A room the sender just created or joined doesn't show in their client until
  the next tick.
- Timestamps shown in clients are rounded to the tick.

## 6. Edge cases

1. **Event created just before a boundary, appended just after.** It gets
   timestamp T but is released at T + I. Timestamp is up to `I` earlier than
   release. Acceptable; it doesn't leak the real send time.
2. **Edits / redactions of unreleased events.** Allowed. Released in the same
   tick or later; target is never shown after its redaction.
3. **State endpoints.** Serving historical state "as of W" may be expensive.
   Fallback: refuse `/state` and members for rooms with an unreleased state
   event (return the pre-tick result from a short cache). Decide at
   implementation.
4. **Restart.** Releases everything immediately. Acceptable.
5. **Clients that order by timestamp.** Unverified for Cinny and FluffyChat;
   most order by timeline position. Test.
6. **Push notifications.** Not affected; the core has no network, so push
   gateways are unreachable anyway. No push on this server.

## 7. Upstreamability

- Off by default, one config key, no schema change.
- Disabled path must be a no-op: `W` reads through to `current_count`.

## 8. Tests

- Unit: tick boundary math (`ceil`, epoch alignment, `I = 0`).
- Integration (two local users):
  - A sends at t; B's `/sync` returns it only after the next tick.
  - Event timestamp equals that tick.
  - `/messages`, `/event/{id}`, `/context` don't return it before the tick.
  - Encrypted message + to-device key arrive in the same sync response.
  - With `I = 0`, behaviour identical to upstream.

## 9. Open for the owner

- Interval. 60 s is the handbook's lower bound; longer hides more, feels worse.

## 10. Implementation

- `src/service/release/mod.rs`: watermark, ticker, `released_count()`,
  `is_released()`, `event_timestamp()`. Unit tests for the tick math.
- `src/service/sync/mod.rs`: while enabled, `wake()` / `wake_all_joined()`
  queue users instead of waking them. Each tick takes the queue, *then* moves
  the watermark, then wakes the queued users. Taking first guarantees every
  event behind a queued wake is at or below the new watermark.
  (A first version waited on the watermark inside the sync handlers instead;
  the end-to-end test showed it missed messages sent between two sync
  requests, which then arrived only at the 30 s long-poll timeout.)
- Config key `release_interval_secs` (`src/core/config/mod.rs`, example config).
- Timestamps: `src/service/rooms/timeline/create.rs`.
- Sync v3 and v5: bounded by `released_count()`.
- Per-event reads: gated inside `state_accessor::user_can_see_event`. Every
  caller is client-facing (`/messages`, `/context`, `/event`, `/relations`,
  threads, search, bundled aggregations, unsigned context), so one check covers
  them. Federation uses `server_can_see_event`, which is untouched.
- Room `/initialSync`: filtered by `is_released()`.
- Room state (`/state`, members, joined members, aliases, room initial sync,
  state in search results): all pass through
  `state_accessor::user_can_see_state_events`. If the room's newest event is
  unreleased, it waits for the tick (at most one interval) before answering.
  Tested: a topic change answers 4 ms after the tick with the new topic.

Not gated in this version (known gaps):
- Notification and highlight counts in sync: computed at append time, so a
  room included in a sync for another reason may show a count that already
  includes an unreleased message.
- `/report`: needs an event ID, which clients can't learn before release.

## 11. Test results (2026-10-24, local debug build)

`optes/test/run-e2e.sh <binary> <interval>` starts a throwaway server and runs
`delayed_release_e2e.py` (two users, one message).

Interval 15 s: all pass. Hidden from `/event`, `/messages` and `/sync` before
the tick; delivered to a waiting `/sync` 7 ms after the tick; timestamp equal
to the tick.

Interval 0: all pass. Delivered in 19 ms; timestamp not rounded (upstream
behaviour).
