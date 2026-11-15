//! Real handshakes with fuzzer-chosen hello contents (version, level, list
//! version, counter) from three devices, through the gateway's replay check
//! (`replay::Replay`). Between handshakes the fuzzer may overwrite a device's
//! stored counter file with any bytes.
//!
//! Random bytes never get through Noise, so `gateway_handshake` can't reach
//! the hello decoding or the counter check. This target encrypts the hello
//! itself and checks, for every handshake:
//! - no panic;
//! - the gateway answers exactly when the hello has the right version and its
//!   counter is higher than the stored one (a file that is not a number
//!   refuses every handshake from that device);
//! - a refused handshake gets no reply and leaves the stored file as it was;
//! - an answered one stores its counter, and the reply decrypts.
#![no_main]
use std::{fs, path::PathBuf, sync::OnceLock};

use libfuzzer_sys::fuzz_target;
use optes_tunnel::{keys, noise, replay::Replay, FRAME, NOISE_PATTERN, PROLOGUE, PROTOCOL_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEVICES: usize = 3;
const MAX_OPS: usize = 16;

struct Keys {
	gateway: ([u8; 32], [u8; 32]),
	devices: Vec<[u8; 32]>,
	device_public: Vec<[u8; 32]>,
	dir: PathBuf,
}

fn keys() -> &'static Keys {
	static KEYS: OnceLock<Keys> = OnceLock::new();
	KEYS.get_or_init(|| {
		let gateway = keys::generate_x25519().unwrap();
		let pairs: Vec<_> = (0..DEVICES).map(|_| keys::generate_x25519().unwrap()).collect();
		let dir = std::env::temp_dir().join(format!("optes-fuzz-hello-{}", std::process::id()));
		fs::create_dir_all(&dir).unwrap();
		Keys {
			gateway,
			devices: pairs.iter().map(|p| p.0).collect(),
			device_public: pairs.iter().map(|p| p.1).collect(),
			dir,
		}
	})
}

/// What the stored file of one device should make the gateway do.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stored {
	Last(u64),
	Unreadable,
}

fn expected(bytes: &[u8]) -> Stored {
	match std::str::from_utf8(bytes).ok().and_then(|t| t.trim().parse::<u64>().ok()) {
		| Some(n) => Stored::Last(n),
		| None => Stored::Unreadable,
	}
}

fuzz_target!(|data: &[u8]| {
	let k = keys();
	for entry in fs::read_dir(&k.dir).unwrap() {
		fs::remove_file(entry.unwrap().path()).unwrap();
	}
	let replay = Replay::new(k.dir.clone());
	let mut stored = [Stored::Last(0); DEVICES];

	let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
	let mut rest = data;
	for _ in 0..MAX_OPS {
		let [op, dev, tail @ ..] = rest else { break };
		let dev = usize::from(*dev) % DEVICES;
		let path = replay.path(&k.device_public[dev]);

		if op & 1 == 1 {
			// Overwrite the stored counter with up to 31 fuzzer bytes.
			let len = usize::from(op >> 3).min(tail.len());
			let (bytes, after) = tail.split_at(len);
			fs::write(&path, bytes).unwrap();
			stored[dev] = expected(bytes);
			rest = after;
			continue;
		}

		// A handshake with an 18-byte hello taken from the input.
		if tail.len() < 18 {
			break;
		}
		let (hello, after) = tail.split_at(18);
		rest = after;
		let counter = u64::from_be_bytes(hello[10..18].try_into().unwrap());
		let should_pass = hello[0] == PROTOCOL_VERSION
			&& matches!(stored[dev], Stored::Last(last) if counter > last);
		let before = fs::read(&path).ok();

		let (answered, reply_ok) = rt.block_on(handshake(k, dev, hello, &replay));

		assert_eq!(answered, should_pass, "hello {hello:?}, stored {:?}", stored[dev]);
		if answered {
			assert!(reply_ok, "reply to an accepted hello did not decrypt");
			assert_eq!(fs::read_to_string(&path).unwrap(), counter.to_string());
			stored[dev] = Stored::Last(counter);
		} else {
			assert_eq!(fs::read(&path).ok(), before, "refused handshake changed the stored counter");
		}
	}
});

/// Sends message 1 carrying `hello` (padded with zeros) from device `dev`.
/// Returns whether the gateway wrote anything back, and if so whether it
/// decrypted as message 2.
async fn handshake(k: &Keys, dev: usize, hello: &[u8], replay: &Replay) -> (bool, bool) {
	let mut init = snow::Builder::new(NOISE_PATTERN.parse().unwrap())
		.prologue(PROLOGUE)
		.local_private_key(&k.devices[dev])
		.remote_public_key(&k.gateway.1)
		.build_initiator()
		.unwrap();
	// Message 1 = ephemeral (32) + static (32 + 16) + payload + tag (16).
	let mut payload = vec![0_u8; FRAME - 32 - 48 - 16];
	payload[..hello.len()].copy_from_slice(hello);
	let mut msg1 = [0_u8; FRAME];
	assert_eq!(init.write_message(&payload, &mut msg1).unwrap(), FRAME);

	let (mut client, mut server) = tokio::io::duplex(4 * FRAME);
	client.write_all(&msg1).await.unwrap();
	let result = noise::respond(
		&mut server,
		&k.gateway.0,
		|key, h| replay.accept(key, h.counter),
		|h| noise::Welcome {
			level: h.level,
			tick_ms: 200,
			window_secs: 15,
			list_version: 0,
			gateway_time_ms: 0,
		},
	)
	.await;
	drop(server);

	let mut reply = Vec::new();
	client.read_to_end(&mut reply).await.unwrap();
	assert_eq!(result.is_ok(), !reply.is_empty(), "reply without success, or success without reply");
	if reply.is_empty() {
		return (false, false);
	}
	let mut out = [0_u8; FRAME];
	(true, reply.len() == FRAME && init.read_message(&reply, &mut out).is_ok())
}
