use std::path::{Path, PathBuf};

use conduwuit::{Result, debug, debug_error, err};
use tokio::{fs, io::AsyncWriteExt};

use crate::media::{
	mxc::Mxc,
	storage_repository::{StorageRepository, get_file_name},
};

pub(crate) struct FilesystemStorage {
	/// Full path to the media directory
	pub(crate) path: PathBuf,
}

impl StorageRepository for FilesystemStorage {
	async fn create(&self, db_key: &[u8], file_data: &[u8]) -> Result {
		let path = self.get_file_path(db_key);
		debug!(?db_key, ?path, "Creating media file");

		let mut inode = fs::File::create(&path).await.map_err(|e| {
			err!(Media(error!(key = ?db_key,"Failed to create file (inode): {e}")))
		})?;
		inode
			.write_all(file_data)
			.await
			.map_err(|e| err!(Database(error!(key=?db_key, "Failed to write to file: {e}"))))?;

		Ok(())
	}

	async fn delete(&self, db_key: &[u8]) -> Result<()> {
		let path = self.get_file_path(db_key);
		debug!(?db_key, ?path, "Removing media file");

		let file_rm = fs::remove_file(&path).await;
		Ok(file_rm?)
	}
}

impl FilesystemStorage {
	/// Returns the full path to a file given a database key.
	///
	/// Uses the base64 hash of the sha256 digest of the database key as the
	/// filename. Base64 is used to prevent the length of the path  from
	/// exceeding the filename length limits in most filesystems.
	fn get_file_path(&self, key: &[u8]) -> PathBuf {
		let mut r = self.path.clone();
		r.push(get_file_name(key));
		r
	}

	// TODO: CALL THIS AT SOME POINT
	pub async fn create_media_dir(&self) -> Result<()> {
		Ok(fs::create_dir_all(self.path.clone()).await?)
	}
}
