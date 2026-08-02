mod data;
pub(super) mod migrations;
pub mod mxc;
mod preview;
mod remote;
mod storage_repository;
mod tests;
mod thumbnail;

use std::{path::PathBuf, sync::Arc, time::SystemTime};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use conduwuit::{
	Err, Result, Server, debug, debug_error, debug_info, debug_warn, err, error, trace,
	utils::{
		self, MutexMap,
		time::{self, TimeDirection},
	},
	warn,
};
use ruma::{OwnedMxcUri, UserId, http_headers::ContentDisposition};
use tokio::{
	fs,
	io::{AsyncReadExt, AsyncWriteExt, BufReader},
};

use self::data::{Data, Metadata};
pub use self::thumbnail::Dim;
use crate::{
	Dep, client, globals,
	media::{
		mxc::Mxc,
		storage_repository::{
			StorageRepository, StorageRepositoryFactory, StorageRepositoryTypes,
		},
	},
	moderation, sending,
};

#[derive(Debug)]
pub struct FileMeta {
	pub content: Option<Vec<u8>>,
	pub content_type: Option<String>,
	pub content_disposition: Option<ContentDisposition>,
}

pub struct Service {
	url_preview_mutex: MutexMap<String, ()>,
	pub(super) db: Data,
	storage_factory: Arc<StorageRepositoryFactory>,
	services: Services,
}

struct Services {
	server: Arc<Server>,
	client: Dep<client::Service>,
	globals: Dep<globals::Service>,
	sending: Dep<sending::Service>,
	moderation: Dep<moderation::Service>,
}

/// generated MXC ID (`media-id`) length
pub const MXC_LENGTH: usize = 32;

/// Cache control for immutable objects.
pub const CACHE_CONTROL_IMMUTABLE: &str = "public,max-age=31536000,immutable";

/// Default cross-origin resource policy.
pub const CORP_CROSS_ORIGIN: &str = "cross-origin";

#[async_trait]
impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			url_preview_mutex: MutexMap::new(),
			db: Data::new(args.db),
			storage_factory: StorageRepositoryFactory::new(&args.server.config),
			services: Services {
				server: args.server.clone(),
				client: args.depend::<client::Service>("client"),
				globals: args.depend::<globals::Service>("globals"),
				sending: args.depend::<sending::Service>("sending"),
				moderation: args.depend::<moderation::Service>("moderation"),
			},
		}))
	}

	async fn worker(self: Arc<Self>) -> Result<()> { Ok(()) }

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	/// Uploads a file.
	pub async fn create(
		&self,
		mxc: &Mxc<'_>,
		user: Option<&UserId>,
		content_disposition: Option<&ContentDisposition>,
		content_type: Option<&str>,
		file: &[u8],
	) -> Result<()> {
		let storage = self.get_storage_repo()?;
	}

	/// Deletes a file in the database and from the media directory via an MXC
	pub async fn delete(&self, mxc: &Mxc<'_>) -> Result<()> {}

	/// Downloads a file via an MXC.
	pub async fn get(&self, mxc: &Mxc<'_>) -> Result<Option<FileMeta>> {
		match self
			.db
			.search_file_metadata(mxc, &ImageDimensions::default())
			.await
		{
			| Ok(Metadata { content_disposition, content_type, key }) => {
				let storage_repo = self.get_storage_repo()?;
				let content = storage_repo.get(&key).await?;

				Ok(Some(FileMeta {
					content,
					content_type,
					content_disposition,
				}))
			},
			| _ => Ok(None),
		}
	}

	/// Deletes all media by the specified user
	pub async fn delete_from_user(&self, user: &UserId) -> Result<usize> {}

	/// Gets all the MXC URIs in our media database
	/// TODO: Update this with new DB format (MXC keys, everything else is in value)
	pub async fn get_all_mxcs(&self) -> Result<Vec<OwnedMxcUri>> {
		let all_keys = self.db.get_all_media_keys().await;

		let mut mxcs = Vec::with_capacity(all_keys.len());

		for key in all_keys {
			trace!("Full MXC key from database: {key:?}");

			let mut parts = key.split(|&b| b == 0xFF);
			let mxc = parts
				.next()
				.map(|bytes| {
					utils::string_from_bytes(bytes).map_err(|e| {
						err!(Database(error!(
							"Failed to parse MXC unicode bytes from our database: {e}"
						)))
					})
				})
				.transpose()?;

			let Some(mxc_s) = mxc else {
				debug_warn!(
					mxc,
					"Parsed MXC URL unicode bytes from database but is still invalid"
				);
				continue;
			};

			trace!("Parsed MXC key to URL: {mxc_s}");
			let mxc = OwnedMxcUri::from(mxc_s);

			if mxc.is_valid() {
				mxcs.push(mxc);
			} else {
				debug_warn!("{mxc:?} from database was found to not be valid");
			}
		}

		Ok(mxcs)
	}

	#[inline]
	pub async fn get_metadata(&self, mxc: &Mxc<'_>) -> Option<FileMeta> {
		self.db
			.search_file_metadata(mxc, &Dim::default())
			.await
			.map(|metadata| FileMeta {
				content_disposition: metadata.content_disposition,
				content_type: metadata.content_type,
				content: None,
			})
			.ok()
	}

	#[inline]
	fn get_storage_repo(&self) -> Result<Arc<impl StorageRepository>> {
		let storage_destination = StorageRepositoryTypes::try_from(
			self.services.server.config.file_storage_type.clone(),
		)?;
		let repo = self.storage_factory.get_repository(storage_destination);
		Ok(repo.clone())
	}
}

#[inline]
#[must_use]
pub fn base64_encode_key(key: &[u8]) -> String {
	general_purpose::URL_SAFE_NO_PAD.encode(key)
}
