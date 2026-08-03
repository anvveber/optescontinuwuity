#![cfg(unix)]

use std::path::PathBuf;

use conduwuit::{Err, Result, warn};
use tokio::net::UnixListener;

/// Bind to a unix socket at the given `path`, creating the file and parent
/// directories as necessary.
pub(super) async fn bind_unix(path: &PathBuf, permissions_mode: String) -> Result<UnixListener> {
	use std::os::unix::fs::PermissionsExt;

	if path.exists() {
		tokio::fs::remove_file(&path)
			.await
			.map_err(|e| warn!("Failed to remove existing UNIX socket: {e}"))
			.unwrap();
	}

	let dir = path.parent().unwrap_or_else(|| std::path::Path::new("/"));
	if let Err(e) = tokio::fs::create_dir_all(dir).await {
		return Err!("Failed to create {dir:?} for socket {path:?}: {e}");
	}

	let listener = match UnixListener::bind(path) {
		| Ok(listener) => listener,
		| Err(e) => return Err!("Failed to bind listener {path:?}: {e}"),
	};

	let octal_perms =
		u32::from_str_radix(&permissions_mode, 8).expect("failed to convert octal permissions");
	let perms = std::fs::Permissions::from_mode(octal_perms);
	if let Err(e) = tokio::fs::set_permissions(&path, perms).await {
		return Err!("Failed to set socket {path:?} permissions: {e}");
	}

	Ok(listener)
}
