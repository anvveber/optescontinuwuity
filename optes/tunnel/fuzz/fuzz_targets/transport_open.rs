//! After a real handshake, arbitrary frames into the transport: tampered or
//! random frames must fail to open; never panic. One genuine frame is mixed
//! in to check the session stays usable after rejecting garbage.
#![no_main]
use libfuzzer_sys::fuzz_target;
use optes_tunnel::{cell::Cell, keys, noise, FRAME};

fuzz_target!(|data: &[u8]| {
	let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
	rt.block_on(async {
		let (gw_priv, gw_pub) = keys::generate_x25519().unwrap();
		let (dev_priv, _) = keys::generate_x25519().unwrap();
		let (mut a, mut b) = tokio::io::duplex(4 * FRAME);
		let server = noise::respond(&mut b, &gw_priv, |_| true, |h| noise::Welcome {
			level: h.level,
			tick_ms: 200,
			window_secs: 15,
			list_version: 0,
		});
		let client = noise::initiate(&mut a, &dev_priv, &gw_pub, noise::Hello { level: 1, list_version: 0 });
		let (s, c) = tokio::join!(server, client);
		let (mut server_crypto, _, _) = s.unwrap();
		let (mut client_crypto, _) = c.unwrap();

		for chunk in data.chunks(FRAME) {
			let mut frame = [0u8; FRAME];
			frame[..chunk.len()].copy_from_slice(chunk);
			assert!(server_crypto.open(&frame).is_err(), "random frame decrypted");
		}

		// Garbage must not desynchronise the session.
		let cell = Cell::data(1, b"still fine".to_vec());
		let frame = client_crypto.seal(&cell).unwrap();
		assert_eq!(server_crypto.open(&frame).unwrap(), cell);
	});
});
