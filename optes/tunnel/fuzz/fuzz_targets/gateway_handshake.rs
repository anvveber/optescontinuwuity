//! Arbitrary bytes as handshake message 1 to the gateway: must be rejected
//! without panic, and the gateway must write nothing back.
#![no_main]
use libfuzzer_sys::fuzz_target;
use optes_tunnel::{noise, FRAME};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const GATEWAY_PRIV: [u8; 32] = [42u8; 32];

fuzz_target!(|data: &[u8]| {
	let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
	rt.block_on(async {
		let (mut client, mut server) = tokio::io::duplex(4 * FRAME);
		let mut msg = data.to_vec();
		msg.resize(FRAME, 0);
		client.write_all(&msg).await.unwrap();

		let result = noise::respond(&mut server, &GATEWAY_PRIV, |_| true, |h| noise::Welcome {
			level: h.level,
			tick_ms: 200,
			window_secs: 15,
			list_version: 0,
		})
		.await;
		drop(server);

		let mut reply = Vec::new();
		let _ = client.read_to_end(&mut reply).await;
		if result.is_err() {
			assert!(reply.is_empty(), "gateway answered a rejected handshake");
		}
	});
});
