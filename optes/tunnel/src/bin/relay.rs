//! optes-relay: local HTTP endpoint for a Matrix client. Every connection the
//! client opens is carried inside one encrypted tunnel to the core.

use std::{
	fs,
	path::{Path, PathBuf},
	sync::{
		atomic::{AtomicU32, Ordering},
		Arc, Mutex,
	},
	time::Duration,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use ed25519_dalek::VerifyingKey;
use optes_tunnel::{
	cell::{Cell, Kind},
	frontlist::{self, Front, FrontList},
	keys,
	noise::{self, Hello},
	policy::{Level, Policy},
	session::{self, now_ms, StreamTable},
};
use rand::seq::SliceRandom;
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::{TcpListener, TcpStream},
	sync::mpsc,
};
use tracing::{debug, info, warn};

#[derive(Parser, Debug)]
#[command(about = "Local relay: point your Matrix client at --listen")]
struct Args {
	/// Address the Matrix client connects to
	#[arg(long, default_value = "127.0.0.1:8008")]
	listen: String,
	/// SOCKS5 proxy for "tor" fronts (e.g. 127.0.0.1:9050)
	#[arg(long)]
	socks: Option<String>,
	/// Initial signed front list (from setup-core.sh)
	#[arg(long)]
	front_list: PathBuf,
	/// Pinned front-list public key: base64, or a file containing it
	#[arg(long)]
	list_key: String,
	/// This device's X25519 private key file
	#[arg(long)]
	device_key: PathBuf,
	/// Privacy level: 1 (burst padding), 2 (windows), 3 (constant rate)
	#[arg(long, default_value_t = 2)]
	level: u8,
	/// Where accepted front lists and their version are kept
	#[arg(long)]
	state_dir: PathBuf,
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct Handle {
	out: mpsc::Sender<Cell>,
	table: StreamTable,
	next_id: AtomicU32,
}

struct Relay {
	args: Args,
	list_key: VerifyingKey,
	device_key: [u8; 32],
	level: Level,
	current: Mutex<Option<Arc<Handle>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
		)
		.init();

	let args = Args::parse();
	fs::create_dir_all(&args.state_dir)?;
	let relay = Arc::new(Relay {
		list_key: keys::verifying_key(&keys::read32_inline_or_file(&args.list_key)?)?,
		device_key: keys::read32(&args.device_key)?,
		level: Level::from_u8(args.level)?,
		current: Mutex::new(None),
		args,
	});

	// Fail early if the front list is unusable.
	relay.load_list()?;

	let listener = TcpListener::bind(&relay.args.listen)
		.await
		.with_context(|| format!("binding {}", relay.args.listen))?;
	info!(addr = %relay.args.listen, "point your Matrix client here");
	tokio::spawn(accept_loop(relay.clone(), listener));

	let mut backoff = Duration::from_secs(2);
	loop {
		match relay.clone().connect_once().await {
			| Ok(()) => backoff = Duration::from_secs(2),
			| Err(e) => warn!("tunnel: {e:#}"),
		}
		*relay.current.lock().expect("lock") = None;
		tokio::time::sleep(backoff).await;
		backoff = (backoff * 2).min(Duration::from_secs(60));
	}
}

async fn accept_loop(relay: Arc<Relay>, listener: TcpListener) {
	loop {
		let Ok((socket, _)) = listener.accept().await else {
			continue;
		};
		let handle = relay.current.lock().expect("lock").clone();
		let Some(handle) = handle else {
			debug!("no tunnel; dropping local connection");
			continue;
		};
		let id = handle.next_id.fetch_add(2, Ordering::Relaxed);
		let data_rx = session::register(&handle.table, id);
		if handle.out.send(Cell::control(Kind::Open, id)).await.is_err() {
			continue;
		}
		session::spawn_stream(socket, id, handle.out.clone(), data_rx);
	}
}

impl Relay {
	fn state_list(&self) -> PathBuf { self.args.state_dir.join("frontlist.signed") }

	fn state_version(&self) -> PathBuf { self.args.state_dir.join("frontlist.version") }

	fn accepted_version(&self) -> u64 {
		fs::read_to_string(self.state_version())
			.ok()
			.and_then(|s| s.trim().parse().ok())
			.unwrap_or(0)
	}

