//! optes-keys: generate keys and sign front lists.

use std::{fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use optes_tunnel::{
	frontlist::{self, Front, FrontList},
	keys,
	session::now_ms,
};

#[derive(Parser, Debug)]
#[command(about = "Keys and front lists for the optes tunnel")]
struct Args {
	#[command(subcommand)]
	cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
	/// X25519 keypair (gateway or device): writes PREFIX.key and PREFIX.pub
	GenX25519 { prefix: PathBuf },
	/// Ed25519 keypair (front-list signing): writes PREFIX.key and PREFIX.pub
	GenEd25519 { prefix: PathBuf },
	/// Build and sign a front list
	SignList {
		/// Ed25519 private key file
		#[arg(long)]
		key: PathBuf,
		/// Must be higher than every list published before
		#[arg(long)]
		version: u64,
		#[arg(long, default_value_t = 30)]
		expires_days: u64,
		/// Gateway X25519 public key: base64 or file
		#[arg(long)]
		gateway_key: String,
		/// transport:host:port, e.g. tor:abc.onion:443 or i2p:abc.b32.i2p:443 (repeatable)
		#[arg(long = "front", required = true)]
		fronts: Vec<String>,
		#[arg(long)]
		out: PathBuf,
	},
	/// Check a signed front list
	VerifyList {
		/// Ed25519 public key: base64 or file
		#[arg(long)]
		key: String,
		file: PathBuf,
	},
}

fn parse_front(value: &str) -> Result<Front> {
	let Some((transport, address)) = value.split_once(':') else {
		bail!("front must be transport:host:port, got {value}");
	};
	if !matches!(transport, "tor" | "i2p" | "tcp") {
		bail!("transport must be tor, i2p or tcp, got {transport}");
	}
	if !address.contains(':') {
		bail!("front address needs a port: {address}");
	}
	Ok(Front { transport: transport.into(), address: address.into() })
}

fn main() -> Result<()> {
	match Args::parse().cmd {
		| Cmd::GenX25519 { prefix } => {
			let (private, public) = keys::generate_x25519()?;
			keys::write_secret(&prefix.with_extension("key"), &private)?;
			keys::write_public(&prefix.with_extension("pub"), &public)?;
			println!("{}", keys::encode(&public));
		},
		| Cmd::GenEd25519 { prefix } => {
			let key = keys::generate_ed25519();
			keys::write_secret(&prefix.with_extension("key"), key.as_bytes())?;
			keys::write_public(&prefix.with_extension("pub"), key.verifying_key().as_bytes())?;
			println!("{}", keys::encode(key.verifying_key().as_bytes()));
		},
		| Cmd::SignList { key, version, expires_days, gateway_key, fronts, out } => {
			let signing = SigningKey::from_bytes(&keys::read32(&key)?);
			let now = now_ms() / 1000;
			let list = FrontList {
				version,
				issued_at: now,
				expires_at: now + expires_days * 86_400,
				gateway_key: keys::encode(&keys::read32_inline_or_file(&gateway_key)?),
				fronts: fronts.iter().map(|f| parse_front(f)).collect::<Result<_>>()?,
			};
			fs::write(&out, frontlist::sign(&list, &signing)?)
				.with_context(|| format!("writing {}", out.display()))?;
			println!("signed version {version}, expires in {expires_days} days");
		},
		| Cmd::VerifyList { key, file } => {
			let key = keys::verifying_key(&keys::read32_inline_or_file(&key)?)?;
			let list = frontlist::verify(&fs::read_to_string(&file)?, &key, 0, now_ms() / 1000)?;
			println!("{}", serde_json::to_string_pretty(&list)?);
		},
	}
	Ok(())
}
