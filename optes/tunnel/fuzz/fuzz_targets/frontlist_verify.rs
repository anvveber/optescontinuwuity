//! Arbitrary signed-list text: verification never panics and never accepts
//! anything not signed by the pinned key.
#![no_main]
use libfuzzer_sys::fuzz_target;
use optes_tunnel::frontlist;

fuzz_target!(|data: &[u8]| {
	let text = String::from_utf8_lossy(data);
	// A fixed key the fuzzer can't sign for.
	let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key();
	assert!(frontlist::verify(&text, &key, 0, 0).is_err());
	assert!(frontlist::verify_ignoring_expiry(&text, &key, 0).is_err());
	let _ = frontlist::peek_version(&text);
});
