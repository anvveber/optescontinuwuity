#!/usr/bin/env python3
"""Browser guard (Host / Origin), pushers, OpenID tokens and OAuth metadata.

Usage: browser_guard_e2e.py <host:port> <registration_token> <first_run_token> <on|off>
  on  = restrict_request_hosts, allowed_origins, allow_pushers=false,
        allow_openid_tokens=false, oauth disabled
  off = defaults (checks upstream behaviour)

Uses http.client so the Host and Origin headers are exactly what the test says.
"""

import http.client
import json
import sys
import uuid

ADDR, TOKEN, FIRST, MODE = sys.argv[1:5]
ON = MODE == "on"
FAILURES = []
SERVER_NAME = "guard.test"
EVIL = "https://evil.example"


def call(method, path, body=None, token=None, host=None, origin=None, extra=None):
    """Returns (status, headers, json-or-{}, raw text)."""
    conn = http.client.HTTPConnection(ADDR, timeout=60)
    headers = {"Host": host or ADDR}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = "Bearer " + token
    if origin is not None:
        headers["Origin"] = origin
    headers.update(extra or {})
    conn.request(method, path, body=data, headers=headers)
    resp = conn.getresponse()
    raw = resp.read().decode(errors="replace")
    parsed = json.loads(raw) if raw.startswith("{") else {}
    return resp.status, {k.lower(): v for k, v in resp.getheaders()}, parsed, raw


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""))
    if not ok:
        FAILURES.append(name)


def register(name, token):
    body = {"username": name, "password": "pw-" + uuid.uuid4().hex}
    status, _, resp, _ = call("POST", "/_matrix/client/v3/register", body)
    if status == 401:
        body["auth"] = {"type": "m.login.registration_token", "token": token,
                        "session": resp.get("session")}
        status, _, resp, _ = call("POST", "/_matrix/client/v3/register", body)
    if status != 200:
        sys.exit(f"register {name}: {status} {resp}")
    return resp["access_token"], resp["user_id"]


VERSIONS = "/_matrix/client/versions"
ACAO = "access-control-allow-origin"

# --- Host ------------------------------------------------------------------
status, _, _, _ = call("GET", VERSIONS)
check("request with Host = the address used: served", status == 200, str(status))
for host in ("localhost:8008", "[::1]:8008", "10.8.0.1:8008",
             "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion",
             "matrix.listed.example"):
    status, _, _, _ = call("GET", VERSIONS, host=host)
    check(f"Host {host}: served", status == 200, str(status))
for host in ("evil.example:8008", "127.0.0.1.evil.example", "localhost.evil.example:8008"):
    status, hdrs, _, _ = call("GET", VERSIONS, host=host)
    if ON:
        check(f"Host {host} (DNS rebinding): refused", status == 403 and ACAO not in hdrs,
              f"{status} {hdrs.get(ACAO)}")
    else:
        check(f"(off) Host {host}: served", status == 200, str(status))

# --- Origin ----------------------------------------------------------------
status, hdrs, _, _ = call("GET", VERSIONS, origin=EVIL)
if ON:
    check("website origin: refused, no CORS header", status == 403 and ACAO not in hdrs,
          f"{status} {hdrs.get(ACAO)}")
else:
    check("(off) website origin: served with CORS *", status == 200 and hdrs.get(ACAO) == "*",
          f"{status} {hdrs.get(ACAO)}")

status, hdrs, _, _ = call("GET", VERSIONS, origin="null")
if ON:
    check("Origin null (sandboxed page, file://): refused", status == 403 and ACAO not in hdrs,
          f"{status} {hdrs.get(ACAO)}")

for origin in ("vector://vector", "https://client.listed.example"):
    status, hdrs, _, _ = call("GET", VERSIONS, origin=origin)
    want = origin if ON else "*"
    check(f"listed origin {origin}: served, CORS header {want}",
          status == 200 and hdrs.get(ACAO) == want, f"{status} {hdrs.get(ACAO)}")

