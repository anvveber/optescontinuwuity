//! Noise IK handshake and transport encryption. Every message is one
//! [`FRAME`].

use anyhow::{bail, ensure, Context, Result};
use snow::{Builder, HandshakeState, TransportState};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{cell::Cell, CELL, FRAME, NOISE_PATTERN, PROLOGUE, PROTOCOL_VERSION, TAG};

/// IK message 1 = ephemeral (32) + encrypted static (32 + tag) + payload + tag.
const HELLO_PAYLOAD: usize = FRAME - 32 - (32 + TAG) - TAG;
/// IK message 2 = ephemeral (32) + payload + tag.
const WELCOME_PAYLOAD: usize = FRAME - 32 - TAG;

/// Rekey each direction after this many frames.
const REKEY_EVERY: u64 = 1 << 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hello {
	pub level: u8,
	pub list_version: u64,
}

impl Hello {
	fn encode(&self) -> [u8; HELLO_PAYLOAD] {
		let mut out = [0_u8; HELLO_PAYLOAD];
		out[0] = PROTOCOL_VERSION;
		out[1] = self.level;
		out[2..10].copy_from_slice(&self.list_version.to_be_bytes());
		out
	}

	fn decode(buf: &[u8]) -> Result<Self> {
		ensure!(buf.len() == HELLO_PAYLOAD, "hello payload has wrong size");
		ensure!(buf[0] == PROTOCOL_VERSION, "unsupported protocol version {}", buf[0]);
		Ok(Self { level: buf[1], list_version: u64::from_be_bytes(buf[2..10].try_into()?) })
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Welcome {
	pub level: u8,
	pub tick_ms: u32,
	pub window_secs: u32,
	pub list_version: u64,
}

impl Welcome {
	fn encode(&self) -> [u8; WELCOME_PAYLOAD] {
		let mut out = [0_u8; WELCOME_PAYLOAD];
		out[0] = PROTOCOL_VERSION;
		out[1] = self.level;
		out[2..6].copy_from_slice(&self.tick_ms.to_be_bytes());
		out[6..10].copy_from_slice(&self.window_secs.to_be_bytes());
		out[10..18].copy_from_slice(&self.list_version.to_be_bytes());
		out
	}

	fn decode(buf: &[u8]) -> Result<Self> {
		ensure!(buf.len() == WELCOME_PAYLOAD, "welcome payload has wrong size");
		ensure!(buf[0] == PROTOCOL_VERSION, "unsupported protocol version {}", buf[0]);
		Ok(Self {
			level: buf[1],
			tick_ms: u32::from_be_bytes(buf[2..6].try_into()?),
			window_secs: u32::from_be_bytes(buf[6..10].try_into()?),
			list_version: u64::from_be_bytes(buf[10..18].try_into()?),
		})
	}
}

/// Transport encryption for an established session.
pub struct Crypto {
	state: TransportState,
	sent: u64,
	received: u64,
}

impl Crypto {
	fn new(state: TransportState) -> Self { Self { state, sent: 0, received: 0 } }

	/// Encrypts a cell into one frame.
	pub fn seal(&mut self, cell: &Cell) -> Result<[u8; FRAME]> {
		let plain = cell.encode();
		let mut out = [0_u8; FRAME];
		let n = self.state.write_message(&plain, &mut out)?;
		ensure!(n == FRAME, "sealed frame has wrong size {n}");
		self.sent += 1;
		if self.sent.is_multiple_of(REKEY_EVERY) {
			self.state.rekey_outgoing();
		}
		Ok(out)
	}

	/// Decrypts one frame into a cell.
	pub fn open(&mut self, frame: &[u8; FRAME]) -> Result<Cell> {
		let mut plain = [0_u8; FRAME];
		let n = self
			.state
			.read_message(frame, &mut plain)
			.context("frame failed to decrypt")?;
		ensure!(n == CELL, "opened cell has wrong size {n}");
		self.received += 1;
		if self.received.is_multiple_of(REKEY_EVERY) {
			self.state.rekey_incoming();
		}
		Cell::decode(&plain[..n])
	}
}

fn builder<'a>() -> Result<Builder<'a>> {
	Ok(Builder::new(NOISE_PATTERN.parse()?).prologue(PROLOGUE))
}

async fn read_frame<S: AsyncRead + Unpin>(io: &mut S) -> Result<[u8; FRAME]> {
	let mut buf = [0_u8; FRAME];
	io.read_exact(&mut buf).await?;
	Ok(buf)
}

/// Relay side.
pub async fn initiate<S>(
	io: &mut S,
	device_private: &[u8; 32],
	gateway_public: &[u8; 32],
	hello: Hello,
) -> Result<(Crypto, Welcome)>
where
	S: AsyncRead + AsyncWrite + Unpin,
{
	let mut hs: HandshakeState = builder()?
		.local_private_key(device_private)
		.remote_public_key(gateway_public)
		.build_initiator()?;

	let mut msg = [0_u8; FRAME];
	let n = hs.write_message(&hello.encode(), &mut msg)?;
	ensure!(n == FRAME, "handshake message 1 has wrong size {n}");
	io.write_all(&msg).await?;
	io.flush().await?;

	let reply = read_frame(io).await.context("reading handshake reply")?;
	let mut payload = [0_u8; FRAME];
	let n = hs
		.read_message(&reply, &mut payload)
		.context("handshake reply failed to decrypt (wrong gateway key?)")?;
	let welcome = Welcome::decode(&payload[..n])?;

	Ok((Crypto::new(hs.into_transport_mode()?), welcome))
}

/// Gateway side. `allowed` decides whether a device key may connect;
/// `welcome` builds the reply from the client's hello.
///
/// On any failure nothing is written; the caller decides how to close.
pub async fn respond<S, A, W>(
	io: &mut S,
	gateway_private: &[u8; 32],
	allowed: A,
	welcome: W,
) -> Result<(Crypto, Hello, [u8; 32])>
where
	S: AsyncRead + AsyncWrite + Unpin,
	A: FnOnce(&[u8; 32]) -> bool,
	W: FnOnce(&Hello) -> Welcome,
{
	let mut hs: HandshakeState = builder()?
		.local_private_key(gateway_private)
		.build_responder()?;

	let msg = read_frame(io).await?;
	let mut payload = [0_u8; FRAME];
	let n = hs.read_message(&msg, &mut payload)?;
	let hello = Hello::decode(&payload[..n])?;

	let device: [u8; 32] = hs
		.get_remote_static()
		.context("no remote static key")?
		.try_into()?;
	if !allowed(&device) {
		bail!("device key not allowed");
	}

	let mut reply = [0_u8; FRAME];
	let n = hs.write_message(&welcome(&hello).encode(), &mut reply)?;
	ensure!(n == FRAME, "handshake message 2 has wrong size {n}");
	io.write_all(&reply).await?;
	io.flush().await?;

	Ok((Crypto::new(hs.into_transport_mode()?), hello, device))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::keys::generate_x25519;

	fn welcome_for(hello: &Hello) -> Welcome {
		Welcome { level: hello.level, tick_ms: 200, window_secs: 15, list_version: 9 }
	}

	#[tokio::test]
	async fn handshake_and_transport() {
		let (gw_priv, gw_pub) = generate_x25519().unwrap();
		let (dev_priv, dev_pub) = generate_x25519().unwrap();
		let (mut a, mut b) = tokio::io::duplex(1 << 16);

		let server = tokio::spawn(async move {
			respond(&mut b, &gw_priv, |k| *k == dev_pub, welcome_for)
				.await
				.map(|(c, h, d)| (c, h, d, b))
		});
		let hello = Hello { level: 3, list_version: 4 };
		let (mut client_crypto, welcome) = initiate(&mut a, &dev_priv, &gw_pub, hello).await.unwrap();
		let (mut server_crypto, got_hello, device, _b) = server.await.unwrap().unwrap();

		assert_eq!(got_hello, hello);
		assert_eq!(device, dev_pub);
		assert_eq!(welcome, welcome_for(&hello));

		for i in 0..3 {
			let cell = Cell::data(1, vec![i; 100]);
			let frame = client_crypto.seal(&cell).unwrap();
			assert_eq!(server_crypto.open(&frame).unwrap(), cell);
		}
	}

	#[tokio::test]
	async fn unknown_device_gets_no_reply() {
		let (gw_priv, gw_pub) = generate_x25519().unwrap();
		let (dev_priv, _) = generate_x25519().unwrap();
		let (mut a, mut b) = tokio::io::duplex(1 << 16);

		let server = tokio::spawn(async move {
			let res = respond(&mut b, &gw_priv, |_| false, welcome_for).await;
			drop(b);
			res.is_err()
		});
		let res = initiate(&mut a, &dev_priv, &gw_pub, Hello { level: 1, list_version: 0 }).await;
		assert!(res.is_err(), "client must not complete a handshake");
		assert!(server.await.unwrap(), "server must reject");
	}

	#[tokio::test]
	async fn wrong_gateway_key_fails() {
		let (gw_priv, _) = generate_x25519().unwrap();
		let (_, other_pub) = generate_x25519().unwrap();
		let (dev_priv, _) = generate_x25519().unwrap();
		let (mut a, mut b) = tokio::io::duplex(1 << 16);

		let server = tokio::spawn(async move {
			let res = respond(&mut b, &gw_priv, |_| true, welcome_for).await;
			drop(b);
			res.is_err()
		});
		let res = initiate(&mut a, &dev_priv, &other_pub, Hello { level: 1, list_version: 0 }).await;
		assert!(res.is_err());
		assert!(server.await.unwrap());
	}

	#[test]
	fn rekey_keeps_working() {
		let (gw_priv, gw_pub) = generate_x25519().unwrap();
		let (dev_priv, _) = generate_x25519().unwrap();
		let mut init = builder()
			.unwrap()
			.local_private_key(&dev_priv)
			.remote_public_key(&gw_pub)
			.build_initiator()
			.unwrap();
		let mut resp = builder().unwrap().local_private_key(&gw_priv).build_responder().unwrap();
		let mut m = [0_u8; FRAME];
		let mut p = [0_u8; FRAME];
		let n = init.write_message(&[], &mut m).unwrap();
		resp.read_message(&m[..n], &mut p).unwrap();
		let n = resp.write_message(&[], &mut m).unwrap();
		init.read_message(&m[..n], &mut p).unwrap();
		let mut a = Crypto::new(init.into_transport_mode().unwrap());
		let mut b = Crypto::new(resp.into_transport_mode().unwrap());
		a.sent = REKEY_EVERY - 2;
		b.received = REKEY_EVERY - 2;
		for i in 0..5_u8 {
			let cell = Cell::data(1, vec![i]);
			let frame = a.seal(&cell).unwrap();
			assert_eq!(b.open(&frame).unwrap(), cell, "frame {i}");
		}
	}
}
