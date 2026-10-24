# Verification notes — handbook section 10

Checked 2026-10-24. Scope: closed federation, onion server name, no Nym.
Applies to the Continuwuity `main` config reference (updated Aug 2026). Re-check
against the example config of the exact tag we pin.

## Decisions recorded

- A: server name = an onion address. Key kept offline only; not served.
- B: federation off.
- C: Claude writes the code for now.
- D: Nym dropped for now.

## Continuwuity

- **Repo:** `forgejo.ellis.link/continuwuation/continuwuity` (mirror: `github.com/continuwuity/continuwuity`).
- **Versions:** calendar versioning since mid-2026 (`v0.5.10` → `v26.6.x`). `v26.9.1` existed by mid-Sept 2026. Pin the newest tag at build time.
- **Security:** recent critical fixes (CVE-2025-68667, CVE-2026-24471). Tracking upstream is mandatory.
- **Default port is 8008, not 6167.** Better: `unix_socket_path` (can't be combined with `address`). Works if gateway and service share a host via namespaces; separate VMs need TCP on an internal link.

### Section 6 settings → actual keys

| Goal | Key | Default | Set to |
|---|---|---|---|
| No federation | `allow_federation` | `true` | `false` |
| No key notaries | `trusted_servers` | `["matrix.org"]` | `[]` |
| **No phone-home** (not in handbook) | `allow_announcements_check` | **`true`** | `false` |
| Block all remote servers (belt-and-braces) | `forbidden_remote_server_names` | `[]` | `[".*"]` |
| URL previews off | `url_preview_*_allowlist` | `[]` (off) | leave empty |
| Presence off | `allow_local_presence` / `allow_incoming_presence` / `allow_outgoing_presence` | `true` / `true` / `false` | all `false` |
| Read receipts off server-wide | `allow_local_read_receipts` | `true` | `false` |
| Typing off server-wide | `allow_local_typing` | `true` | `false` |
| Device names not shared | `allow_device_name_federation` | `false` | leave |
| Token-only registration | `allow_registration` + `registration_token_file` | — | `true` + file |
| No email capture | `[global.smtp]` | absent | leave absent |
| No profile scraping | `require_auth_for_profile_requests` | `false` | `true` |
| Room directory locked | `lockdown_public_room_directory` | `false` | `true` |
| Logging | `log` | `"info"` | `"warn"` |
| Telemetry off | `sentry`, `allow_otlp`, `allow_prometheus_metrics` | all `false` | leave |
| No TURN / calls | `turn_uris`, `[global.matrix_rtc] foci` | `[]` | leave |
| Outbound via Tor | `[global.proxy] global = { url = "socks5h://…" }` | `"none"` | set, but enforce by network isolation |
| Cosmetic | `new_user_displayname_suffix` | flag emoji | `""` |

Note: turning read receipts and typing off server-side means a misconfigured client can't leak them. Cost: nobody gets them.

### Media retention — no config key, no patch needed

`!admin media delete-past-remote-media -b <duration> --yes-i-want-to-delete-local-media`
deletes media older than the duration. Run it on a schedule with
`admin_signal_execute` + a systemd timer sending SIGUSR2. To test: exact command
syntax inside `admin_signal_execute`.

### Patch list, revised

| Patch | Status |
|---|---|
| 1. Delayed release | **Still needed.** No upstream option found. |
| 2. Strip device IP storage | **Likely drop.** Core only accepts connections from local Tor, so any stored IP is loopback. Confirm what is stored in phase 1a. |
| 3. Compile out URL preview | **Likely drop.** Off by default. Confirm no other fetch path. |
| 4. Onion-peer TLS | **Drop.** No federation. |
| 5. Fail-closed proxy in code | **Optional.** Network isolation (service VM has no route out) does the job. |

## Tor / Arti

- Arti (Jan 2026 FAQ): usable as client and for embedding; supports **accessing and hosting** onion services; no relay mode. Latest seen: 2.6.0 (Sept 2026).
- Arti 2.5.1: onion services can target unix sockets.
- **Not verified:** onion client authorization through Arti's embedding API. Needed for phase 3. Core and fronts use C tor, so phase 1 is unaffected.

## matrix-rust-sdk

- `ClientBuilder::proxy()` accepts HTTP proxies only.
- `ClientBuilder::http_client()` accepts a custom `reqwest::Client` (overrides proxy / user agent / TLS settings).
- Phase 3 path: Arti inside the app as a local SOCKS proxy → reqwest with `socks5h` → `http_client()`. Raw Noise tunnel needs more work; design in phase 2.

## Still unverified

- i2pd SAM specifics and hidden/firewalled config (phase 2).
- Bouncer design that drives sync without holding room keys (phase 1c).
- Which unmodified clients support SOCKS + onion client auth for phase 1a testing (Orbot on GrapheneOS handles client auth; check Element X / FluffyChat behaviour through it).
- AMD SEV-SNP VPS providers with anonymous payment (decision E).

## Sources

- Continuwuity config reference: https://continuwuity.org/reference/config.html
- Continuwuity media admin commands: https://continuwuity.org/reference/admin/media.html
- Version history (Renovate PR): https://github.com/JJGadgets/Biohazard/pull/6457
- This Week in Matrix 2026-03-06: https://matrix.org/blog/2026/03/06/this-week-in-matrix-2026-03-06/
- Arti FAQ: https://arti.torproject.org/FAQs
- Arti 2.5.1: https://forum.torproject.org/t/arti-2-5-1-released/21947
- Arti 2.6.0: https://blog.torproject.org/arti_2_6_0_released/
- matrix-rust-sdk ClientBuilder: https://docs.rs/matrix-sdk/latest/matrix_sdk/struct.ClientBuilder.html