status, hdrs, _, _ = call("GET", VERSIONS, origin="http://" + ADDR)
check("same-origin request (the server's own pages): served", status == 200, str(status))
if ON:
    status, _, _, _ = call("GET", VERSIONS, origin="http://" + ADDR + ".evil.example")
    check("origin that only starts like the server's address: refused", status == 403, str(status))
    status, _, _, _ = call("GET", VERSIONS, origin="https://client.listed.example.evil.example")
    check("origin that only starts like a listed one: refused", status == 403, str(status))

# Preflight, as a browser sends before a cross-origin POST with JSON.
pre = {"Access-Control-Request-Method": "POST",
       "Access-Control-Request-Headers": "authorization,content-type"}
status, hdrs, _, _ = call("OPTIONS", "/_matrix/client/v3/login", origin=EVIL, extra=pre)
if ON:
    check("preflight from a website: refused, no CORS header", status == 403 and ACAO not in hdrs,
          f"{status} {hdrs.get(ACAO)}")
else:
    check("(off) preflight from a website: allowed", status == 200 and hdrs.get(ACAO) == "*",
          f"{status} {hdrs.get(ACAO)}")
status, hdrs, _, _ = call("OPTIONS", "/_matrix/client/v3/login", origin="vector://vector", extra=pre)
check("preflight from a listed origin: allowed",
      status == 200 and hdrs.get(ACAO) == ("vector://vector" if ON else "*"),
      f"{status} {hdrs.get(ACAO)}")

# Blind writes: a page can send a POST without reading the answer.
status, _, _, _ = call("POST", "/_matrix/client/v3/login",
                       {"type": "m.login.password", "identifier": {"type": "m.id.user", "user": "x"},
                        "password": "guess"}, origin=EVIL)
if ON:
    check("login attempt from a website: refused before it reaches login", status == 403, str(status))

# --- OAuth metadata must not publish the server name -----------------------
status, _, _, raw = call("GET", "/_matrix/client/v1/auth_metadata")
if ON:
    check("auth_metadata: not served, no server name", status != 200 and SERVER_NAME not in raw,
          f"{status}")
else:
    check("(off) auth_metadata: served and names the server", status == 200 and SERVER_NAME in raw,
          f"{status}")

# --- pushers and OpenID tokens ---------------------------------------------
sfx = uuid.uuid4().hex[:6]
user, user_id = register("u" + sfx, FIRST)

pusher = {"app_display_name": "t", "app_id": "test.app", "pushkey": "PUSHKEY-" + sfx,
          "kind": "http", "lang": "en", "device_display_name": "d",
          "data": {"url": "https://push.example/_matrix/push/v1/notify"}}
status, _, resp, _ = call("POST", "/_matrix/client/v3/pushers/set", pusher, user)
check("pusher registration answered with success", status == 200, f"{status} {resp}")
status, _, resp, raw = call("GET", "/_matrix/client/v3/pushers", token=user)
stored = "PUSHKEY-" + sfx in raw
if ON:
    check("push token not stored", status == 200 and not stored, raw[:120])
else:
    check("(off) push token stored", status == 200 and stored, raw[:120])

status, _, resp, _ = call("POST", f"/_matrix/client/v3/user/{user_id}/openid/request_token", {}, user)
if ON:
    check("OpenID token refused", status == 403, f"{status} {resp}")
else:
    check("(off) OpenID token issued", status == 200 and "access_token" in resp, str(status))

# The normal API still works for a logged-in native client (no Origin).
status, _, resp, _ = call("GET", "/_matrix/client/v3/account/whoami", token=user)
check("logged-in client without Origin: works", status == 200 and resp.get("user_id") == user_id, str(status))
status, _, resp, _ = call("GET", "/_matrix/client/v3/login")
check("password login still offered",
      status == 200 and any(f.get("type") == "m.login.password" for f in resp.get("flows", [])), str(status))

print("\n" + ("ALL PASSED" if not FAILURES else f"{len(FAILURES)} FAILED: {FAILURES}"))
sys.exit(1 if FAILURES else 0)
