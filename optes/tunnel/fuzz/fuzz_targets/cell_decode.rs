//! Any bytes into Cell::decode: no panic; valid cells round-trip exactly.
#![no_main]
use libfuzzer_sys::fuzz_target;
use optes_tunnel::cell::Cell;

fuzz_target!(|data: &[u8]| {
	if let Ok(cell) = Cell::decode(data) {
		let encoded = cell.encode();
		let again = Cell::decode(&encoded).expect("re-decoding an encoded cell must work");
		assert_eq!(again, cell);
	}
});
