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
leaked = private_rooms & joined(bob)
check("private category and its channel are not joined", not leaked)

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

# --- rooms that do not name the space are not its channels ------------------
# Adding a child needs power in the space only. A moderator must not be able
# to pull members into a room that way, or to take them out of one.
carol, carol_id, _ = register("carol" + sfx, TOKEN)
dave, dave_id, _ = register("dave" + sfx, TOKEN)
call("POST", f"/_matrix/client/v3/rooms/{q(server)}/invite", {"user_id": carol_id}, admin)
call("POST", f"/_matrix/client/v3/join/{q(server)}", {}, carol)
check("carol joins the server and its channels", wait_until(lambda: open_rooms <= joined(carol)))

# A private conversation between carol and dave. Bob (moderator) is not in it.
status, resp = call("POST", "/_matrix/client/v3/createRoom",
                    {"preset": "trusted_private_chat", "is_direct": True, "invite": [dave_id]}, carol)
dm = resp.get("room_id")
call("POST", f"/_matrix/client/v3/join/{q(dm)}", {}, dave)
# An open room outside the server, made by the admin.
status, resp = call("POST", "/_matrix/client/v3/createRoom", {"name": "elsewhere", "preset": "public_chat"}, admin)
outside = resp.get("room_id")
# A room bob invited carol to; she has not accepted.
status, resp = create(bob, "bob-room", parent=server, private=True)
bob_room = resp.get("room_id")
call("POST", f"/_matrix/client/v3/rooms/{q(bob_room)}/invite", {"user_id": carol_id}, bob)
status, resp = call("POST", "/_matrix/client/v3/createRoom",
                    {"preset": "trusted_private_chat", "is_direct": True, "invite": [carol_id]}, bob)
bob_dm = resp.get("room_id")

check("moderator can list a foreign room as a child (the space allows it)",
      add_child(bob, server, dm) == 200 and add_child(bob, server, outside) == 200
      and add_child(bob, server, bob_dm) == 200)
add_child(bob, server, bob_room)
check("a channel of the server is still joined through an open invitation",
      wait_until(lambda: bob_room in joined(carol)))
time.sleep(2)  # the background work for the other three would have run by now
check("members are not joined to an open room that does not name the space", outside not in joined(carol))
check("an invitation to a room that does not name the space is not accepted for the member",
      bob_dm not in joined(carol))

status, _ = call("POST", f"/_matrix/client/v3/rooms/{q(server)}/kick", {"user_id": carol_id}, bob)
check("moderator removes carol from the server", status == 200, str(status))
check("removal from the server leaves its channels",
      wait_until(lambda: not ((open_rooms | {bob_room}) & joined(carol))),
      f"still in {len((open_rooms | {bob_room}) & joined(carol))}")
check("removal from the server does not touch a room that does not name the space",
      dm in joined(carol))

# --- a profile change does not re-join a channel the member left ------------
# A new display name is a join -> join member event in every room, the
# space included (audit L10). Only a real join may join channels.
links = rooms["links"]
call("POST", f"/_matrix/client/v3/rooms/{q(links)}/leave", {}, bob)
check("bob leaves one channel", wait_until(lambda: links not in joined(bob)))
new_name = "bob renamed " + sfx
status, _ = call("PUT", f"/_matrix/client/v3/profile/{q(bob_id)}/displayname", {"displayname": new_name}, bob)
check("bob changes his display name", status == 200, str(status))
check("control: the new name reached the space",
      wait_until(lambda: call("GET", f"/_matrix/client/v3/rooms/{q(server)}/state/m.room.member/{q(bob_id)}",
                              token=bob)[1].get("displayname") == new_name))
time.sleep(2)  # the auto-join work would have run by now
check("a profile change does not re-join a channel the member left", links not in joined(bob))
check("the other channels stay joined", (open_rooms - {links}) <= joined(bob))

# --- leaving ---------------------------------------------------------------
call("POST", f"/_matrix/client/v3/rooms/{q(server)}/leave", {}, bob)
ok = wait_until(lambda: not (open_rooms & joined(bob)))
check("leaving the server leaves its channels", ok, f"still in {len(open_rooms & joined(bob))}")

print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