	/// The newest valid list: the stored one if valid, else the initial one.
	fn load_list(&self) -> Result<FrontList> {
		let min = self.accepted_version();
		let now = now_ms() / 1000;
		let mut errors = Vec::new();
		for path in [self.state_list(), self.args.front_list.clone()] {
			match read_and_verify(&path, &self.list_key, min, now) {
				| Ok(list) => return Ok(list),
				| Err(e) => errors.push(format!("{}: {e:#}", path.display())),
			}
		}
		bail!("no usable front list:\n  {}", errors.join("\n  "))
	}

	/// Verifies a list fetched from the gateway and stores it if newer.
	fn store_list(&self, signed: &str) -> Result<()> {
		let current = self.accepted_version();
		let list = frontlist::verify(signed, &self.list_key, current, now_ms() / 1000)?;
		if list.version > current {
			fs::write(self.state_list(), signed)?;
			fs::write(self.state_version(), list.version.to_string())?;
			info!(version = list.version, "stored new front list");
		}
		Ok(())
	}

	async fn connect_once(self: Arc<Self>) -> Result<()> {
		let list = self.load_list()?;
		let gateway_key = list.gateway_key()?;
		let front = list
			.fronts
			.choose(&mut rand::thread_rng())
			.context("front list is empty")?
			.clone();

		info!(address = %front.address, "connecting");
		let mut io = tokio::time::timeout(Duration::from_secs(120), self.dial(&front))
			.await
			.context("connect timed out")??;

		let hello = Hello { level: self.level as u8, list_version: list.version };
		let (crypto, welcome) = tokio::time::timeout(
			Duration::from_secs(120),
			noise::initiate(&mut io, &self.device_key, &gateway_key, hello),
		)
		.await
		.context("handshake timed out")??;

		let level = Level::from_u8(welcome.level)?;
		let policy = Policy {
			level,
			tick: Duration::from_millis(welcome.tick_ms.into()),
			window: Duration::from_secs(welcome.window_secs.into()),
			window_offset: Duration::ZERO,
		};
		info!(?level, "tunnel up");

		let (out_tx, out_rx) = mpsc::channel(session::OUT_QUEUE);
		let (in_tx, mut in_rx) = mpsc::channel(session::OUT_QUEUE);
		let handle = Arc::new(Handle {
			out: out_tx.clone(),
			table: session::new_table(),
			next_id: AtomicU32::new(1),
		});
		*self.current.lock().expect("lock") = Some(handle.clone());

		if welcome.list_version > list.version {
			out_tx.send(Cell::control(Kind::ListGet, 0)).await?;
		}
		drop(out_tx);

		let dispatcher = {
			let relay = self.clone();
			let table = handle.table.clone();
			tokio::spawn(async move {
				let mut parts = Vec::new();
				while let Some(cell) = in_rx.recv().await {
					let Some(cell) = session::deliver(&table, cell).await else {
						continue;
					};
					if cell.kind == Kind::ListPart {
						if cell.payload.is_empty() {
							let signed = String::from_utf8_lossy(&parts).into_owned();
							if let Err(e) = relay.store_list(&signed) {
								warn!("front list from gateway rejected: {e:#}");
							}
							parts.clear();
						} else {
							parts.extend_from_slice(&cell.payload);
						}
					}
				}
			})
		};
		// The session's queue stays open while `handle` lives.
		let result = session::run(io, crypto, policy, out_rx, in_tx).await;
		dispatcher.abort();
		*self.current.lock().expect("lock") = None;
		handle.table.lock().expect("lock").clear();
		info!("tunnel down");
		result
	}

	async fn dial(&self, front: &Front) -> Result<Box<dyn Io>> {
		match front.transport.as_str() {
			| "tor" => {
				let proxy = self
					.args
					.socks
					.as_deref()
					.context("front needs --socks (Tor's SOCKS port)")?;
				let stream = tokio_socks::tcp::Socks5Stream::connect(proxy, front.address.as_str()).await?;
				Ok(Box::new(stream))
			},
			| "tcp" => Ok(Box::new(TcpStream::connect(&front.address).await?)),
			| other => bail!("unknown transport {other}"),
		}
	}
}

fn read_and_verify(path: &Path, key: &VerifyingKey, min: u64, now: u64) -> Result<FrontList> {
	let signed = fs::read_to_string(path)?;
	frontlist::verify(&signed, key, min, now)
}
