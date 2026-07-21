use std::{convert::TryFrom, path::PathBuf, sync::Arc};

use conduwuit::{Config, Err, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::media::{
	base64_encode_key, mxc::Mxc, storage_repository::fs_storage::FilesystemStorage,
};

pub mod fs_storage;

pub(crate) trait StorageRepository {
	async fn create(&self, db_key: &[u8], file_data: &[u8]) -> Result<()>;
	async fn get(&self, db_key: &[u8]) -> Result<&'_ [u8]>;
	/// Delete
	async fn delete(&self, db_key: Vec<&[u8]>) -> Vec<Result<()>>;
	async fn list<'mxc>(&self) -> Result<Vec<Mxc<'mxc>>>;
}

pub enum StorageRepositoryTypes {
	FilesystemStorage,
}

impl StorageRepositoryTypes {
	fn as_str(&self) -> &'static str {
		match self {
			| StorageRepositoryTypes::FilesystemStorage => "filesystem",
		}
	}
}

impl TryFrom<&str> for StorageRepositoryTypes {
	type Error = &'static str;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		match value {
			| "filesystem" => Ok(StorageRepositoryTypes::FilesystemStorage),
			| _ => Err("Unknown storage repository type"),
		}
	}
}

impl TryFrom<String> for StorageRepositoryTypes {
	type Error = &'static str;

	fn try_from(value: String) -> Result<Self, Self::Error> { Self::try_from(value.as_str()) }
}

impl Serialize for StorageRepositoryTypes {
	fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(self.as_str())
	}
}

impl<'de> Deserialize<'de> for StorageRepositoryTypes {
	fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let s = String::deserialize(deserializer)?;
		Ok(s.try_into().map_err(serde::de::Error::custom)?)
	}
}

pub(crate) struct StorageRepositoryFactory {
	filesystem: Arc<FilesystemStorage>,
}

impl StorageRepositoryFactory {
	pub fn new(config: &Arc<Config>) -> Arc<Self> {
		let mut path = PathBuf::new();
		path.push(config.database_path.clone());
		path.push("media");
		let filesystem = Arc::new(FilesystemStorage { path });

		Arc::new(Self { filesystem })
	}

	pub fn get_repository(
		&self,
		repo_type: StorageRepositoryTypes,
	) -> Arc<impl StorageRepository> {
		match repo_type {
			| StorageRepositoryTypes::FilesystemStorage => self.filesystem.clone(),
		}
	}
}

/// Returns the filename of a file given a database key.
///
/// Uses the base64 hash of the sha256 digest of the database key as the
/// filename. Base64 is used to prevent the length of the path  from exceeding
/// the filename length limits in most filesystems.
pub(crate) fn get_file_name(key: &[u8]) -> String {
	let sha256_digest = <sha2::Sha256 as sha2::Digest>::digest(key);
	let b64_encoded = base64_encode_key(&sha256_digest);
	b64_encoded
}
