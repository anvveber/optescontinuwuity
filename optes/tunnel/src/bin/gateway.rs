//! optes-gateway: terminates relay tunnels on the core and forwards each
//! stream to continuwuity.

use std::{
	fs,
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use optes_tunnel::{
	cell::{Cell, Kind},
	frontlist, keys,
	noise::{self, Welcome},
	policy::{Level, Policy},
	session, MAX_PAYLOAD,
};
use rand::Rng;
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::{TcpListener, TcpStream, UnixListener, UnixStream},
	sync::mpsc,
};
use tracing::{debug, info, warn};

#[derive(Parser, Debug)]
#[command(about = "Terminates optes relay tunnels and forwards streams to continuwuity")]
struct Args {
	/// Where Tor delivers connections: unix:/path or tcp:host:port
	#[arg(long)]
	listen: String,
	/// continuwuity: unix:/path or tcp:host:port
	#[arg(long)]
	upstream: String,
	/// Gateway X25519 private key file
	#[arg(long)]
	key: PathBuf,
	/// Directory of allowed device public keys (*.pub, one base64 key each)
	#[arg(long)]
	devices: PathBuf,
	/// Signed front list served to relays
	#[arg(long)]
	front_list: Option<PathBuf>,
	/// Constant-rate tick (level 3), ms. Same for every client.
	#[arg(long, default_value_t = 200)]
	tick_ms: u32,
	/// Window length (level 2), s. Should equal release_interval_secs.
	#[arg(long, default_value_t = 15)]
	window_secs: u32,
	/// Lowest level accepted; clients asking for less are raised to this.
	#[arg(long, default_value_t = 1)]
	min_level: u8,
}

#[derive(Clone, Debug)]
enum Endpoint {
	Unix(PathBuf),
	Tcp(String),
}

impl Endpoint {
	fn parse(value: &str) -> Result<Self> {
		if let Some(path) = value.strip_prefix("unix:") {
			Ok(Self::Unix(path.into()))
		} else if let Some(addr) = value.strip_prefix("tcp:") {
			Ok(Self::Tcp(addr.into()))
		} else {
			bail!("endpoint must start with unix: or tcp: ({value})")
		}
	}
}

struct Gateway {
	key: [u8; 32],
	devices: PathBuf,
	front_list: Option<PathBuf>,
	upstream: Endpoint,
	tick_ms: u32,
	window_secs: u32,
	min_level: Level,
}

#[tokio::main]
async fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
		)
		.init();

	let args = Args::parse();
	let gateway = Arc::new(Gateway {
		key: keys::read32(&args.key)?,
		devices: args.devices,
		front_list: args.front_list,
		upstream: Endpoint::parse(&args.upstream)?,
		tick_ms: args.tick_ms,
		window_secs: args.window_secs,
		min_level: Level::from_u8(args.min_level)?,
	});

	match Endpoint::parse(&args.listen)? {
		| Endpoint::Unix(path) => {
			_ = fs::remove_file(&path);
			let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
			info!(path = %path.display(), "listening");
			loop {
				let (conn, _) = listener.accept().await?;
				tokio::spawn(handle(gateway.clone(), conn));
			}
		},
		| Endpoint::Tcp(addr) => {
			let listener = TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
			info!(%addr, "listening");
			loop {
				let (conn, _) = listener.accept().await?;
				tokio::spawn(handle(gateway.clone(), conn));
			}
		},
	}
}

fn allowed(devices: &Path, key: &[u8; 32]) -> bool {
	let Ok(entries) = fs::read_dir(devices) else {
		warn!(dir = %devices.display(), "cannot read device directory");
		return false;
	};
	entries
		.filter_map(Result::ok)
		.filter(|e| e.path().extension().is_some_and(|x| x == "pub"))
		.any(|e| keys::read32(&e.path()).is_ok_and(|k| &k == key))
}

fn list_version(path: Option<&Path>) -> u64 {
	path.and_then(|p| fs::read_to_string(p).ok())
		.and_then(|s| frontlist::peek_version(&s).ok())
		.unwrap_or(0)
}

