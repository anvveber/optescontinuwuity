#!/usr/bin/env python3
"""require_encryption, hide_server_identity and show_device_names_to_others,
against a running server.

Usage: hardening_e2e.py <base_url> <registration_token> <first_run_token> <on|off>
  on  = require_encryption and hide_server_identity enabled,
        show_device_names_to_others disabled;
  off = the opposite (checks upstream behaviour)
"""

import json
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE, TOKEN, FIRST, MODE = sys.argv[1:5]
ON = MODE == "on"
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
            raw = resp.read()
            return resp.status, json.loads(raw) if raw.startswith(b"{") else {}
    except urllib.error.HTTPError as err:
        raw = err.read()
        return err.code, json.loads(raw) if raw.startswith(b"{") else {}


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""))
    if not ok:
        FAILURES.append(name)


def register(name, token, device_name=None, full=False):
    body = {"username": name, "password": "pw-" + uuid.uuid4().hex}
    if device_name:
        body["initial_device_display_name"] = device_name
    status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status == 401:
        body["auth"] = {"type": "m.login.registration_token", "token": token,
                        "session": resp.get("session")}
        status, resp = call("POST", "/_matrix/client/v3/register", body)
    if status != 200:
        sys.exit(f"register {name}: {status} {resp}")
    if full:
        return resp["access_token"], resp["user_id"], resp["device_id"]
    return resp["access_token"], resp["user_id"]


def send(token, room, etype, content):
    return call("PUT", f"/_matrix/client/v3/rooms/{q(room)}/send/{etype}/{uuid.uuid4().hex}",
                content, token)


sfx = uuid.uuid4().hex[:6]
admin, admin_id = register("admin" + sfx, FIRST)
server_name = admin_id.split(":", 1)[1]

# --- require_encryption ----------------------------------------------------
_, room = call("POST", "/_matrix/client/v3/createRoom", {"preset": "private_chat"}, admin)
rid = room["room_id"]
status, enc = call("GET", f"/_matrix/client/v3/rooms/{q(rid)}/state/m.room.encryption/", token=admin)
if ON:
    check("new room is encrypted without the client asking", status == 200, str(enc))
else:
    check("(off) new room not encrypted unless asked", status == 404, str(status))

_, space = call("POST", "/_matrix/client/v3/createRoom",
                {"preset": "private_chat", "creation_content": {"type": "m.space"}}, admin)
status, _ = call("GET", f"/_matrix/client/v3/rooms/{q(space['room_id'])}/state/m.room.encryption/",
                 token=admin)
check("spaces are left unencrypted", status == 404, str(status))

status, _ = send(admin, rid, "m.room.message", {"msgtype": "m.text", "body": "plaintext"})
check("plaintext message " + ("refused" if ON else "accepted (off)"),
      status == (403 if ON else 200), str(status))
status, _ = send(admin, rid, "m.sticker", {"body": "s", "url": "mxc://x/y", "info": {}})
check("plaintext sticker " + ("refused" if ON else "accepted (off)"),
      status == (403 if ON else 200), str(status))
status, sent = send(admin, rid, "m.room.encrypted", {
    "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "AAAA", "sender_key": "x",
    "device_id": "D", "session_id": "S"})
check("encrypted message accepted", status == 200, str(status))
status, _ = send(admin, rid, "m.reaction",
                 {"m.relates_to": {"rel_type": "m.annotation", "event_id": sent.get("event_id"), "key": "+1"}})
check("reaction (sent unencrypted by clients) accepted", status == 200, str(status))
status, _ = call("PUT", f"/_matrix/client/v3/rooms/{q(rid)}/redact/{q(sent.get('event_id', ''))}/{uuid.uuid4().hex}",
                 {}, admin)
check("redaction accepted", status == 200, str(status))

status, alias = call("GET", f"/_matrix/client/v3/directory/room/{q('#admins:' + server_name)}", token=admin)
if status == 200:
    status, _ = send(admin, alias["room_id"], "m.room.message", {"msgtype": "m.text", "body": "!admin server uptime"})
    check("admin room still takes plaintext admin commands", status == 200, str(status))
else:
    check("admin room found", False, str(status))

# --- show_device_names_to_others -------------------------------------------
NAME = "Element Desktop: Linux"
CLIENT_NAME = "Name the client put in its own keys"
alice, alice_id, alice_dev = register("alice" + sfx, TOKEN, NAME, full=True)
bob, bob_id = register("bob" + sfx, TOKEN)
status, _ = call("POST", "/_matrix/client/v3/keys/upload", {"device_keys": {
    "user_id": alice_id, "device_id": alice_dev,
    "algorithms": ["m.olm.v1.curve25519-aes-sha2", "m.megolm.v1.aes-sha2"],
    "keys": {f"curve25519:{alice_dev}": "c" * 43, f"ed25519:{alice_dev}": "e" * 43},
    "signatures": {alice_id: {f"ed25519:{alice_dev}": "s" * 86}},
    "unsigned": {"device_display_name": CLIENT_NAME}}}, alice)
check("device keys uploaded", status == 200, str(status))


def shown_name(token):
    status, resp = call("POST", "/_matrix/client/v3/keys/query", {"device_keys": {alice_id: []}}, token)
    dev = resp.get("device_keys", {}).get(alice_id, {}).get(alice_dev)
    if status != 200 or dev is None:
        return None, json.dumps(resp)
    return dev.get("unsigned", {}).get("device_display_name"), json.dumps(resp)


name, raw = shown_name(bob)
if ON:
    check("another user sees the device ID, not the name", name == alice_dev, str(name))
    check("neither name appears anywhere in the reply", NAME not in raw and CLIENT_NAME not in raw)
else:
    check("(off) another user sees the device name", name == NAME, str(name))
name, _ = shown_name(alice)
check("the owner sees the name of their own device", name == NAME, str(name))

# --- hide_server_identity --------------------------------------------------
for path in ("/_continuwuity/server_version", "/", "/_continuwuity/about",
             "/_continuwuity/_debug/error"):
    status, _ = call("GET", path)
    if ON:
        check(f"{path} hidden", status == 404, str(status))
    else:
        check(f"(off) {path} served", status != 404, str(status))
status, _ = call("GET", "/_matrix/client/versions")
check("client API still works", status == 200, str(status))

print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
