//! Signed front list: which fronts exist and the gateway's Noise key.
//!
//! File format: the JSON bytes, a newline, `-----SIG-----`, a newline, the
//! base64 Ed25519 signature over the JSON bytes exactly as stored.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::keys;

const SEPARATOR: &str = "\n-----SIG-----\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Front {
	/// "tor" (reached through the SOCKS proxy) or "tcp" (direct; tests only).
	pub transport: String,
	/// host:port
	pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontList {
	pub version: u64,
	pub issued_at: u64,
	pub expires_at: u64,
	/// Base64 X25519 public key of the gateway.
	pub gateway_key: String,
	pub fronts: Vec<Front>,
}

impl FrontList {
	pub fn gateway_key(&self) -> Result<[u8; 32]> { keys::decode32(&self.gateway_key) }
}

/// Serializes and signs. Returns the signed file contents.
pub fn sign(list: &FrontList, key: &SigningKey) -> Result<String> {
	let json = serde_json::to_string_pretty(list)?;
	let sig = key.sign(json.as_bytes());
	Ok(format!("{json}{SEPARATOR}{}\n", STANDARD.encode(sig.to_bytes())))
}

/// Reads the version field without checking the signature. Only for the
/// gateway, which reads its own file to announce the version.
pub fn peek_version(signed: &str) -> Result<u64> {
	let (json, _) = signed.split_once(SEPARATOR).context("missing signature separator")?;
	Ok(serde_json::from_str::<FrontList>(json)?.version)
}

/// Verifies a signed list.
///
/// - signature must match `key`;
/// - `version` must be >= `min_version` (rollback protection);
/// - `expires_at` must be after `now` (seconds since the Unix epoch).
pub fn verify(signed: &str, key: &VerifyingKey, min_version: u64, now: u64) -> Result<FrontList> {
	let (json, sig) = signed
		.split_once(SEPARATOR)
		.context("missing signature separator")?;
	let sig_bytes = STANDARD
		.decode(sig.trim())
		.context("signature is not valid base64")?;
	let sig = Signature::from_slice(&sig_bytes).context("malformed signature")?;
	key.verify(json.as_bytes(), &sig)
		.context("front list signature does not verify")?;

	let list: FrontList = serde_json::from_str(json).context("front list JSON")?;
	if list.version < min_version {
		bail!("front list version {} is older than accepted version {min_version}", list.version);
	}
	if list.expires_at <= now {
		bail!("front list expired at {}", list.expires_at);
	}
	list.gateway_key().context("gateway_key")?;
	if list.fronts.is_empty() {
		bail!("front list has no fronts");
	}

	Ok(list)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn sample(version: u64) -> FrontList {
		FrontList {
			version,
			issued_at: 1_000,
			expires_at: 2_000,
			gateway_key: keys::encode(&[3_u8; 32]),
			fronts: vec![Front { transport: "tor".into(), address: "x.onion:443".into() }],
		}
	}

	#[test]
	fn sign_and_verify() {
		let key = keys::generate_ed25519();
		let signed = sign(&sample(5), &key).unwrap();
		let list = verify(&signed, &key.verifying_key(), 0, 1_500).unwrap();
		assert_eq!(list, sample(5));
		assert_eq!(peek_version(&signed).unwrap(), 5);
	}

	#[test]
	fn rejects_tampering() {
		let key = keys::generate_ed25519();
		let signed = sign(&sample(5), &key).unwrap().replace("x.onion", "y.onion");
		assert!(verify(&signed, &key.verifying_key(), 0, 1_500).is_err());
	}

	#[test]
	fn rejects_other_key() {
		let signed = sign(&sample(5), &keys::generate_ed25519()).unwrap();
		let other = keys::generate_ed25519();
		assert!(verify(&signed, &other.verifying_key(), 0, 1_500).is_err());
	}

	#[test]
	fn rejects_rollback() {
		let key = keys::generate_ed25519();
		let signed = sign(&sample(5), &key).unwrap();
		assert!(verify(&signed, &key.verifying_key(), 6, 1_500).is_err());
		assert!(verify(&signed, &key.verifying_key(), 5, 1_500).is_ok());
	}

	#[test]
	fn rejects_expired() {
		let key = keys::generate_ed25519();
		let signed = sign(&sample(5), &key).unwrap();
		assert!(verify(&signed, &key.verifying_key(), 0, 2_000).is_err());
	}
}
