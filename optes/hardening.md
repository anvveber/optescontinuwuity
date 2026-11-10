# Hardening options

All are set by `setup-core.sh`.

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

## Websites must not reach the server through a relay

The problem: users reach the server at `http://127.0.0.1:8008` (their relay).
A website open in the same browser can send requests there too. With the
Matrix default (`Access-Control-Allow-Origin: *`) it can also read the
answers: test user names, try passwords, and learn that this person uses this
server.

### `restrict_request_hosts = true`

The `Host` header must be an IP address, `localhost`, a `.onion` / `.i2p`
name, or a name in `allowed_hosts`. Anything else gets 403.

Stops DNS rebinding: a page on `evil.example` that re-points its domain at
127.0.0.1 still sends `Host: evil.example`. No list of relay addresses is
needed: a relay on a WireGuard address (level 4) is an IP address too.

### `allowed_origins = [...]`

Use it together with `restrict_request_hosts`. Alone it can be bypassed: with
DNS rebinding a website becomes "the server's own address".

A request that carries an `Origin` header (browsers add it to cross-site
requests and to every POST) is refused with 403 unless the origin is in the
list or is the server's own address (its own account pages). Only listed
origins get CORS headers. Preflights are refused the same way.

- Native clients (FluffyChat, Element X, nheko, bots) send no `Origin`: not
  affected.
- Clients built on web technology send their app's origin and need it listed.
  `setup-core.sh` lists `vector://vector` (Element Desktop) and the Tauri
  origins (`tauri://localhost`, `http://tauri.localhost`,
  `https://tauri.localhost`; Cinny desktop). **These values are not tested
  against the real apps.** A refused client gets "This origin may not use
  this server"; add its origin with `setup-core.sh allow-origin <origin>`.
- A web client opened in a normal browser (e.g. `https://app.cinny.in`) would
  need its website's origin listed. That trusts the website: avoid it.
- `Origin: null` (sandboxed pages, `file://`) is refused.

Not covered: a page can still send a plain GET without `Origin` (an image
tag). It can't read the answer and GET changes nothing; by timing it may still
notice that something listens on port 8008.

### OAuth off (`[global.oauth] compatibility_mode = "disabled"`)

With OAuth on, `/_matrix/client/v1/auth_metadata` answers without login and
contains the server name. Its URLs also point at `https://<server name>/`,
which is never served here, so OAuth login could not work anyway. Password
login is unaffected.

## `allow_pushers = false`

Phone clients register a push token (from Google, Apple or a UnifiedPush
server) so the homeserver can wake them. This server has no network and can
never use it, but the token would sit in the database and ties an account to
a phone. With the option off, a registration is answered with success and
nothing is stored. Clients must sync in the background to notify.

## `allow_openid_tokens = false`

An OpenID token proves a user's Matrix ID to a third-party service. Clients
request one for integration managers and widgets, then send it there over the
normal internet. With the option off the request is refused (403).

Tests: `optes/test/run-browser-guard-e2e.sh target/debug/conduwuit on|off`.
