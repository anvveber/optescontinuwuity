//! Cell encoding: the 2032-byte plaintext inside every transport frame.

use anyhow::{bail, Result};

use crate::{CELL, CELL_HEADER, MAX_PAYLOAD};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
	Pad = 0,
	Open = 1,
	Data = 2,
	Close = 3,
	ListGet = 4,
	ListPart = 5,
}

impl Kind {
	fn from_u8(byte: u8) -> Result<Self> {
		Ok(match byte {
			| 0 => Self::Pad,
			| 1 => Self::Open,
			| 2 => Self::Data,
			| 3 => Self::Close,
			| 4 => Self::ListGet,
			| 5 => Self::ListPart,
			| other => bail!("unknown cell type {other}"),
		})
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
	pub kind: Kind,
	pub stream: u32,
	pub payload: Vec<u8>,
}

impl Cell {
	#[must_use]
	pub fn pad() -> Self { Self::control(Kind::Pad, 0) }

	#[must_use]
	pub fn control(kind: Kind, stream: u32) -> Self {
		Self { kind, stream, payload: Vec::new() }
	}

	/// # Panics
	/// If `payload` is longer than [`MAX_PAYLOAD`].
	#[must_use]
	pub fn data(stream: u32, payload: Vec<u8>) -> Self {
		assert!(payload.len() <= MAX_PAYLOAD, "payload too long");
		Self { kind: Kind::Data, stream, payload }
	}

	#[must_use]
	pub fn is_pad(&self) -> bool { self.kind == Kind::Pad }

	/// Encodes to exactly [`CELL`] bytes. Unused space is zero.
	#[must_use]
	pub fn encode(&self) -> [u8; CELL] {
		let mut out = [0_u8; CELL];
		out[0] = self.kind as u8;
		out[1..5].copy_from_slice(&self.stream.to_be_bytes());
		let len = u16::try_from(self.payload.len()).expect("payload length checked at construction");
		out[5..7].copy_from_slice(&len.to_be_bytes());
		out[CELL_HEADER..CELL_HEADER + self.payload.len()].copy_from_slice(&self.payload);
		out
	}

	pub fn decode(buf: &[u8]) -> Result<Self> {
		if buf.len() != CELL {
			bail!("cell must be {CELL} bytes, got {}", buf.len());
		}
		let kind = Kind::from_u8(buf[0])?;
		let stream = u32::from_be_bytes(buf[1..5].try_into()?);
		let len = usize::from(u16::from_be_bytes(buf[5..7].try_into()?));
		if len > MAX_PAYLOAD {
			bail!("payload length {len} exceeds {MAX_PAYLOAD}");
		}
		Ok(Self { kind, stream, payload: buf[CELL_HEADER..CELL_HEADER + len].to_vec() })
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn roundtrip() {
		let cell = Cell::data(7, b"hello".to_vec());
		let enc = cell.encode();
		assert_eq!(enc.len(), CELL);
		assert_eq!(Cell::decode(&enc).unwrap(), cell);
	}

	#[test]
	fn max_payload_fits() {
		let cell = Cell::data(1, vec![0xAB; MAX_PAYLOAD]);
		assert_eq!(Cell::decode(&cell.encode()).unwrap(), cell);
	}

	#[test]
	fn rejects_bad_length() {
		let mut enc = Cell::pad().encode();
		enc[5..7].copy_from_slice(&u16::MAX.to_be_bytes());
		assert!(Cell::decode(&enc).is_err());
	}

	#[test]
	fn rejects_unknown_type() {
		let mut enc = Cell::pad().encode();
		enc[0] = 99;
		assert!(Cell::decode(&enc).is_err());
	}
}
