//! Signed front list: which fronts exist and the gateway's Noise key.
//!
//! File format: the JSON bytes, a newline, `-----SIG-----`, a newline, the
//! base64 Ed25519 signature over the JSON bytes exactly as stored.

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::keys;

const SEPARATOR: &str = "\n-----SIG-----\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Front {
	/// "tor" or "i2p" (each through its own SOCKS proxy). Test builds
	/// (feature `test-fronts`) also know "tcp", a direct connection.
	pub transport: String,
	/// host:port
	pub address: String,
}

/// Whether `host` is `len` base32 characters followed by `suffix`.
fn is_base32_host(host: &str, len: usize, suffix: &str) -> bool {
	host.strip_suffix(suffix).is_some_and(|name| {
		name.len() == len && name.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'))
	})
}

impl Front {
	/// Whether this relay may dial the front.
	///
	/// A front is only ever reached through an anonymity network: `tor`
	/// fronts must be v3 onion addresses and `i2p` fronts `.b32.i2p`
	/// addresses. Anything else is refused, so a front list (even a
	/// correctly signed one) can't make the relay connect somewhere directly
	/// or through a Tor exit. Unknown transports are refused too; a list
	/// may carry them for newer relays.
	pub fn check(&self) -> Result<()> {
		let (host, port) = self
			.address
			.rsplit_once(':')
			.context("front address must be host:port")?;
		ensure!(port.parse::<u16>().is_ok_and(|p| p != 0), "front port is not valid: {port}");

		match self.transport.as_str() {
			| "tor" => ensure!(
				cfg!(feature = "test-fronts") || is_base32_host(host, 56, ".onion"),
				"tor front is not a v3 onion address: {host}"
			),
			| "i2p" => ensure!(
				cfg!(feature = "test-fronts") || is_base32_host(host, 52, ".b32.i2p"),
				"i2p front is not a .b32.i2p address: {host}"
			),
			| "tcp" if cfg!(feature = "test-fronts") => {},
			| other => bail!("unknown transport {other}"),
		}
		Ok(())
	}
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
	let list = verify_ignoring_expiry(signed, key, min_version)?;
	if list.expires_at <= now {
		bail!("front list expired at {}", list.expires_at);
	}
	Ok(list)
}

/// Like [`verify`] but accepts an expired list. Used only to reach the gateway
/// and fetch a fresh one: the gateway's identity is still checked by the Noise
/// handshake, so an expired list can't redirect a relay anywhere unsafe.
pub fn verify_ignoring_expiry(signed: &str, key: &VerifyingKey, min_version: u64) -> Result<FrontList> {
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

	fn front(transport: &str, address: &str) -> Front {
		Front { transport: transport.into(), address: address.into() }
	}

	#[test]
	fn accepts_onion_and_b32_fronts() {
		let onion = format!("{}.onion:443", "a".repeat(56));
		let b32 = format!("{}.b32.i2p:443", "b2".repeat(26));
		assert!(front("tor", &onion).check().is_ok());
		assert!(front("i2p", &b32).check().is_ok());
	}

	#[test]
	fn rejects_bad_ports_and_unknown_transports() {
		let onion = "a".repeat(56) + ".onion";
		assert!(front("tor", &onion).check().is_err(), "no port");
		assert!(front("tor", &format!("{onion}:0")).check().is_err());
		assert!(front("tor", &format!("{onion}:70000")).check().is_err());
		assert!(front("nym", &format!("{onion}:443")).check().is_err());
	}

	/// Release builds: nothing but onion / b32 addresses, and no direct TCP.
	#[cfg(not(feature = "test-fronts"))]
	#[test]
	fn rejects_anything_that_leaves_the_anonymity_network() {
		let onion = "a".repeat(56) + ".onion";
		assert!(front("tcp", "203.0.113.5:443").check().is_err(), "direct tcp");
		assert!(front("tcp", &format!("{onion}:443")).check().is_err(), "tcp transport");
		assert!(front("tor", "example.org:443").check().is_err(), "clearnet host via tor");
		assert!(front("tor", "203.0.113.5:443").check().is_err(), "ip via tor");
		assert!(front("tor", &format!("{}.onion:443", "a".repeat(16))).check().is_err(), "v2 onion");
		assert!(front("tor", &format!("{}.onion:443", "A".repeat(56))).check().is_err(), "upper case");
		assert!(front("tor", &format!("{onion}.example.org:443")).check().is_err(), "suffix trick");
		assert!(front("i2p", "example.i2p:443").check().is_err(), "named i2p host");
		assert!(front("i2p", &format!("{onion}:443")).check().is_err(), "onion as i2p");
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

	#[test]
	fn expired_still_checks_signature_and_version() {
		let key = keys::generate_ed25519();
		let signed = sign(&sample(5), &key).unwrap();
		assert!(verify_ignoring_expiry(&signed, &key.verifying_key(), 5).is_ok());
		assert!(verify_ignoring_expiry(&signed, &key.verifying_key(), 6).is_err());
		let other = keys::generate_ed25519();
		assert!(verify_ignoring_expiry(&signed, &other.verifying_key(), 0).is_err());
		let tampered = signed.replace("x.onion", "y.onion");
		assert!(verify_ignoring_expiry(&tampered, &key.verifying_key(), 0).is_err());
	}
}
