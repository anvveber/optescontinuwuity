//! A running tunnel: one reader task, one writer task applying the policy,
//! and helpers to carry local sockets as streams.

use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::Duration,
};

use anyhow::{bail, Result};
use tokio::{
	io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, WriteHalf},
	sync::mpsc::{self, error::TryRecvError},
	task::JoinHandle,
	time::{interval, sleep, MissedTickBehavior},
};

use crate::{
	cell::{Cell, Kind},
	noise::Crypto,
	policy::{burst_target, until_next_window, Level, Policy, MAX_BURST},
	FRAME, MAX_PAYLOAD,
};

/// Cells waiting to be sent. Bounded, so a slow tunnel pushes back on local
/// sockets instead of buffering without limit.
pub const OUT_QUEUE: usize = 256;
/// Per-stream buffer for data arriving from the tunnel.
pub const STREAM_QUEUE: usize = 1024;

pub type StreamTable = Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>>;

#[must_use]
pub fn new_table() -> StreamTable { Arc::new(Mutex::new(HashMap::new())) }

/// Runs until the connection fails or `out_rx` is closed. Decrypted non-pad
/// cells go to `in_tx`.
///
/// `idle_limit`: fail if no frame at all (padding included) arrives for that
/// long. Only for a side whose peer sends regularly (levels 2 and 3): it
/// notices a dead connection that never reports an error.
///
/// Dropping the returned future stops both halves of the session.
pub async fn run<S>(
	io: S,
	crypto: Crypto,
	policy: Policy,
	out_rx: mpsc::Receiver<Cell>,
	in_tx: mpsc::Sender<Cell>,
	idle_limit: Option<Duration>,
) -> Result<()>
where
	S: AsyncRead + AsyncWrite + Send + 'static,
{
	let (reader, writer) = tokio::io::split(io);
	let crypto = Arc::new(Mutex::new(crypto));

	let mut tasks = AbortOnDrop(
		tokio::spawn(read_loop(reader, crypto.clone(), in_tx, idle_limit)),
		tokio::spawn(write_loop(writer, crypto, policy, out_rx)),
	);
	let AbortOnDrop(read_task, write_task) = &mut tasks;

	let result = tokio::select! {
		r = read_task => r,
		w = write_task => w,
	};

	match result {
		| Ok(inner) => inner,
		| Err(join) if join.is_cancelled() => Ok(()),
		| Err(join) => bail!("session task panicked: {join}"),
	}
}

/// Both halves of a session; aborted together when this is dropped.
struct AbortOnDrop(JoinHandle<Result<()>>, JoinHandle<Result<()>>);

impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort();
		self.1.abort();
	}
}

async fn read_loop<R>(
	mut reader: R,
	crypto: Arc<Mutex<Crypto>>,
	in_tx: mpsc::Sender<Cell>,
	idle_limit: Option<Duration>,
) -> Result<()>
where
	R: AsyncRead + Unpin,
{
	let mut frame = [0_u8; FRAME];
	loop {
		match idle_limit {
			| Some(limit) => {
				tokio::time::timeout(limit, reader.read_exact(&mut frame))
					.await
					.map_err(|_| anyhow::anyhow!("no frame from the peer for {} s", limit.as_secs()))??;
			},
			| None => {
				reader.read_exact(&mut frame).await?;
			},
		}
		let cell = crypto.lock().expect("crypto lock").open(&frame)?;
		if cell.is_pad() {
			continue;
		}
		if in_tx.send(cell).await.is_err() {
			return Ok(());
		}
	}
}

async fn send<W: AsyncWrite>(
	writer: &mut WriteHalf<W>,
	crypto: &Mutex<Crypto>,
	cell: &Cell,
) -> Result<()> {
	let frame = crypto.lock().expect("crypto lock").seal(cell)?;
	writer.write_all(&frame).await?;
	Ok(())
}

/// Sends whatever is queued (up to [`MAX_BURST`]), then pads the burst.
/// Returns false if the queue is closed and empty.
async fn burst<W: AsyncWrite>(
	writer: &mut WriteHalf<W>,
	crypto: &Mutex<Crypto>,
	out_rx: &mut mpsc::Receiver<Cell>,
	first: Option<Cell>,
) -> Result<bool> {
	let mut sent = 0;
	let mut open = true;
	if let Some(cell) = first {
		send(writer, crypto, &cell).await?;
		sent += 1;
	}
	while sent < MAX_BURST {
		match out_rx.try_recv() {
			| Ok(cell) => {
				send(writer, crypto, &cell).await?;
				sent += 1;
			},
			| Err(TryRecvError::Empty) => break,
			| Err(TryRecvError::Disconnected) => {
				open = false;
				break;
			},
		}
	}
	for _ in sent..burst_target(sent) {
		send(writer, crypto, &Cell::pad()).await?;
	}
	writer.flush().await?;
	Ok(open || sent > 0)
}

async fn write_loop<W>(
	mut writer: WriteHalf<W>,
	crypto: Arc<Mutex<Crypto>>,
	policy: Policy,
	mut out_rx: mpsc::Receiver<Cell>,
) -> Result<()>
where
	W: AsyncWrite,
{
	match policy.level {
		| Level::Constant => {
			let mut ticks = interval(policy.tick);
			ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
			loop {
				ticks.tick().await;
				let cell = match out_rx.try_recv() {
					| Ok(cell) => cell,
					| Err(TryRecvError::Empty) => Cell::pad(),
					| Err(TryRecvError::Disconnected) => return Ok(()),
				};
				send(&mut writer, &crypto, &cell).await?;
				writer.flush().await?;
			}
		},
		| Level::Burst => loop {
			let Some(first) = out_rx.recv().await else {
				return Ok(());
			};
			if !burst(&mut writer, &crypto, &mut out_rx, Some(first)).await? {
				return Ok(());
			}
		},
		| Level::Window => loop {
			let now = now_ms().saturating_add_signed(policy.clock_offset_ms);
			sleep(until_next_window(now, policy.window, policy.window_offset)).await;
			if !burst(&mut writer, &crypto, &mut out_rx, None).await? {
				return Ok(());
			}
		},
	}
}

