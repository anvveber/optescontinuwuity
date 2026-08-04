use std::{
	net::SocketAddr,
	os::fd::{FromRawFd, IntoRawFd, OwnedFd},
	path::PathBuf,
};

use axum_server::AddrListener;
use conduwuit::{info, warn};
use socket2::{Domain, Socket, Type};
use tokio::net::{TcpListener, UnixListener};

use crate::serve::Listener;

/// Given a file descriptor, classify it and convert to a `TcpListener`,
/// `UnixListener`, or neither (in which case the fd is returned).
fn classify_fd(fd: OwnedFd) -> std::io::Result<Result<Listener, OwnedFd>> {
	// This is safe because if the fd is not in fact a socket,
	// the methods below will return ENOTSOCK.
	let socket = Socket::from(fd);

	let Ok(socket_type) = socket.r#type() else {
		return Ok(Err(socket.into()));
	};

	let Ok(listening) = socket.is_listener() else {
		return Ok(Err(socket.into()));
	};

	if !listening || socket_type != Type::STREAM {
		return Ok(Err(socket.into()));
	}

	let Ok(domain) = socket.domain() else {
		return Ok(Err(socket.into()));
	};

	match domain {
		| Domain::IPV4 | Domain::IPV6 => {
			let std: std::net::TcpListener = socket.into();
			std.set_nonblocking(true)?;

			Ok(Ok(Listener::Tcp(TcpListener::from_std(std)?)))
		},
		| Domain::UNIX => {
			let std: std::os::unix::net::UnixListener = socket.into();
			std.set_nonblocking(true)?;

			Ok(Ok(Listener::Unix(UnixListener::from_std(std)?)))
		},
		| _ => Ok(Err(socket.into())),
	}
}

/// Classify FDs passed by systemd for socket activation, and convert them into
/// appropriate types of Listener.
fn get_listen_fds() -> Vec<Listener> {
	let Ok(fds) = sd_notify::listen_fds() else {
		return vec![];
	};

	let mut listeners = Vec::with_capacity(fds.len());

	for fd in fds {
		// SAFETY: The fd is open, because it was obtained from `sd_listen_fds`, and
		// this is the only callsite.
		let fd = unsafe { OwnedFd::from_raw_fd(fd) };

		match classify_fd(fd) {
			| Ok(Ok(listener)) => listeners.push(listener),
			| Ok(Err(fd)) => info!(
				fd = fd.into_raw_fd(),
				"FD obtained from sd_listen_fds is not a socket, ignoring"
			),
			| Err(e) => warn!(?e, "Failed to classify FD obtained from sd_listen_fds"),
		}
	}

	listeners
}

pub(super) fn populate_listeners_from_socket_activation(
	listen_sockets: &mut Vec<Listener>,
	bound_ip_addrs: &mut Vec<SocketAddr>,
	bound_unix_socket_paths: &mut Vec<PathBuf>,
) {
	for listener in get_listen_fds() {
		match listener {
			| Listener::Tcp(listener) => {
				if let Ok(addr) = listener.get_local_addr() {
					bound_ip_addrs.push(addr);
				}

				listen_sockets.push(Listener::Tcp(listener));
			},
			| Listener::Unix(listener) => {
				if let Ok(addr) = listener.get_local_addr()
					&& let Some(path) = addr.as_pathname()
				{
					bound_unix_socket_paths.push(path.to_path_buf());
				}

				listen_sockets.push(Listener::Unix(listener));
			},
		}
	}
}
