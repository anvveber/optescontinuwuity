use crate::media::storage_repository::{StorageRepository, get_file_name};
use conduwuit::{Result, debug, err, warn};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::{fs, io::AsyncWriteExt};
use walkdir::WalkDir;

pub(crate) struct FilesystemStorage {
	/// Full path to the media directory
	pub(crate) path: PathBuf,
}

impl StorageRepository for FilesystemStorage {
	async fn create(&self, key: &[u8], file_data: &[u8]) -> Result<()> {
		let path = self.get_file_path(key);
		debug!(?key, ?path, "Creating file");

		let mut inode = fs::File::create(&path)
			.await
			.map_err(|e| err!(Media(error!(?key, %e, "Failed to create file"))))?;

		inode
			.write_all(file_data)
			.await
			.map_err(|e| err!(Database(error!(?key, %e, "Failed to write to file"))))?;

		Ok(())
	}

	async fn get(&self, db_key: &[u8]) -> Result<Option<Vec<u8>>> {
		let path = self.get_file_path(db_key);
		debug!(?path, "Getting media file");

		if let Ok(_) = fs::try_exists(&path).await {
			// Pre-allocate a small buffer to attempt to reduce reallocations for small files
			let mut content = Vec::with_capacity(8096);
			BufReader::new(fs::File::open(&path).await?)
				.read_to_end(&mut content)
				.await?;

			if content.is_empty() {
				Ok(None)
			} else {
				Ok(Some(content))
			}
		} else {
			Err(err!(Media(
				error!(key = ?db_key, ?path, "Attempted to get file which doesn't exist")
			)))
		}
	}

	async fn delete(&self, key: &[u8]) -> Result<()> {
		let path = self.get_file_path(key);
		debug!(?key, ?path, "Deleting file");

		let file_rm = fs::remove_file(&path).await;
		if let Err(ref err) = file_rm {
			warn!(?key, ?err, "Failed to delete file");
		}
		file_rm.map_err(|e| e.into())
	}

	async fn list(&self) -> Vec<String> {
		let filenames: Vec<String> = WalkDir::new(&self.path)
			.into_iter()
			.filter_map(Result::ok)
			.filter(|entry| entry.file_type().is_file())
			.filter_map(|entry| {
				let relative_path = entry
					.path()
					.strip_prefix(&self.path)
					.unwrap_or(entry.path());
				Some(relative_path.to_str()?.to_owned())
			})
			.collect();
		filenames
	}
}

impl FilesystemStorage {
	pub async fn new(path: PathBuf) -> Result<Self> {
		let instance = Self { path };
		instance.create_media_dir().await?;
		Ok(instance)
	}

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

	pub async fn create_media_dir(&self) -> Result<()> {
		Ok(fs::create_dir_all(self.path.clone()).await?)
	}
}
