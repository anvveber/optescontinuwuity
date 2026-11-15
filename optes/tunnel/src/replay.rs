//! Replay protection for handshake message 1 (gateway side).
//!
//! Message 1 of Noise IK can be recorded (by a front) and sent again. Each
//! hello therefore carries a counter, and the gateway accepts a hello only if
//! its counter is higher than the highest one it accepted from that device.
//! The highest value is kept in `<state-dir>/<device key in hex>.ctr`. A
//! relay starts its counter at the time of its first run and then adds one
//! per handshake, so the file shows when a device was set up and how often it
//! has connected since, not when it connected.

use std::{fs, path::PathBuf, sync::Mutex};

use tracing::warn;

pub struct Replay {
	dir: PathBuf,
	lock: Mutex<()>,
}

impl Replay {
	/// Counters are kept in `dir`, which must exist.
	pub fn new(dir: PathBuf) -> Self { Self { dir, lock: Mutex::new(()) } }

	/// The file that holds the highest counter accepted from `device`.
	pub fn path(&self, device: &[u8; 32]) -> PathBuf {
		let name: String = device.iter().map(|b| format!("{b:02x}")).collect();
		self.dir.join(format!("{name}.ctr"))
	}

	/// True if `counter` is higher than every counter accepted from `device`
	/// before; records it. Fails closed: if the stored value can't be read or
	/// the new value can't be stored, the handshake is refused.
	pub fn accept(&self, device: &[u8; 32], counter: u64) -> bool {
		let _guard = self.lock.lock().expect("replay lock");
		let path = self.path(device);
		let last = match fs::read_to_string(&path) {
			| Ok(text) => match text.trim().parse::<u64>() {
				| Ok(last) => last,
				| Err(_) => {
					warn!(file = %path.display(), "unreadable replay counter; refusing handshake");
					return false;
				},
			},
			| Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
			| Err(e) => {
				warn!(file = %path.display(), "cannot read replay counter: {e}");
				return false;
			},
		};
		if counter <= last {
			return false;
		}

		let tmp = path.with_extension("tmp");
		if let Err(e) = fs::write(&tmp, counter.to_string()).and_then(|()| fs::rename(&tmp, &path)) {
			warn!(file = %path.display(), "cannot store replay counter: {e}");
			return false;
		}
		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn temp_dir(name: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("optes-replay-{name}-{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn only_higher_counters_pass() {
		let dir = temp_dir("higher");
		let replay = Replay::new(dir.clone());
		let (a, b) = ([1_u8; 32], [2_u8; 32]);
		assert!(!replay.accept(&a, 0), "0 is never fresh");
		assert!(replay.accept(&a, 5));
		assert!(!replay.accept(&a, 5), "same counter again");
		assert!(!replay.accept(&a, 4));
		assert!(replay.accept(&a, 6));
		assert!(replay.accept(&b, 1), "devices are independent");
		assert_eq!(fs::read_to_string(replay.path(&a)).unwrap(), "6");
		assert!(replay.accept(&a, u64::MAX));
		assert!(!replay.accept(&a, u64::MAX));
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn unreadable_file_refuses() {
		let dir = temp_dir("unreadable");
		let replay = Replay::new(dir.clone());
		let dev = [3_u8; 32];
		fs::write(replay.path(&dev), "not a number").unwrap();
		assert!(!replay.accept(&dev, u64::MAX));
		fs::write(replay.path(&dev), [0xff, 0xfe]).unwrap();
		assert!(!replay.accept(&dev, u64::MAX), "invalid UTF-8");
		fs::write(replay.path(&dev), " 7\n").unwrap();
		assert!(!replay.accept(&dev, 7));
		assert!(replay.accept(&dev, 8));
		fs::remove_dir_all(dir).unwrap();
	}
}
