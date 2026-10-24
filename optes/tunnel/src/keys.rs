//! Key files: one line, standard base64 of the raw 32-byte key.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{SigningKey, VerifyingKey};

use crate::NOISE_PATTERN;

pub fn encode(key: &[u8]) -> String { STANDARD.encode(key) }

pub fn decode32(text: &str) -> Result<[u8; 32]> {
	let bytes = STANDARD
		.decode(text.trim())
		.context("key is not valid base64")?;
	let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) else {
		bail!("key must be 32 bytes, got {}", bytes.len());
	};
	Ok(key)
}

pub fn read32(path: &Path) -> Result<[u8; 32]> {
	let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
	decode32(&text).with_context(|| format!("in {}", path.display()))
}

/// Reads a key given either inline as base64 or as a path to a key file.
pub fn read32_inline_or_file(value: &str) -> Result<[u8; 32]> {
	let path = Path::new(value);
	if path.exists() {
		read32(path)
	} else {
		decode32(value)
	}
}

/// Writes a key file with mode 0600.
pub fn write_secret(path: &Path, key: &[u8]) -> Result<()> {
	use std::{io::Write, os::unix::fs::OpenOptionsExt};

	let mut file = fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(path)
		.with_context(|| format!("creating {} (must not exist)", path.display()))?;
	writeln!(file, "{}", encode(key))?;
	Ok(())
}

pub fn write_public(path: &Path, key: &[u8]) -> Result<()> {
	fs::write(path, format!("{}\n", encode(key)))
		.with_context(|| format!("writing {}", path.display()))
}

/// New X25519 keypair for Noise: (private, public).
pub fn generate_x25519() -> Result<([u8; 32], [u8; 32])> {
	let pair = snow::Builder::new(NOISE_PATTERN.parse()?).generate_keypair()?;
	Ok((pair.private.as_slice().try_into()?, pair.public.as_slice().try_into()?))
}

/// New Ed25519 keypair for signing front lists.
#[must_use]
pub fn generate_ed25519() -> SigningKey { SigningKey::generate(&mut rand::rngs::OsRng) }

pub fn verifying_key(bytes: &[u8; 32]) -> Result<VerifyingKey> {
	VerifyingKey::from_bytes(bytes).context("invalid Ed25519 public key")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn base64_roundtrip() {
		let key = [7_u8; 32];
		assert_eq!(decode32(&encode(&key)).unwrap(), key);
	}

	#[test]
	fn rejects_wrong_length() {
		assert!(decode32(&encode(&[1_u8; 31])).is_err());
	}
}
