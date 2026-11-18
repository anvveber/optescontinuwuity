#!/usr/bin/env python3
"""Per-room retention against a running server with allow_room_retention = true
and a short retention_purge_interval_secs.

Usage: retention_e2e.py <base_url> <registration_token> <first_run_token> <server_log> [<db_dir>]

With <db_dir> (and RocksDB compression off in the server config) it also checks
the database files: the text and IDs of purged messages must be gone from
every file in it, while a kept message must still be there. Only with delayed
release off: with it on, this short test's writes can still sit in RocksDB's
write buffer in memory before the purge, so the files could not show them and
the check would prove nothing.
"""

import os

import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE, TOKEN, FIRST, LOG = sys.argv[1:5]
DB = sys.argv[5] if len(sys.argv) > 5 else None
FAILURES = []
q = urllib.parse.quote
LIFETIME_MS = 20_000
RELEASE_S = int(__import__("os").environ.get("RELEASE", "0"))
if RELEASE_S:
    DB = None


def settle():
    """With delayed release on, wait until the next tick has released what we sent."""
    if RELEASE_S:
        time.sleep(RELEASE_S - time.time() % RELEASE_S + 1)


def call(method, path, body=None, token=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.status, json.loads(resp.read() or b"{}")
    except urllib.error.HTTPError as err:
        return err.code, json.loads(err.read() or b"{}")


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""))
    if not ok:
        FAILURES.append(name)


def register(name, token):
    body = {"username": name, "password": "pw-" + uuid.uuid4().hex}
    status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status == 401:
        body["auth"] = {"type": "m.login.registration_token", "token": token,
                        "session": resp.get("session")}
        status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status != 200:
        sys.exit(f"register {name}: {status} {resp}")
    return resp["access_token"], resp["user_id"]


def send(token, room, content, etype="m.room.message"):
    status, resp = call("PUT", f"/_matrix/client/v3/rooms/{q(room)}/send/{etype}/{uuid.uuid4().hex}",
                        content, token)
    assert status == 200, (status, resp)
    return resp["event_id"]


def text(body, **extra):
    return {"msgtype": "m.text", "body": body, **extra}


def room_with(token, name, lifetime=None):
    state = []
    if lifetime is not None:
        state.append({"type": "m.room.retention", "state_key": "", "content": {"max_lifetime": lifetime}})
    return call("POST", "/_matrix/client/v3/createRoom",
                {"preset": "public_chat", "name": name, "initial_state": state}, token)[1]["room_id"]


def timeline_ids(token, room):
    _, msgs = call("GET", f"/_matrix/client/v3/rooms/{q(room)}/messages?dir=b&limit=100", token=token)
    return {e["event_id"] for e in msgs.get("chunk", [])}


def sync_ids(token, room):
    _, s = call("GET", "/_matrix/client/v3/sync?timeout=0", token=token)
    return {e["event_id"] for e in s.get("rooms", {}).get("join", {}).get(room, {})
            .get("timeline", {}).get("events", [])}


FILTER = q(json.dumps({"room": {"timeline": {"limit": 50}}}))


def sync_since_ids(token, room, since):
    """An incremental /sync that continues from an old token."""
    _, s = call("GET", f"/_matrix/client/v3/sync?timeout=0&filter={FILTER}&since={since}", token=token)
    return {e["event_id"] for e in s.get("rooms", {}).get("join", {}).get(room, {})
            .get("timeline", {}).get("events", [])}


SLIDING = "/_matrix/client/unstable/org.matrix.simplified_msc3575/sync"


def sliding(token, room, conn=None, pos=None):
    """Sliding sync (MSC4186) of one room: (event IDs, conn, pos). Without
    conn: a new connection (initial sliding sync)."""
    conn = conn or uuid.uuid4().hex
    body = {"conn_id": conn, "room_subscriptions": {room: {"timeline_limit": 50, "required_state": []}}}
    status, resp = call("POST", SLIDING + "?timeout=0" + (f"&pos={pos}" if pos else ""), body, token)
    assert status == 200, (status, resp)
    events = resp.get("rooms", {}).get(room, {}).get("timeline", [])
    return {e["event_id"] for e in events}, conn, resp.get("pos")