async fn handle<S>(gateway: Arc<Gateway>, mut conn: S)
where
	S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
	let handshake = noise::respond(
		&mut conn,
		&gateway.key,
		|key| allowed(&gateway.devices, key),
		|hello| Welcome {
			level: Level::from_u8(hello.level)
				.unwrap_or(Level::Constant)
				.max(gateway.min_level) as u8,
			tick_ms: gateway.tick_ms,
			window_secs: gateway.window_secs,
			list_version: list_version(gateway.front_list.as_deref()),
		},
	);

	let (crypto, hello) = match tokio::time::timeout(Duration::from_secs(60), handshake).await {
		| Ok(Ok((crypto, hello, _device))) => (crypto, hello),
		| outcome => {
			// Say nothing; close after a random delay so failures look alike.
			let reason = match outcome {
				| Ok(Err(e)) => format!("{e:#}"),
				| _ => "timed out".to_owned(),
			};
			debug!(%reason, "handshake rejected");
			let wait = rand::thread_rng().gen_range(5..=30);
			tokio::time::sleep(Duration::from_secs(wait)).await;
			return;
		},
	};

	let level = Level::from_u8(hello.level).unwrap_or(Level::Constant).max(gateway.min_level);
	let policy = Policy {
		level,
		tick: Duration::from_millis(gateway.tick_ms.into()),
		window: Duration::from_secs(gateway.window_secs.into()),
		window_offset: Duration::from_secs(1),
	};
	info!(?level, "tunnel up");

	let (out_tx, out_rx) = mpsc::channel(session::OUT_QUEUE);
	let (in_tx, mut in_rx) = mpsc::channel(session::OUT_QUEUE);
	let table = session::new_table();

	let dispatcher = {
		let gateway = gateway.clone();
		let out_tx = out_tx.clone();
		tokio::spawn(async move {
			while let Some(cell) = in_rx.recv().await {
				let Some(cell) = session::deliver(&table, cell).await else {
					continue;
				};
				match cell.kind {
					| Kind::Open => {
						let data_rx = session::register(&table, cell.stream);
						open_upstream(gateway.upstream.clone(), cell.stream, out_tx.clone(), data_rx);
					},
					| Kind::ListGet => send_front_list(gateway.front_list.as_deref(), &out_tx).await,
					| _ => {},
				}
			}
		})
	};
	drop(out_tx);

	if let Err(e) = session::run(conn, crypto, policy, out_rx, in_tx).await {
		debug!("tunnel closed: {e:#}");
	}
	dispatcher.abort();
	info!("tunnel down");
}

fn open_upstream(upstream: Endpoint, id: u32, out: mpsc::Sender<Cell>, data_rx: mpsc::Receiver<Vec<u8>>) {
	tokio::spawn(async move {
		match upstream {
			| Endpoint::Unix(path) => match UnixStream::connect(&path).await {
				| Ok(sock) => session::spawn_stream(sock, id, out, data_rx),
				| Err(e) => {
					warn!("upstream connect failed: {e}");
					_ = out.send(Cell::control(Kind::Close, id)).await;
				},
			},
			| Endpoint::Tcp(addr) => match TcpStream::connect(&addr).await {
				| Ok(sock) => session::spawn_stream(sock, id, out, data_rx),
				| Err(e) => {
					warn!("upstream connect failed: {e}");
					_ = out.send(Cell::control(Kind::Close, id)).await;
				},
			},
		}
	});
}

async fn send_front_list(path: Option<&Path>, out: &mpsc::Sender<Cell>) {
	let bytes = path.and_then(|p| fs::read(p).ok()).unwrap_or_default();
	let mut index = 0_u32;
	for chunk in bytes.chunks(MAX_PAYLOAD) {
		let cell = Cell { kind: Kind::ListPart, stream: index, payload: chunk.to_vec() };
		if out.send(cell).await.is_err() {
			return;
		}
		index += 1;
	}
	// Empty part marks the end.
	_ = out.send(Cell { kind: Kind::ListPart, stream: index, payload: Vec::new() }).await;
}
