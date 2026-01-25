mod commands;

use std::path::PathBuf;

use clap::Subcommand;
use conduwuit::Result;

use crate::admin_command_dispatch;

#[admin_command_dispatch]
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
	/// Time elapsed since startup
	Uptime,

	/// Show configuration values
	ShowConfig,

	/// Reload configuration values
	ReloadConfig {
		path: Option<PathBuf>,
	},

	/// Print database memory usage statistics
	MemoryUsage,

	/// Clears all of Continuwuity's caches
	ClearCaches,

	/// Performs an online backup of the database (only available for RocksDB
	///   at the moment)
	BackupDatabase,

	/// List database backups
	ListBackups,

	/// Send a message to the admin room.
	AdminNotice {
		message: Vec<String>,
	},

	/// Hot-reload the server
	#[clap(alias = "reload")]
	ReloadMods,

	#[cfg(unix)]
	/// Restart the server
	Restart {
		#[arg(short, long)]
		force: bool,
	},

	/// Shutdown the server
	Shutdown,

	/// Import an existing signing key, replacing the existing key.
	///
	/// Old Synapse key:
	/// ```plaintext
	/// ed25519 <key_id> <b64_key>
	/// ```
	/// Old Dendrite key:
	/// ```plaintext
	/// -----BEGIN MATRIX PRIVATE KEY-----
	/// Key-ID: ed25519:<key_id>
	///
	/// <b64_key>
	/// -----END MATRIX PRIVATE KEY-----
	/// ```
	ImportSigningKey {
		key_id: String,
		b64_key: String,
	},
}
