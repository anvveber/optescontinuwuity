use conduwuit::{Config, Error, Result, err};
use ruma::OwnedServerName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{convert::TryFrom, path::PathBuf, sync::Arc};

use crate::media::{
	base64_encode_key, mxc::Mxc, storage_repository::fs_storage::FilesystemStorage,
};

pub mod fs_storage;

/// Where a file originated, to be used for more efficient cleanup and similar
#[derive(Debug)]
pub enum FileOrigin {
	/// A local file, originating from a user on the local server
	Local,
	/// A remote file, originating from another server defined by parameter
	Remote(OwnedServerName),
	/// A file of unknown origin. Examples include files found on startup but not existing in DB
	/// AND not in a subfolder of a valid server name (e.g. example.com/filename)
	Unknown,
}

pub(crate) trait StorageRepository {
	/// Create a file with a provided db key and file data.
	async fn create(&self, key: &[u8], file_data: &[u8]) -> Result<()>;

	/// Get a file based on the full DB key. Returns `Ok(Some())` if the file exists and is non-empty,
	/// `Ok(None)` if it's empty, and `Err()` if there was any errors reading the file (including if
	/// it doesn't exist)
	async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;

	/// Delete singular file based on a given key.
	async fn delete(&self, key: &[u8]) -> Result<()>;

	/// List all stored files, relative to root storage location (i.e. "directory/filename")
	async fn list(&self) -> Vec<String>;
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
	type Error = Error;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		match value {
			| "filesystem" => Ok(StorageRepositoryTypes::FilesystemStorage),
			| _ => Err(err!("Unknown storage repository type")),
		}
	}
}

impl TryFrom<String> for StorageRepositoryTypes {
	type Error = Error;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		Self::try_from(value.as_str())
	}
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
		// Initialize filesystem storage
		let mut media_path = PathBuf::new();
		media_path.push(config.database_path.clone());
		media_path.push("media");
		let filesystem = Arc::new(FilesystemStorage { path: media_path });

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