#[must_use]
pub fn now_ms() -> u64 {
	u64::try_from(
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or(Duration::ZERO)
			.as_millis(),
	)
	.unwrap_or(u64::MAX)
}

/// Carries a local socket as stream `id`: bytes read from the socket become
/// DATA cells, local EOF becomes CLOSE; bytes from `data_rx` are written to
/// the socket, and the socket's write side is shut down when `data_rx` ends.
pub fn spawn_stream<T>(socket: T, id: u32, out: mpsc::Sender<Cell>, mut data_rx: mpsc::Receiver<Vec<u8>>)
where
	T: AsyncRead + AsyncWrite + Send + 'static,
{
	let (mut rd, mut wr) = tokio::io::split(socket);

	tokio::spawn(async move {
		let mut buf = vec![0_u8; MAX_PAYLOAD];
		loop {
			match rd.read(&mut buf).await {
				| Ok(0) | Err(_) => break,
				| Ok(n) =>
					if out.send(Cell::data(id, buf[..n].to_vec())).await.is_err() {
						return;
					},
			}
		}
		_ = out.send(Cell::control(Kind::Close, id)).await;
	});

	tokio::spawn(async move {
		while let Some(bytes) = data_rx.recv().await {
			if wr.write_all(&bytes).await.is_err() {
				break;
			}
		}
		_ = wr.shutdown().await;
	});
}

/// Registers stream `id` and returns the receiver for its incoming data.
#[must_use]
pub fn register(table: &StreamTable, id: u32) -> mpsc::Receiver<Vec<u8>> {
	let (tx, rx) = mpsc::channel(STREAM_QUEUE);
	table.lock().expect("table lock").insert(id, tx);
	rx
}

/// Handles DATA and CLOSE for registered streams. Returns the cell back if
/// it is some other kind.
pub async fn deliver(table: &StreamTable, cell: Cell) -> Option<Cell> {
	match cell.kind {
		| Kind::Data => {
			let tx = table.lock().expect("table lock").get(&cell.stream).cloned();
			if let Some(tx) = tx {
				_ = tx.send(cell.payload).await;
			}
			None
		},
		| Kind::Close => {
			table.lock().expect("table lock").remove(&cell.stream);
			None
		},
		| _ => Some(cell),
	}
}

#[cfg(test)]
mod tests {
	use std::time::Instant;

	use tokio::io::AsyncReadExt;

	use super::*;
	use crate::{keys::generate_x25519, noise};

	/// Two ends of an established tunnel over an in-memory pipe.
	async fn tunnel() -> (Crypto, tokio::io::DuplexStream, Crypto, tokio::io::DuplexStream) {
		let (gw_priv, gw_pub) = generate_x25519().unwrap();
		let (dev_priv, _) = generate_x25519().unwrap();
		let (mut a, mut b) = tokio::io::duplex(1 << 16);
		let server = tokio::spawn(async move {
			let (c, _, _) = noise::respond(&mut b, &gw_priv, |_, _| true, |h| noise::Welcome {
				level: h.level,
				tick_ms: 200,
				window_secs: 15,
				list_version: 0,
				gateway_time_ms: 0,
			})
			.await
			.unwrap();
			(c, b)
		});
		let hello = noise::Hello { level: 1, list_version: 0, counter: 1 };
		let (client, _) = noise::initiate(&mut a, &dev_priv, &gw_pub, hello).await.unwrap();
		let (server, b) = server.await.unwrap();
		(client, a, server, b)
	}

	fn burst_policy() -> Policy {
		Policy {
			level: Level::Burst,
			tick: Duration::from_millis(200),
			window: Duration::from_secs(15),
			window_offset: Duration::ZERO,
			clock_offset_ms: 0,
		}
	}

	#[tokio::test]
	async fn silent_peer_ends_the_session_after_the_idle_limit() {
		let (client, a, _server, _b) = tunnel().await;
		let (_out_tx, out_rx) = mpsc::channel(8);
		let (in_tx, _in_rx) = mpsc::channel(8);
		let started = Instant::now();
		let result = tokio::time::timeout(
			Duration::from_secs(10),
			run(a, client, burst_policy(), out_rx, in_tx, Some(Duration::from_secs(1))),
		)
		.await
		.expect("the session must end by itself");
		assert!(result.is_err(), "a silent peer is an error");
		assert!(started.elapsed() < Duration::from_secs(3));
	}

	#[tokio::test]
	async fn dropping_the_session_closes_the_connection() {
		let (client, a, _server, mut b) = tunnel().await;
		let (_out_tx, out_rx) = mpsc::channel(8);
		let (in_tx, _in_rx) = mpsc::channel(8);
		tokio::select! {
			_ = run(a, client, burst_policy(), out_rx, in_tx, None) => panic!("ended early"),
			() = sleep(Duration::from_millis(200)) => {},
		}
		// Both halves are gone, so the peer sees the end of the stream.
		let mut buf = [0_u8; 1];
		let n = tokio::time::timeout(Duration::from_secs(2), b.read(&mut buf))
			.await
			.expect("the connection must be closed")
			.unwrap();
		assert_eq!(n, 0);
	}
}
