//! optes-relay: local HTTP endpoint for a Matrix client. Every connection the
//! client opens is carried inside one encrypted tunnel to the core.

use std::{
	fs,
	path::{Path, PathBuf},
	sync::{
		atomic::{AtomicU32, Ordering},
		Arc, Mutex,
	},
	time::{Duration, Instant},
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
	net::TcpListener,
	sync::mpsc,
};
use tracing::{debug, info, warn};

#[derive(Parser, Debug)]
#[command(about = "Local relay: point your Matrix client at --listen")]
struct Args {
	/// Address the Matrix client connects to
	#[arg(long, default_value = "127.0.0.1:8008")]
	listen: String,
	/// SOCKS5 proxy for "tor" fronts (Tor's SocksPort, e.g. 127.0.0.1:9050)
	#[arg(long)]
	socks: Option<String>,
	/// SOCKS5 proxy for "i2p" fronts (i2pd's SOCKS proxy, e.g. 127.0.0.1:4447)
	#[arg(long)]
	socks_i2p: Option<String>,
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
	/// How often to ask the gateway for a newer front list while connected, s
	#[arg(long, default_value_t = 6 * 3600)]
	list_refresh_secs: u64,
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

	/// The counter for the next handshake (replay protection, see the
	/// gateway). One higher than the last one used; stored before it is sent,
	/// so a value is never used twice. With no stored value (first run, or the
	/// state directory was lost) it starts at the current time in ms, which is
	/// above anything this device key can have used before.
	fn next_hello_counter(&self) -> Result<u64> {
		let path = self.args.state_dir.join("hello.counter");
		let next = match fs::read_to_string(&path) {
			| Ok(text) => text
				.trim()
				.parse::<u64>()
				.with_context(|| format!("{} is not a number", path.display()))?
				.checked_add(1)
				.context("handshake counter overflow")?,
			| Err(e) if e.kind() == std::io::ErrorKind::NotFound => now_ms().max(1),
			| Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
		};
		let tmp = path.with_extension("tmp");
		fs::write(&tmp, next.to_string())
			.and_then(|()| fs::rename(&tmp, &path))
			.with_context(|| format!("writing {}", path.display()))?;
		Ok(next)
	}

	fn accepted_version(&self) -> u64 {
		fs::read_to_string(self.state_version())
			.ok()
			.and_then(|s| s.trim().parse().ok())
			.unwrap_or(0)
	}

