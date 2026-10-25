//! Arbitrary sequences of cells (as a peer could send after the handshake)
//! through the stream table: no panic, no unbounded state.
#![no_main]
use libfuzzer_sys::fuzz_target;
use optes_tunnel::{
	cell::{Cell, Kind},
	session, CELL,
};

fuzz_target!(|data: &[u8]| {
	let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
	rt.block_on(async {
		let table = session::new_table();
		let mut receivers = Vec::new();
		for chunk in data.chunks(CELL) {
			let mut buf = [0u8; CELL];
			buf[..chunk.len()].copy_from_slice(chunk);
			let Ok(cell) = Cell::decode(&buf) else { continue };
			if let Some(rest) = session::deliver(&table, cell).await {
				if rest.kind == Kind::Open && receivers.len() < 64 {
					receivers.push(session::register(&table, rest.stream));
				}
			}
			// Drain so bounded channels never block the loop.
			for rx in &mut receivers {
				while rx.try_recv().is_ok() {}
			}
		}
		assert!(table.lock().unwrap().len() <= 64);
	});
});