def readable(token, room, eid):
    return call("GET", f"/_matrix/client/v3/rooms/{q(room)}/event/{q(eid)}", token=token)[0] == 200


sfx = uuid.uuid4().hex[:6]
admin, _ = register("admin" + sfx, FIRST)
alice, _ = register("alice" + sfx, TOKEN)
bob, bob_id = register("bob" + sfx, TOKEN)

word = "kumquat" + sfx
temp = room_with(alice, "temp", LIFETIME_MS)
perm = room_with(alice, "permanent")
call("POST", f"/_matrix/client/v3/join/{q(temp)}", {}, bob)
settle()
# Positions taken before the messages exist: a client that was offline
# continues from such a position (audit L10).
since_old = call("GET", "/_matrix/client/v3/sync?timeout=0", token=bob)[1]["next_batch"]
_, ss_control, ss_control_pos = sliding(bob, temp)
_, ss_after, ss_after_pos = sliding(bob, temp)

m1 = send(alice, temp, text(f"first {word}"))
m2 = send(alice, temp, text(f"second marker{sfx}m2"))
reply = send(bob, temp, text(f"reply marker{sfx}re", **{"m.relates_to": {"m.in_reply_to": {"event_id": m1}}}))
reaction = send(bob, temp, {"m.relates_to": {"rel_type": "m.annotation", "event_id": m2, "key": "+1"}},
                "m.reaction")
thread = send(bob, temp, text(f"in thread marker{sfx}th", **{"m.relates_to": {"rel_type": "m.thread", "event_id": m1}}))
p1 = send(alice, perm, text(f"keep me marker{sfx}p1"))
old = {m1, m2, reply, reaction, thread}
settle()

check("messages visible before expiry", old <= timeline_ids(alice, temp))
check("control: sync from an old token has them before expiry", old <= sync_since_ids(bob, temp, since_old))
check("control: sliding sync from an old pos has them before expiry",
      old <= sliding(bob, temp, ss_control, ss_control_pos)[0])


def search_hits():
    _, res = call("POST", "/_matrix/client/v3/search",
                  {"search_categories": {"room_events": {"search_term": word}}}, alice)
    return res.get("search_categories", {}).get("room_events", {}).get("results", [])


def relations(event):
    status, rel = call("GET", f"/_matrix/client/v1/rooms/{q(temp)}/relations/{q(event)}", token=alice)
    return status, rel.get("chunk")


check("control: search finds the message before expiry", len(search_hits()) == 1, str(len(search_hits())))
status, chunk = relations(m2)
check("control: relations of m2 list the reaction before expiry",
      status == 200 and any(e["event_id"] == reaction for e in chunk or []), f"{status} {chunk}")

time.sleep(LIFETIME_MS / 1000 + 2)
newest = send(alice, temp, text("newest"))
settle()

# --- hidden right away -----------------------------------------------------
check("expired messages gone from /messages", not (old & timeline_ids(bob, temp)))
check("expired messages gone from initial sync", not (old & sync_ids(bob, temp)))
check("expired messages gone from a sync that continues from an old token",
      not (old & sync_since_ids(bob, temp, since_old)))
check("expired messages gone from initial sliding sync", not (old & sliding(bob, temp)[0]))
check("expired messages gone from sliding sync from an old pos",
      not (old & sliding(bob, temp, ss_after, ss_after_pos)[0]))
