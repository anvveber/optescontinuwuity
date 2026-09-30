use std::sync::Arc;

use rocksdb::WriteBatchWithTransaction;
use serde::Serialize;

use super::Map;
use crate::{serialize_to_vec, util::or_else};

/// Atomic writes across maps. Dropping an uncommitted batch discards its
/// writes.
pub struct Batch {
	map: Arc<Map>,
	writes: WriteBatchWithTransaction<false>,
	wake: Vec<(Arc<Map>, Vec<u8>)>,
}

impl Map {
	pub fn batch(self: &Arc<Self>) -> Batch {
		Batch {
			map: self.clone(),
			writes: WriteBatchWithTransaction::default(),
			wake: Vec::new(),
		}
	}
}

impl Batch {
	pub fn insert(&mut self, map: &Arc<Map>, key: &[u8], value: impl AsRef<[u8]>) {
		assert!(Arc::ptr_eq(&self.map.db, &map.db), "batch maps must use the same database");
		self.writes.put_cf(&map.cf(), key, value.as_ref());
		self.wake.push((map.clone(), key.to_vec()));
	}

	pub fn put<K: Serialize, V: Serialize>(&mut self, map: &Arc<Map>, key: K, value: V) {
		let key = serialize_to_vec(key).expect("failed to serialize batch key");
		let value = serialize_to_vec(value).expect("failed to serialize batch value");
		self.insert(map, &key, value);
	}

	pub fn remove(&mut self, map: &Arc<Map>, key: &[u8]) {
		assert!(Arc::ptr_eq(&self.map.db, &map.db), "batch maps must use the same database");
		self.writes.delete_cf(&map.cf(), key);
	}

	pub fn commit(self) {
		self.map
			.db
			.db
			.write_opt(&self.writes, &self.map.write_options)
			.or_else(or_else)
			.expect("database batch write error");
		if !self.map.db.corked() {
			self.map.db.flush().expect("database flush error");
		}
		for (map, key) in self.wake {
			map.watchers.wake(&key);
		}
	}
}
