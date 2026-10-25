# Per-channel retention — design note

Status: design, 2026-10-25. Not implemented. Decisions marked **D1–D3** are
the owner's.

## Goal

Per room: messages older than N days are removed from the server. Rooms without
a setting keep everything. So `#rules` or an effort-post channel stays forever;
`#chat` forgets after, say, 30 days.

## Format: MSC1763 `m.room.retention`

A state event in the room:

```json
{ "type": "m.room.retention", "state_key": "",
  "content": { "max_lifetime": 2592000000 } }
```

`max_lifetime` in milliseconds (30 days above). `min_lifetime` exists in the
MSC; Synapse ignores it and so would we. Not yet in the Matrix spec; Synapse
has implemented it for years as experimental. Clients mostly don't show it, so
it is set by `make-space.py` (`retention = "30d"` per channel) or by sending the
state event directly. Changing it needs the room's state power level (50).

## Behaviour (following Synapse)

1. An event is *expired* when `origin_server_ts + max_lifetime < now`.
2. Expired events are hidden from clients at once (same single check point as
   delayed release: `user_can_see_event`, sync, `/messages`).
3. A purge job (every hour) deletes them from the database.
4. Never touched: **state events** (names, membership, power levels, the
   retention event itself) and the **newest event in the room** (the room
   needs one to continue the history graph).

## D1 — remove content, or remove the whole event?

| | Redact (content only) | Purge (whole event) |
|---|---|---|
| What's left | a skeleton: sender, time, event type | nothing |
| Effort | small: reuse the existing redaction code | larger: delete from ~10 database tables (timeline, relations, threads, search, receipts…) |
| Risk | low; redaction is a normal Matrix operation | medium; a missed table leaves stale references; needs careful tests |
| Protects | message content (already end-to-end encrypted) against a future key leak | content **and** who-wrote-when |

Recommendation: **purge**. Content is already encrypted, so redaction alone
mostly protects against key leaks; the remaining exposure on a seized disk is
the metadata (who talked when), which only purging removes.

## D2 — media (images, files)

The server stores who uploaded a file, not which room it was for: in encrypted
rooms that link is inside the encrypted message. So media can't follow a
channel's retention. Options:

- **a)** Keep one server-wide media expiry (currently: **all media is deleted
  after 30 days** — this already conflicts with permanent channels: images in
  `#rules` would vanish after a month).
- **b)** No media expiry. Files stay; they're encrypted blobs whose keys live
  in the (deleted or kept) messages and in users' key backups.
- **c)** a), with a long period (e.g. 1 year).
- **d)** Later: MSC3911 (clients tell the server which event a file belongs to).
  Not supported by clients yet.

Recommendation: **b** for now if permanent channels with images matter, else
**c**. Either way the current 30-day media rule should change once permanent
channels exist.

## D3 — server-wide default for rooms without a setting

- keep forever (Synapse's default), or
- a default `max_lifetime` (e.g. 1 year), with permanent channels opting out
  via a very large value.

Recommendation: keep forever; set retention per channel.

## Limits

- Clients that already downloaded old messages keep them locally; the server
  can't delete from devices.
- Key backups (if users use them) keep message keys; deleting the message on
  the server makes those keys useless, which is the point.
- Delayed release is unaffected; retention uses real event times.

## Tests (planned)

- expired message hidden from `/messages`, `/event`, `/context`, sync;
- after a purge run: gone from the database (checked through every read
  endpoint and the admin room info commands);
- state events and the newest event survive;
- rooms without retention unaffected; changing `max_lifetime` applies to
  existing messages;
- room keeps working after a purge (send, join, paginate).
