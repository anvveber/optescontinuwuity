# Fuzzing the tunnel

Targets: every place that parses input from the network or from files.

| Target | Checks |
|---|---|
| `cell_decode` | any bytes → `Cell::decode`: no panic; valid cells round-trip |
| `frontlist_verify` | any text → list verification: no panic; nothing unsigned accepted |
| `gateway_handshake` | any first message → gateway: rejected, no panic, **no reply** |
| `transport_open` | random frames in an established session: never decrypt; session stays usable |
| `session_cells` | any cell sequence through the stream table: no panic, bounded state |

Run (needs nightly + `cargo install cargo-fuzz`):

```
cp -r seeds corpus     # first time
cargo +nightly fuzz run <target> corpus/<target> -- -max_total_time=600
```
