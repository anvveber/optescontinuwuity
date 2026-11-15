# optes — private Matrix server

This folder holds everything specific to this fork. The rest of the repository
is upstream continuwuity (v26.9.1) plus a small set of patches.

Start with `HANDOFF.md` (project state), then `handbook.md` (design, threat model, section 14 = current status).

| File | What |
|---|---|
| `HANDOFF.md` | State of the project, decisions, next steps |
| `handbook.md` | Design and threat model; §14 status map |
| `verification-notes.md` | Facts checked before building (config keys, versions) |
| `delayed-release-spec.md` | Server patch: release events to clients once per tick |
| `bouncer-decision.md` | Why there is no bouncer; `record_device_activity` |
| `phase2-protocol-spec.md` | Relay ↔ gateway tunnel protocol |
| `tunnel/` | Rust: `optes-relay`, `optes-gateway`, `optes-keys` |
| `setup-core.sh` | Core VPS (Debian 13) |
| `setup-front.sh` | Front VPS, Tor or I2P (Debian 13) |
| `setup-relay.sh` | Relay on your own machine (Fedora/Debian) |
| `make-kit.sh` | Build the release binaries and pack a deploy kit (`core/ front/ relay/ tools/`, README, checksums) |
| `test/` | End-to-end tests (no Tor needed) |
| `test/vm.py` | A throwaway Debian 13 VM (QEMU) for running the setup scripts; never run them on your own machine |
| `hardening.md` | `require_encryption`, `hide_server_identity`, the browser guard (`restrict_request_hosts`, `allowed_origins`), `allow_pushers`, `allow_openid_tokens`: what they do and don't cover |
| `audit-2026-11-10.md` | Privacy audit of the fork's own parts: findings and what was done about each |
| `spaces.md` | Discord-style servers: auto-join, who can create what, the template tool |
| `tools/make-space.py` | Create a server (space, categories, channels) from a TOML file |
| `update-upstream.sh` | Move the fork onto a new continuwuity release and run every test |
| `UPSTREAM` | The upstream tag the fork is based on |

Deployment order: core → front → `setup-core.sh add-front` → relay →
`setup-core.sh add-device` → point a Matrix client at `http://127.0.0.1:8008`.
