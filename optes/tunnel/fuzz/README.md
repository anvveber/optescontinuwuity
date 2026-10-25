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

## Results

2026-10-25, 8 minutes per target on 2 cores: cell_decode 15.2M runs,
frontlist_verify 1.0M, gateway_handshake 281k, session_cells 2.8M,
transport_open 47k. No crashes, hangs or failed assertions.

Limit: random input can't produce a valid Noise handshake, so the gateway's
parsing *after* decryption (the 18-byte hello) is only reached by unit tests,
not by fuzzing. That code reads three fixed fields and maps unknown levels to
the strictest one.
