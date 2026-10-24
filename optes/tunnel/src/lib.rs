//! optes tunnel: fixed-size encrypted frames between a client-side relay and
//! a core-side gateway. See `optes/phase2-protocol-spec.md`.

pub mod cell;
pub mod frontlist;
pub mod keys;
pub mod noise;
pub mod policy;
pub mod session;

/// Size of every frame on the wire, handshake messages included.
pub const FRAME: usize = 2048;
/// AEAD tag size (ChaChaPoly).
pub const TAG: usize = 16;
/// Plaintext size of a transport frame.
pub const CELL: usize = FRAME - TAG;
/// Cell header: type (1) + stream id (4) + payload length (2).
pub const CELL_HEADER: usize = 7;
/// Largest payload one cell carries.
pub const MAX_PAYLOAD: usize = CELL - CELL_HEADER;

/// Noise protocol name and prologue.
pub const NOISE_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub const PROLOGUE: &[u8] = b"optes-tunnel-v1";
pub const PROTOCOL_VERSION: u8 = 1;
