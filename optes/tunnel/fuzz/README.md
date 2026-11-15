# Fuzzing the tunnel

Targets: every place that parses input from the network or from files.

| Target | Checks |
|---|---|
| `cell_decode` | any bytes → `Cell::decode`: no panic; valid cells round-trip |
| `frontlist_verify` | any text → list verification: no panic; nothing unsigned accepted |
| `gateway_handshake` | any first message → gateway: rejected, no panic, **no reply** |
| `transport_open` | random frames in an established session: never decrypt; session stays usable |
| `session_cells` | any cell sequence through the stream table: no panic, bounded state |
| `hello_counter` | real handshakes with any hello (version, level, list version, counter) from three devices, through the gateway's replay check, with the stored counter files overwritten by any bytes in between: the gateway answers exactly when the version is right and the counter is higher than the stored one; a refused handshake gets no reply and leaves the stored counter alone; a file that is not a number refuses every handshake |

Run (needs `cargo install cargo-fuzz`, and a nightly toolchain or
`RUSTC_BOOTSTRAP=1` with the pinned one):

```
cp -r seeds corpus     # first time
cargo +nightly fuzz run <target> corpus/<target> -- -max_total_time=600
# or, with the pinned stable toolchain:
RUSTC_BOOTSTRAP=1 cargo fuzz run <target> corpus/<target> -- -max_total_time=600
```

## Results

2026-11-15, protocol v2, 15 minutes per target, all six at once on a machine
with 4+ cores (stable toolchain with `RUSTC_BOOTSTRAP=1`, AddressSanitizer on):

| Target | Runs |
|---|---|
| `cell_decode` | 121.3M |
| `frontlist_verify` | 3.0M |
| `gateway_handshake` | 759k |
| `transport_open` | 112k |
| `session_cells` | 10.9M |
| `hello_counter` | 104k |

No crashes, hangs or failed assertions. `hello_counter` was checked once
against a broken replay rule (`<` in place of `<=`): it failed within a
minute.

2026-10-25, protocol v1, 8 minutes per target on 2 cores: cell_decode 15.2M
runs, frontlist_verify 1.0M, gateway_handshake 281k, session_cells 2.8M,
transport_open 47k. No crashes, hangs or failed assertions.

Limit: random input can't produce a valid Noise handshake, so
`gateway_handshake` reaches only the decryption of message 1. The hello
decoding and the replay check after it are covered by `hello_counter`, which
encrypts the hello itself. Not covered by fuzzing: the gateway's check that a
device key is listed (a directory lookup); what the gateway does with the
hello's level (`hello_counter` varies it but checks nothing about it; the code
maps unknown levels to the strictest one, see the unit tests); the relay's own
counter file.
