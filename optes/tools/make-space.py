#!/usr/bin/env python3
"""Create a Discord-style server (a Matrix space with categories and channels)
from a TOML template.

Usage:
  make-space.py --homeserver http://127.0.0.1:8008 --user alice template.toml

Asks for the password, logs in, creates everything, logs out again (so no
extra session is left behind). Needs Python 3.11+, nothing else.

Who may run it depends on the server: with allow_space_creation = false only
server admins can create the top-level space.

Template format (see example-server.toml):

  name = "My server"
  topic = "optional"
  channels = ["lobby"]                 # channels above all categories

  [[category]]
  name = "General"
  channels = ["welcome", "chat"]

  [[category]]
  name = "Staff"
  private = true                       # invite only; members don't auto-join
  channels = ["mods"]

  [[channel]]                          # a channel with options
  name = "announcements"
  category = "General"
  read_only = true                     # only moderators (power 50) can post
  topic = "News"
  retention = "30d"                    # messages deleted after 30 days
                                       # (s, m, h, d, w, y; "forever" or omit =
                                       # keep forever)

  retention can also be set on a [[category]] (applies to its channels) or at
  the top level (applies to every channel without its own setting). Needs
  allow_room_retention = true on the server.

What it builds:
  - the server: an invite-only space; members join by invitation;
  - categories: sub-spaces, open to everyone in the server (unless private);
  - channels: encrypted rooms, open to everyone in their category or server;
  - adding channels, categories and settings needs power level 50 in the space.
With auto_join_space_children = true on the server, joining the server
joins all of its non-private categories and channels.
"""

import argparse
import getpass
import json
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import uuid


UNITS = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 7 * 86400, "y": 365 * 86400}


def parse_duration_ms(value):
    """'30d' -> milliseconds. None or 'forever' -> None (keep forever)."""
    if value is None or str(value).strip() == "forever":
        return None
    value = str(value).strip()
    if not value or value[-1] not in UNITS or not value[:-1].isdigit():
        sys.exit(f"bad retention value {value!r}: use e.g. 12h, 30d, 1y")
    return int(value[:-1]) * UNITS[value[-1]] * 1000


class Matrix:
    def __init__(self, base):
        self.base = base.rstrip("/")
        self.token = None
        self.server = None

    def call(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        if self.token:
            req.add_header("Authorization", "Bearer " + self.token)
        try:
            with urllib.request.urlopen(req, timeout=180) as resp:
                return json.loads(resp.read() or b"{}")
        except urllib.error.HTTPError as err:
            detail = err.read().decode(errors="replace")
            sys.exit(f"{method} {path} failed: {err.code} {detail}")

    def login(self, user, password):
        resp = self.call("POST", "/_matrix/client/v3/login", {
            "type": "m.login.password",
            "identifier": {"type": "m.id.user", "user": user},
            "password": password,
            "initial_device_display_name": "make-space.py",
        })
        self.token = resp["access_token"]
        self.server = resp["user_id"].split(":", 1)[1]

    def logout(self):
        if self.token:
            self.call("POST", "/_matrix/client/v3/logout", {})
            self.token = None

    def create(self, name, topic=None, space=False, parent=None, private=False,
               read_only=False, retention_ms=None):
        state = []
        if retention_ms is not None and not space:
            state.append({"type": "m.room.retention", "state_key": "",
                          "content": {"max_lifetime": retention_ms}})
        if not space:
            state.append({"type": "m.room.encryption", "state_key": "",
                          "content": {"algorithm": "m.megolm.v1.aes-sha2"}})
        if parent:
            state.append({"type": "m.space.parent", "state_key": parent,
                          "content": {"canonical": True, "via": [self.server]}})
        if parent and not private:
            join = {"join_rule": "restricted",
                    "allow": [{"type": "m.room_membership", "room_id": parent}]}
        else:
            join = {"join_rule": "invite"}
        state.append({"type": "m.room.join_rules", "state_key": "", "content": join})
        state.append({"type": "m.room.history_visibility", "state_key": "",
                      "content": {"history_visibility": "shared"}})

        body = {"name": name, "preset": "private_chat", "initial_state": state}
        if topic:
            body["topic"] = topic
        if space:
            body["creation_content"] = {"type": "m.space"}
            # Members can't post into the space itself; adding rooms needs 50.
            body["power_level_content_override"] = {"events_default": 50, "state_default": 50}
        elif read_only:
            body["power_level_content_override"] = {"events_default": 50}
        return self.call("POST", "/_matrix/client/v3/createRoom", body)["room_id"]

    def add_child(self, space, child, order):
        path = (f"/_matrix/client/v3/rooms/{urllib.parse.quote(space)}/state/"
                f"m.space.child/{urllib.parse.quote(child)}")
        self.call("PUT", path, {"via": [self.server], "order": f"{order:04d}"})


def load(path):
    with open(path, "rb") as f:
        t = tomllib.load(f)
    if "name" not in t:
        sys.exit("template needs a top-level name")
    default = t.get("retention")
    categories = {c["name"]: {"private": c.get("private", False), "topic": c.get("topic"),
                              "retention": c.get("retention", default),
                              "channels": [{"name": n} for n in c.get("channels", [])]}
                  for c in t.get("category", [])}
    top = [{"name": n} for n in t.get("channels", [])]
    for ch in t.get("channel", []):
        if ch.get("category") and ch["category"] not in categories:
            sys.exit(f"channel {ch['name']}: unknown category {ch['category']}")
        target = categories[ch["category"]]["channels"] if ch.get("category") else top
        target.append(ch)
    return t, top, categories


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--homeserver", default="http://127.0.0.1:8008")
    ap.add_argument("--user", required=True, help="localpart or full user ID")
    ap.add_argument("--password", help="omit to be asked (recommended)")
    ap.add_argument("--json", action="store_true", help="print name -> room ID as JSON at the end")
    ap.add_argument("template")
    args = ap.parse_args()

    t, top, categories = load(args.template)
    mx = Matrix(args.homeserver)
    mx.login(args.user, args.password or getpass.getpass(f"Password for {args.user}: "))
    made = {}
    try:
        order = 0
        server = mx.create(t["name"], t.get("topic"), space=True)
        made[t["name"]] = server
        print(f"server   {t['name']}: {server}")

        def channel(ch, parent, inherited):
            nonlocal order
            retention = ch.get("retention", inherited)
            room = mx.create(ch["name"], ch.get("topic"), parent=parent,
                             private=ch.get("private", False),
                             read_only=ch.get("read_only", False),
                             retention_ms=parse_duration_ms(retention))
            order += 1
            mx.add_child(parent, room, order)
            made[ch["name"]] = room
            shown = retention if retention not in (None, "forever") else None
            print(f"  channel  #{ch['name']}: {room}" + (f"  (retention {shown})" if shown else ""))

        for ch in top:
            channel(ch, server, t.get("retention"))
        for name, cat in categories.items():
            sub = mx.create(name, cat["topic"], space=True, parent=server, private=cat["private"])
            order += 1
            mx.add_child(server, sub, order)
            made[name] = sub
            print(f"category {name}{' (private)' if cat['private'] else ''}: {sub}")
            for ch in cat["channels"]:
                channel(ch, sub, cat["retention"])
        print("\nDone. Invite people to the server space; they join its channels automatically"
              " if the server has auto_join_space_children on.")
        if args.json:
            print(json.dumps(made))
    finally:
        mx.logout()


if __name__ == "__main__":
    main()
