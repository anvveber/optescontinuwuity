#!/usr/bin/env python3
"""End-to-end check of delayed release against a running server.

Usage: delayed_release_e2e.py <base_url> <registration_token> <interval_secs> [first_run_token]

interval_secs = 0 checks that the disabled path behaves like upstream.
Uses only the standard library.
"""

import json
import sys
import time
import urllib.error
import urllib.request
import uuid

BASE, TOKEN, INTERVAL = sys.argv[1], sys.argv[2], int(sys.argv[3])
FIRST_RUN_TOKEN = sys.argv[4] if len(sys.argv) > 4 else ""
INTERVAL_MS = INTERVAL * 1000
FAILURES = []


def call(method, path, body=None, token=None, timeout=60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, json.loads(resp.read() or b"{}")
    except urllib.error.HTTPError as err:
        return err.code, json.loads(err.read() or b"{}")


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""))
    if not ok:
        FAILURES.append(name)


def register(username, token=None):
    token = token or TOKEN
    body = {"username": username, "password": "pw-" + uuid.uuid4().hex}
    status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status == 401:
        body["auth"] = {
            "type": "m.login.registration_token",
            "token": token,
            "session": resp.get("session"),
        }
        status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status != 200:
        sys.exit(f"register {username} failed: {status} {resp}")
    return resp["access_token"]


def now_ms():
    return int(time.time() * 1000)


def wait_until_just_after_tick():
    """Start right after a tick so the send lands mid-window."""
    if INTERVAL_MS == 0:
        return
    while now_ms() % INTERVAL_MS > 1000:
        time.sleep(0.1)
    time.sleep(1.5)


def main():
    suffix = uuid.uuid4().hex[:6]
    if FIRST_RUN_TOKEN:
        register("admin" + suffix, FIRST_RUN_TOKEN)
    alice = register("alice" + suffix)
    bob = register("bob" + suffix)

    _, room = call("POST", "/_matrix/client/v3/createRoom",
                   {"preset": "private_chat", "invite": []}, alice)
    room_id = room["room_id"]
    _, me = call("GET", "/_matrix/client/v3/account/whoami", token=bob)
    call("POST", f"/_matrix/client/v3/rooms/{room_id}/invite",
         {"user_id": me["user_id"]}, alice)

    # Let the invite through (one tick at most), then join.
    time.sleep(INTERVAL + 1)
    status, _ = call("POST", f"/_matrix/client/v3/join/{room_id}", {}, bob)
    check("bob joins", status == 200, str(status))
    time.sleep(INTERVAL + 1)

    _, sync = call("GET", "/_matrix/client/v3/sync?timeout=0", token=bob)
    since = sync["next_batch"]

    wait_until_just_after_tick()
    sent_at = now_ms()
    status, resp = call(
        "PUT",
        f"/_matrix/client/v3/rooms/{room_id}/send/m.room.message/{uuid.uuid4().hex}",
        {"msgtype": "m.text", "body": "hello"}, alice)
    check("send returns at once", status == 200 and now_ms() - sent_at < 3000)
    event_id = resp["event_id"]

    if INTERVAL:
        status, _ = call("GET", f"/_matrix/client/v3/rooms/{room_id}/event/{event_id}", token=bob)
        check("/event hidden before tick", status in (403, 404), str(status))

        _, msgs = call("GET", f"/_matrix/client/v3/rooms/{room_id}/messages?dir=b&limit=20", token=bob)
        ids = [e["event_id"] for e in msgs.get("chunk", [])]
        check("/messages hidden before tick", event_id not in ids)

        _, quick = call("GET", f"/_matrix/client/v3/sync?timeout=0&since={since}", token=bob)
        timeline = quick.get("rooms", {}).get("join", {}).get(room_id, {}).get("timeline", {}).get("events", [])
        check("sync (no wait) hidden before tick", event_id not in [e["event_id"] for e in timeline])

    # Long-poll until it arrives.
    found, got_at, ts = False, None, None
    deadline = time.time() + INTERVAL + 40
    while time.time() < deadline and not found:
        _, sync = call("GET", f"/_matrix/client/v3/sync?timeout=30000&since={since}", token=bob, timeout=90)
        since = sync["next_batch"]
        events = sync.get("rooms", {}).get("join", {}).get(room_id, {}).get("timeline", {}).get("events", [])
        for e in events:
            if e["event_id"] == event_id:
                found, got_at, ts = True, now_ms(), e["origin_server_ts"]
    check("bob receives the message", found)
    if not found:
        return

    delay = got_at - sent_at
    if INTERVAL:
        next_tick = -(-sent_at // INTERVAL_MS) * INTERVAL_MS
        check("timestamp is the tick", ts == next_tick, f"ts={ts} tick={next_tick}")
        check("delivered at the tick, not before", got_at >= next_tick - 200,
              f"delay={delay}ms, tick in {next_tick - sent_at}ms")
        check("delivered within ~1 s of the tick", got_at - next_tick < 1500,
              f"{got_at - next_tick}ms after tick")
        status, _ = call("GET", f"/_matrix/client/v3/rooms/{room_id}/event/{event_id}", token=bob)
        check("/event visible after tick", status == 200, str(status))

        # State endpoints: a topic change must not show before its tick.
        wait_until_just_after_tick()
        topic = "topic " + uuid.uuid4().hex[:6]
        t_set = now_ms()
        call("PUT", f"/_matrix/client/v3/rooms/{room_id}/state/m.room.topic/", {"topic": topic}, alice)
        status, got = call("GET", f"/_matrix/client/v3/rooms/{room_id}/state/m.room.topic/", token=bob)
        t_got = now_ms()
        tick = -(-t_set // INTERVAL_MS) * INTERVAL_MS
        check("/state waits for the tick", t_got >= tick - 200, f"answered {t_got - tick} ms relative to tick")
        check("/state then shows the new topic", got.get("topic") == topic, json.dumps(got))
    else:
        check("delivered immediately", delay < 3000, f"delay={delay}ms")
        check("timestamp not rounded", abs(ts - sent_at) < 3000, f"ts-sent={ts - sent_at}ms")


def check_no_device_activity():
    """record_device_activity = false: no last-seen data stored."""
    token = register("carol" + uuid.uuid4().hex[:6])
    call("GET", "/_matrix/client/v3/sync?timeout=0", token=token)
    _, devs = call("GET", "/_matrix/client/v3/devices", token=token)
    devices = devs.get("devices", [])
    leaked = [d for d in devices if d.get("last_seen_ts") or d.get("last_seen_ip")]
    check("no device last-seen stored", devices and not leaked, json.dumps(devices))


main()
check_no_device_activity()
print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