check("expired message not readable via /event", not readable(bob, temp, m1))
_, ctx = call("GET", f"/_matrix/client/v3/rooms/{q(temp)}/context/{q(m1)}", token=bob)
check("expired message not served via /context", "event" not in ctx)
hits = search_hits()
check("expired message not found by search", not hits, f"{len(hits)} hits")
check("newest message still visible", newest in timeline_ids(bob, temp))
check("room without retention keeps its messages", readable(alice, perm, p1))

# --- on disk before the purge (control for the disk check below) ----------
PURGED_MARKERS = [word, f"marker{sfx}m2", f"marker{sfx}re", f"marker{sfx}th"] + sorted(old)


def on_disk(marker, allow_prev_events=False):
    """Files under the database directory whose bytes contain marker. With
    allow_prev_events, an occurrence inside a kept event's "prev_events" list
    does not count: the room's history keeps those links."""
    found = []
    needle = marker.encode()
    for root, _, files in os.walk(DB):
        for name in files:
            try:
                with open(os.path.join(root, name), "rb") as f:
                    raw = f.read()
            except OSError:
                continue
            at = raw.find(needle)
            while at >= 0:
                before = raw[max(0, at - 400):at]
                k = before.rfind(b'"prev_events":[')
                linked = allow_prev_events and k >= 0 and b"]" not in before[k:]
                if not linked:
                    found.append(os.path.relpath(os.path.join(root, name), DB))
                    break
                at = raw.find(needle, at + 1)
    return found


if DB:
    seen = [m for m in PURGED_MARKERS if on_disk(m)]
    check("control: the expired messages are in the database files before the purge",
          len(seen) == len(PURGED_MARKERS), f"{len(seen)} of {len(PURGED_MARKERS)} found")

# --- deleted by the purge job ---------------------------------------------
time.sleep(6)  # purge interval in the test config is 3 s
if DB:
    left = {}
    for _ in range(30):
        left = {m: on_disk(m, allow_prev_events=m.startswith("$")) for m in PURGED_MARKERS}
        left = {m: f for m, f in left.items() if f}
        if not left:
            break
        time.sleep(1)
    check("purged messages are gone from every database file (text; event IDs except in kept events' prev_events)", not left,
          "; ".join(f"{m[:20]}: {', '.join(f)}" for m, f in list(left.items())[:4]))
    check("control: the kept message is still in the database files", bool(on_disk(f"marker{sfx}p1")))
call("PUT", f"/_matrix/client/v3/rooms/{q(temp)}/state/m.room.retention/",
     {"max_lifetime": 10 * 365 * 86400 * 1000}, alice)
settle()
reappeared = old & timeline_ids(alice, temp)
check("after purge, lifting the policy brings nothing back (really deleted)", not reappeared,
      f"{len(reappeared)} back")
check("lifting the policy shows recent messages again", newest in timeline_ids(alice, temp))
status, chunk = relations(m2)
check("relations of a purged event are empty", status in (200, 403, 404) and not chunk, f"{status} {chunk}")
check("search finds nothing even with the policy lifted", not search_hits())

# --- room still works, state kept -----------------------------------------
_, name = call("GET", f"/_matrix/client/v3/rooms/{q(temp)}/state/m.room.name/", token=bob)
check("state events kept (room name)", name.get("name") == "temp", str(name))
_, members = call("GET", f"/_matrix/client/v3/rooms/{q(temp)}/joined_members", token=alice)
check("membership kept", bob_id in members.get("joined", {}), str(list(members.get("joined", {}))))
after = send(bob, temp, text("still works"))
settle()
check("room accepts new messages after purge", readable(alice, temp, after))
carol, _ = register("carol" + sfx, TOKEN)
status, _ = call("POST", f"/_matrix/client/v3/join/{q(temp)}", {}, carol)
settle()
check("new member can join after purge", status == 200, str(status))
check("new member sees the history that remains", after in timeline_ids(carol, temp))

log = open(LOG).read()
check("server never saw a purged event still readable", "still readable after purge" not in log)
check("server logged the purge", "purged expired events" in log or "retention purge" in log)

print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
