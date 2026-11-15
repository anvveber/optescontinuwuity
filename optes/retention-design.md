# Per-channel retention — design note

Status: implemented 2026-10-25 (`src/service/retention/`). Decisions:
**D1 = purge whole events, D2 = keep the server-wide 30-day media rule,
D3 = keep forever by default.**

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

## Erasing from the database files (2026-11-15)

Deleting a key in RocksDB writes a "deleted" mark; the old bytes stay in the
write-ahead log and in older table files until a compaction rewrites them.
Measured before this change: after a purge, the text of every purged message
was still in the write-ahead log (`000004.log`).

So after every purge run that removed something, the server:
- deletes the client transaction IDs that point to purged events (user, device
  and the client's send ID, which in Element carries the send time);
- writes every memory table to a file, which retires the write-ahead logs;
- compacts every table (forced down to the last level), which drops deleted
  entries from all older files, also older deletions in other tables that name
  purged events (e.g. former forward extremities).

And with `allow_room_retention` on, RocksDB:
- keeps no archive of retired write-ahead logs (`wal_size_limit_mb = 0`;
  upstream keeps up to 1 GB, i.e. on a small server everything ever written);
- rewrites its file index (MANIFEST) after every change, since it records the
  first and last key of every file it ever listed (`max_manifest_file_size`).

What stays:
- the event IDs of purged events where kept events name them (`prev_events`):
  the room's history needs those links. An ID is a hash; it says that an event
  existed, not what it said or who sent it.
- the disk blocks of deleted files: the filesystem frees them but does not
  overwrite them. Readable by the host (handbook §4.6) until reused.
- backups and disk snapshots taken before the purge.

Cost: every purge run that removed something compacts the whole database.
On a small server that is short; on a large one it is real disk work, at
most every `retention_purge_interval_secs` (900 s). Not measured on a database
of real size. The server logs the duration ("erased purged events from the
database files", `elapsed_ms`).

Test: `run-retention-e2e.sh` (delayed release off) checks every file in the
database directory, with compression off so text can be found: before the
purge all 9 markers (texts and event IDs) are there; after it no text and no
ID outside `prev_events`; a kept message still there. The same check fails on
the binary before this change. With delayed release on, the test's writes can
still be in RocksDB's memory buffer before the purge, so the check is skipped
there.

## Tests (planned)

- expired message hidden from `/messages`, `/event`, `/context`, sync;
- after a purge run: gone from the database (checked through every read
  endpoint and the admin room info commands);
- state events and the newest event survive;
- rooms without retention unaffected; changing `max_lifetime` applies to
  existing messages;
- room keeps working after a purge (send, join, paginate).