	/// The newest valid list: the stored one if valid, else the initial one.
	/// If every list has expired, the newest correctly signed one is used
	/// anyway, only to reach the gateway and fetch a fresh list.
	fn load_list(&self) -> Result<FrontList> {
		let min = self.accepted_version();
		let now = now_ms() / 1000;
		let paths = [self.state_list(), self.args.front_list.clone()];
		let mut errors = Vec::new();
		for path in &paths {
			match read_and_verify(path, &self.list_key, min, Some(now)) {
				| Ok(list) => return Ok(list),
				| Err(e) => errors.push(format!("{}: {e:#}", path.display())),
			}
		}
		for path in &paths {
			if let Ok(list) = read_and_verify(path, &self.list_key, min, None) {
				warn!(version = list.version, "front list expired; using it only to fetch a new one");
				return Ok(list);
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
		let usable: Vec<&Front> = list.fronts.iter().filter(|f| self.can_dial(f)).collect();
		let front = (*usable
			.choose(&mut rand::thread_rng())
			.context("no front in the list is usable: each needs an onion or b32.i2p address and a SOCKS proxy for its transport")?)
		.clone();

		// The address stays out of the default log level: logs are kept.
		debug!(address = %front.address, "connecting");
		info!(transport = %front.transport, "connecting");
		let mut io = tokio::time::timeout(Duration::from_secs(120), self.dial(&front))
			.await
			.context("connect timed out")??;

		let hello = Hello {
			level: self.level as u8,
			list_version: list.version,
			counter: self.next_hello_counter()?,
		};
		let sent_at = Instant::now();
		let (crypto, welcome) = tokio::time::timeout(
			Duration::from_secs(120),
			noise::initiate(&mut io, &self.device_key, &gateway_key, hello),
		)
		.await
		.context("handshake timed out")??;
		// Gateway clock minus ours. The welcome was stamped about half a round
		// trip ago.
		let half_trip = i64::try_from(sent_at.elapsed().as_millis() / 2).unwrap_or(0);
		let clock_offset_ms = if welcome.gateway_time_ms == 0 {
			0
		} else {
			i64::try_from(welcome.gateway_time_ms)
				.unwrap_or(i64::MAX)
				.saturating_add(half_trip)
				.saturating_sub(i64::try_from(now_ms()).unwrap_or(i64::MAX))
		};

		let level = Level::from_u8(welcome.level)?;
		if level < self.level {
			bail!("gateway offered level {} below the requested {}", welcome.level, self.level as u8);
		}
		if welcome.tick_ms < 10 || welcome.window_secs == 0 {
			bail!("gateway sent unusable timing (tick {} ms, window {} s)", welcome.tick_ms, welcome.window_secs);
		}
		let policy = Policy {
			level,
			tick: Duration::from_millis(welcome.tick_ms.into()),
			window: Duration::from_secs(welcome.window_secs.into()),
			window_offset: Duration::ZERO,
			clock_offset_ms,
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

		if welcome.list_version > list.version || list.expires_at <= now_ms() / 1000 {
			out_tx.send(Cell::control(Kind::ListGet, 0)).await?;
		}
		// Long sessions (level 4 runs for weeks) must still pick up renewed lists.
		let refresher = {
			let out = out_tx.clone();
			let every = Duration::from_secs(self.args.list_refresh_secs.max(1));
			tokio::spawn(async move {
				loop {
					tokio::time::sleep(every).await;
					if out.send(Cell::control(Kind::ListGet, 0)).await.is_err() {
						return;
					}
				}
			})
		};
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
		refresher.abort();
		*self.current.lock().expect("lock") = None;
		handle.table.lock().expect("lock").clear();
		info!("tunnel down");
		result
	}

	fn proxy_for(&self, transport: &str) -> Option<&str> {
		match transport {
			| "tor" => self.args.socks.as_deref(),
			| "i2p" => self.args.socks_i2p.as_deref(),
			| _ => None,
		}
	}

	/// A front is usable if its address passes [`Front::check`] (onion or
	/// b32.i2p only) and this relay has a proxy for its transport.
	fn can_dial(&self, front: &Front) -> bool {
		if front.check().is_err() {
			return false;
		}
		#[cfg(feature = "test-fronts")]
		if front.transport == "tcp" {
			return true;
		}
		self.proxy_for(&front.transport).is_some()
	}

	async fn dial(&self, front: &Front) -> Result<Box<dyn Io>> {
		front.check()?;
		match front.transport.as_str() {
			| "tor" | "i2p" => {
				let proxy = self
					.proxy_for(&front.transport)
					.context("no SOCKS proxy configured for this transport")?;
				let stream = tokio_socks::tcp::Socks5Stream::connect(proxy, front.address.as_str()).await?;
				Ok(Box::new(stream))
			},
			#[cfg(feature = "test-fronts")]
			| "tcp" => Ok(Box::new(tokio::net::TcpStream::connect(&front.address).await?)),
			| other => bail!("unknown transport {other}"),
		}
	}
}

/// `now = None` accepts an expired list.
fn read_and_verify(path: &Path, key: &VerifyingKey, min: u64, now: Option<u64>) -> Result<FrontList> {
	let signed = fs::read_to_string(path)?;
	match now {
		| Some(now) => frontlist::verify(&signed, key, min, now),
		| None => frontlist::verify_ignoring_expiry(&signed, key, min),
	}
}
