# optes — private Matrix server

This folder holds everything specific to this fork. The rest of the repository
is upstream continuwuity (v26.9.1) plus two small patches.

Start with `handbook.md` (design, threat model, section 14 = current status).

| File | What |
|---|---|
| `handbook.md` | Design and threat model; §14 status map |
| `verification-notes.md` | Facts checked before building (config keys, versions) |
| `delayed-release-spec.md` | Server patch: release events to clients once per tick |
| `bouncer-decision.md` | Why there is no bouncer; `record_device_activity` |
| `phase2-protocol-spec.md` | Relay ↔ gateway tunnel protocol |
| `tunnel/` | Rust: `optes-relay`, `optes-gateway`, `optes-keys` |
| `setup-core.sh` | Core VPS (Debian 13) |
| `setup-front.sh` | Front VPS, Tor or I2P (Debian 13) |
| `setup-relay.sh` | Relay on your own machine (Fedora/Debian) |
| `test/` | End-to-end tests (no Tor needed) |

Deployment order: core → front → `setup-core.sh add-front` → relay →
`setup-core.sh add-device` → point a Matrix client at `http://127.0.0.1:8008`.
