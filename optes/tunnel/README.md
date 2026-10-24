# optes-tunnel

Relay (client side) and gateway (core side) for the phase 2 tunnel.
Protocol: `../phase2-protocol-spec.md`.

| Binary | Runs on | Does |
|---|---|---|
| `optes-relay` | your device, or an always-on machine at home (level 4) | Local HTTP endpoint for any Matrix client; carries every connection through one encrypted tunnel to a front. |
| `optes-gateway` | the core | Ends tunnels; forwards each stream to continuwuity's socket. Needs no network. |
| `optes-keys` | anywhere | Generates keys, signs and checks front lists. |

Build: `cargo build --release` (in this directory).

Relay example (Tor fronts, level 2):

```
optes-relay --socks 127.0.0.1:9050 --front-list frontlist.signed \
  --list-key list.pub --device-key device.key --level 2 \
  --state-dir ~/.local/state/optes-relay
```

Then set the Matrix client's homeserver to `http://127.0.0.1:8008`.

Tests: `../test/run-tunnel-e2e.sh <continuwuity-binary> target/debug <level>`.
