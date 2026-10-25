#!/usr/bin/env python3
"""Matrix traffic through the relay, then checks on what crossed the wire.

Usage: tunnel_e2e.py <relay_url> <reg_token> <first_run_token> <level>
                     <wire_log> <window_secs> <tick_ms>
"""

import json
import sys
import time
import urllib.error
import urllib.request
import uuid

BASE, TOKEN, FIRST, LEVEL, WIRE, WINDOW, TICK = sys.argv[1:8]
LEVEL, WINDOW, TICK = int(LEVEL), int(WINDOW), int(TICK)
FRAME = 2048
FAILURES = []


def call(method, path, body=None, token=None, timeout=120):
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


def wait_for_tunnel():
    for _ in range(120):
        try:
            with urllib.request.urlopen(BASE + "/_matrix/client/versions", timeout=60):
                return
        except Exception:
            time.sleep(1)
    sys.exit("relay never answered")


def matrix_flow():
    sfx = uuid.uuid4().hex[:6]
    if FIRST:  # empty when the first account already exists
        register("admin" + sfx, FIRST)
    alice, _ = register("alice" + sfx, TOKEN)
    bob, bob_id = register("bob" + sfx, TOKEN)
    _, room = call("POST", "/_matrix/client/v3/createRoom", {"preset": "private_chat"}, alice)
    rid = room["room_id"]
    call("POST", f"/_matrix/client/v3/rooms/{rid}/invite", {"user_id": bob_id}, alice)
    status, _ = call("POST", f"/_matrix/client/v3/join/{rid}", {}, bob)
    check("join through tunnel", status == 200, str(status))

    _, sync = call("GET", "/_matrix/client/v3/sync?timeout=0", token=bob)
    since = sync["next_batch"]
    body = "hello " + uuid.uuid4().hex  # unique, also checks content integrity
    big = "x" * 60_000  # spans many cells
    t0 = time.time()
    status, resp = call("PUT", f"/_matrix/client/v3/rooms/{rid}/send/m.room.message/{uuid.uuid4().hex}",
                        {"msgtype": "m.text", "body": body, "pad": big}, alice)
    check("send through tunnel", status == 200, str(status))
    eid = resp.get("event_id")

    got = None
    deadline = time.time() + 4 * WINDOW + 60
    while time.time() < deadline and got is None:
        _, sync = call("GET", f"/_matrix/client/v3/sync?timeout=10000&since={since}", token=bob)
        since = sync.get("next_batch", since)
        for e in sync.get("rooms", {}).get("join", {}).get(rid, {}).get("timeline", {}).get("events", []):
            if e["event_id"] == eid:
                got = e
    check("bob receives through tunnel", got is not None)
    if got:
        check("content intact (60 KB event)",
              got["content"]["body"] == body and got["content"]["pad"] == big)
        print(f"     round trip {time.time() - t0:.1f}s at level {LEVEL}")


def wire_checks():
    rows = [line.split() for line in open(WIRE)]
    up = [(int(t), int(n)) for d, t, n in rows if d == "up"]
    down = [(int(t), int(n)) for d, t, n in rows if d == "down"]
    for name, chunks in (("up", up), ("down", down)):
        total = sum(n for _, n in chunks)
        check(f"{name}: whole frames only", total % FRAME == 0, f"{total} bytes")

    if LEVEL == 3:
        # Idle period: count frames per second in both directions.
        time.sleep(5)
        rows = [line.split() for line in open(WIRE)]
        end = int(time.time() * 1000)
        for d in ("up", "down"):
            recent = sum(int(n) for dd, t, n in rows if dd == d and int(t) > end - 4000) / FRAME
            expected = 4000 / TICK
            check(f"level 3 {d}: constant rate while idle",
                  abs(recent - expected) <= 2, f"{recent:.0f} frames in 4 s, expected {expected:.0f}")

    if LEVEL == 2:
        # Idle: bursts only near window boundaries (+1 s for down).
        time.sleep(2 * WINDOW + 1)
        rows = [line.split() for line in open(WIRE)]
        end = int(time.time() * 1000)
        w = WINDOW * 1000
        for d, offset in (("up", 0), ("down", 1000)):
            times = [int(t) for dd, t, n in rows if dd == d and int(t) > end - 2 * w]
            off_grid = [t for t in times if min((t - offset) % w, w - (t - offset) % w) > 500]
            check(f"level 2 {d}: idle traffic only at window boundaries",
                  times and not off_grid, f"{len(times)} chunks, {len(off_grid)} off-grid")
            per_window = {}
            for dd, t, n in rows:
                if dd == d and int(t) > end - 2 * w:
                    k = (int(t) - offset + 500) // w
                    per_window[k] = per_window.get(k, 0) + int(n) // FRAME
            check(f"level 2 {d}: every window sends >= 4 frames",
                  per_window and min(per_window.values()) >= 4, str(per_window))


wait_for_tunnel()
matrix_flow()
wire_checks()
print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
