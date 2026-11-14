# Discord-style servers

Matrix *spaces* are the equivalent of Discord servers; sub-spaces work as
categories. This fork adds three config keys (all set by `setup-core.sh`):

| Key | Effect |
|---|---|
| `auto_join_space_children = true` | Joining a space joins all its channels and categories you're allowed into. New channels added later are joined automatically. Leaving the space leaves them. Invite-only ("private") channels are skipped. |
| `allow_space_creation = false` | Only server admins create top-level spaces ("servers"). |
| `restrict_room_creation_to_spaces = true` | Non-admins create rooms only inside a space where they have the power to add rooms (power 50 by default = moderator), or DMs. |

**Which rooms count as channels of a server.** A room listed in a space
(`m.space.child`) is treated as its channel only if the room names the space
in return: an `m.space.parent` event for the space, or a restricted join rule
that admits the space's members. Listing a room needs power in the space only;
the two things above need power in the room. Without this rule a moderator of
a space could list any room there, then remove a member from the space, and
the server would take that member out of the room as well.

- Rooms made with `make-space.py` have both. A client that creates a room
  "inside a space" has to send one of them anyway, or the server refuses the
  creation for non-admins.
- A room that existed before and is added to a server with a client's "add
  existing room" is not joined automatically, and not left automatically,
  unless it names the space. Members can still join it by hand.

So: admins make servers; moderators of a server add channels and categories
to it; everyone can DM. Clients that create rooms "inside a space" (Cinny,
Element) send what the server needs; plain "create room" outside a space is
refused for non-admins.

## Making a server from a template

```
optes/tools/make-space.py --user <admin> optes/tools/example-server.toml
```

Point `--homeserver` at the relay (default `http://127.0.0.1:8008`). It asks
for the password, creates the space, categories and encrypted channels, and
logs out again. Then invite people to the space; they get all channels.

Template format: see the comment at the top of `tools/make-space.py` and
`tools/example-server.toml`.

Retention: `retention = "30d"` on a channel, a category (its channels
inherit) or the top level; `"forever"` overrides an inherited value. Expired
messages are hidden at once and deleted within 15 minutes. Note: media
(images/files) follow the separate server-wide 30-day rule, also in permanent
channels.

## Making someone a moderator

In the space's settings (Cinny: space → settings → permissions), set their
power level to 50. They can then add channels and categories to that space.

## Known limits

- Discord-style roles don't exist; Matrix has numeric power levels per room.
- Leaving one channel while staying in the server works: you stay out of that
  channel until you rejoin it. Leaving and rejoining the whole server joins all
  channels again.
- A channel that belongs to two servers is left when the user leaves one of
  them, even if they are still in the other.
- `is_direct` is trusted: a non-admin could create a "DM" and invite many
  people. Cheap to abuse only in the sense of creating group chats.

Tests: `optes/test/run-spaces-e2e.sh target/debug/conduwuit`.
