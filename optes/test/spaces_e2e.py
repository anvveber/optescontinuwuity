#!/usr/bin/env python3
"""Space auto-join and creation limits, against a running server with
auto_join_space_children = true, allow_space_creation = false,
restrict_room_creation_to_spaces = true.

Usage: spaces_e2e.py <base_url> <registration_token> <first_run_token> <tools_dir>
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE, TOKEN, FIRST, TOOLS = sys.argv[1:5]
FAILURES = []
q = urllib.parse.quote


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
    password = "pw-" + uuid.uuid4().hex
    body = {"username": name, "password": password}
    status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status == 401:
        body["auth"] = {"type": "m.login.registration_token", "token": token,
                        "session": resp.get("session")}
        status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status != 200:
        sys.exit(f"register {name}: {status} {resp}")
    return resp["access_token"], resp["user_id"], password


def joined(token):
    return set(call("GET", "/_matrix/client/v3/joined_rooms", token=token)[1]["joined_rooms"])


def wait_until(pred, timeout=10):
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(0.3)
    return pred()


def create(token, name, space=False, parent=None, direct=False, private=False):
    state = []
    if parent:
        state.append({"type": "m.space.parent", "state_key": parent,
                      "content": {"canonical": True, "via": ["spaces.test"]}})
        if not private:
            state.append({"type": "m.room.join_rules", "state_key": "",
                          "content": {"join_rule": "restricted",
                                      "allow": [{"type": "m.room_membership", "room_id": parent}]}})
    body = {"name": name, "preset": "private_chat", "initial_state": state, "is_direct": direct}
    if space:
        body["creation_content"] = {"type": "m.space"}
    return call("POST", "/_matrix/client/v3/createRoom", body, token)


def add_child(token, space, child):
    return call("PUT", f"/_matrix/client/v3/rooms/{q(space)}/state/m.space.child/{q(child)}",
                {"via": ["spaces.test"]}, token)[0]


sfx = uuid.uuid4().hex[:6]
admin, admin_id, admin_pw = register("admin" + sfx, FIRST)
bob, bob_id, _ = register("bob" + sfx, TOKEN)

# --- creation limits -------------------------------------------------------
status, _ = create(bob, "bob's server", space=True)
check("non-admin cannot create a space", status == 403, str(status))
status, _ = create(bob, "random group room")
check("non-admin cannot create a standalone group room", status == 403, str(status))
status, _ = create(bob, "dm", direct=True)
check("non-admin can create a DM", status == 200, str(status))

# --- build a server with the template tool (as admin) ----------------------
template = """
name = "Test server"
channels = ["lobby"]
[[category]]
name = "General"
channels = ["chat", "links"]
[[category]]
name = "Staff"
private = true
channels = ["mods"]
"""
with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
    f.write(template)
out = subprocess.run([sys.executable, os.path.join(TOOLS, "make-space.py"), "--homeserver", BASE,
                      "--user", admin_id, "--password", admin_pw, "--json", f.name],
                     capture_output=True, text=True)
check("template tool runs", out.returncode == 0, out.stderr.strip()[-200:])
rooms = json.loads(out.stdout.strip().splitlines()[-1])
server = rooms["Test server"]
open_rooms = {rooms["lobby"], rooms["General"], rooms["chat"], rooms["links"]}
private_rooms = {rooms["Staff"], rooms["mods"]}

# --- auto-join -------------------------------------------------------------
call("POST", f"/_matrix/client/v3/rooms/{q(server)}/invite", {"user_id": bob_id}, admin)
status, _ = call("POST", f"/_matrix/client/v3/join/{q(server)}", {}, bob)
check("bob joins the server space", status == 200, str(status))
ok = wait_until(lambda: open_rooms <= joined(bob))
check("joining the server joins its channels and categories (incl. nested)", ok,
      f"missing {len(open_rooms - joined(bob))}")
check("private category and its channel are not joined", not (private_rooms & joined(bob)))

status, resp = create(admin, "new-channel", parent=rooms["General"])
new = resp.get("room_id")
add_child(admin, rooms["General"], new)
check("new channel added later is auto-joined", wait_until(lambda: new in joined(bob)))

# --- moderators can add channels, still not servers ------------------------
pl = call("GET", f"/_matrix/client/v3/rooms/{q(server)}/state/m.room.power_levels/", token=admin)[1]
pl.setdefault("users", {})[bob_id] = 50
call("PUT", f"/_matrix/client/v3/rooms/{q(server)}/state/m.room.power_levels/", pl, admin)
status, resp = create(bob, "bob-channel", parent=server)
check("moderator (power 50 in the space) can create a channel in it", status == 200, str(status))
if status == 200:
    check("moderator can add it to the space", add_child(bob, server, resp["room_id"]) == 200)
status, _ = create(bob, "bob's own server", space=True)
check("moderator still cannot create a top-level space", status == 403, str(status))
status, resp = create(bob, "bob-category", space=True, parent=server)
check("moderator can create a category (sub-space) in the server", status == 200, str(status))

# --- leaving ---------------------------------------------------------------
call("POST", f"/_matrix/client/v3/rooms/{q(server)}/leave", {}, bob)
ok = wait_until(lambda: not (open_rooms & joined(bob)))
check("leaving the server leaves its channels", ok, f"still in {len(open_rooms & joined(bob))}")

print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
