# Hardening options

Both are set by `setup-core.sh`.

## `require_encryption = true`

- Every new room (except spaces, which carry no messages) starts encrypted,
  even if the client didn't ask.
- The server refuses unencrypted messages, stickers, polls and live location.
- Still accepted unencrypted, because clients send them that way by design:
  **reactions** (the emoji itself is visible to the server) and
  **redactions**.
- The admin room is exempt, so `!admin` commands keep working.

Not covered (Matrix limitation): room names, topics, avatars, membership and
other state events are never end-to-end encrypted.

Effect on bots and tools that can't encrypt: they can't post messages.

## `hide_server_identity = true`

- Removes `/_continuwuity/server_version`, the landing page `/`, and the about
  and debug pages (the debug page also had an unauthenticated endpoint that
  triggers an internal panic).
- Generic session cookie name.

Still visible to anyone who can reach the server: the account and login pages
under `/_continuwuity/` (clients need them for login), the list of supported
features in `/_matrix/client/versions`, and error message wording. A
determined client can still tell it's continuwuity; this removes the easy
version lookup.

Tests: `optes/test/run-hardening-e2e.sh target/debug/conduwuit on|off`.
