mod tls;
mod unix;

#[cfg(all(target_os = "linux", feature = "systemd"))]
mod fd;

use std::{
	io,
	net::SocketAddr,
	sync::{Arc, atomic::Ordering},
	time::Duration,
};

use axum::Router;
use axum_server::{Handle as ServerHandle, from_tcp, from_unix};
use conduwuit::{Err, Result, Server, config::Manager, debug_info, err, info, warn};
use conduwuit_service::Services;
use tokio::{
	net::{TcpListener, UnixListener},
	sync::broadcast,
	task::JoinSet,
};

use super::layers;

type RustlsConfig = cfg_select! {
	feature = "direct_tls" => axum_server::tls_rustls::RustlsConfig,
	_ => (),
};

/// Serve clients
pub(super) async fn serve(
	services: Arc<Services>,
	mut shutdown_rx: broadcast::Receiver<Duration>,
) -> Result<()> {
	let server = &services.server;
	let config = &server.config;

	if !config.listening {
		let _ = shutdown_rx
			.recv()
			.await
			.map_err(|e| err!(error!("channel error: {e}")))?;
		return Ok(());
	}

	// Fail early if TLS is configured but unsupported
	let should_use_tls = should_use_tls(server)?;

	let mut listeners = Vec::new();

	// Track which addrs/paths we have so we don't bind duplicates
	let mut bound_ip_addrs = Vec::new();
	#[cfg(all(target_os = "linux", feature = "systemd"))]
	let mut bound_unix_socket_paths = Vec::new();

	#[cfg(all(target_os = "linux", feature = "systemd"))]
	if config.use_socket_activation || config.use_exclusively_socket_activation {
		fd::populate_listeners_from_socket_activation(
			&mut listeners,
			&mut bound_ip_addrs,
			&mut bound_unix_socket_paths,
		);

		if config.use_exclusively_socket_activation && listeners.is_empty() {
			return Err!(error!(
				"Socket activation provided no usable listening sockets, and \
				 `use_exclusively_socket_activation` is enabled"
			));
		}
	}

	if !config.use_exclusively_socket_activation {
		populate_tcp_listeners(config, &mut listeners, &mut bound_ip_addrs).await?;

		if cfg!(unix)
			&& let Some(ref path) = config.unix_socket_path
		{
			#[cfg(all(target_os = "linux", feature = "systemd"))]
			if bound_unix_socket_paths.contains(path) {
				info!(path = ?path.display(), "Unix socket path already bound, not binding again");
			}

			match unix::bind_unix(path, config.unix_socket_perms.to_string()).await {
				| Ok(sock) => {
					listeners.push(Listener::Unix(sock));
				},
				| Err(e) => {
					return Err!(warn!(path = %path.display(), ?e, "Failed to bind Unix socket"));
				},
			}
		}
	}

	if listeners.is_empty() {
		return Err!(error!("No listening addresses configured!"));
	}

	// Cache TLS config for reuse if listening on multiple addresses
	let tls_conf_cache = if should_use_tls {
		cfg_select! {
			feature = "direct_tls" => Some(tls::prepare(server).await?),
			_ => None,
		}
	} else {
		None
	};

	let (app, _guard) = layers::build(&services)?;

	let mut join_set = JoinSet::new();

	for listener in listeners {
		spawn_listener_task(
			listener,
			server,
			shutdown_rx.resubscribe(),
			app.clone(),
			&mut join_set,
			tls_conf_cache.as_ref(),
		);
	}

	while join_set.join_next().await.is_some() {}

	let handle_active = server
		.metrics
		.requests_handle_active
		.load(Ordering::Relaxed);
	debug_info!(
		handle_finished = server
			.metrics
			.requests_handle_finished
			.load(Ordering::Relaxed),
		panics = server.metrics.requests_panic.load(Ordering::Relaxed),
		handle_active,
		"Stopped listening",
	);

	debug_assert!(handle_active == 0, "active request handles still pending");

	Ok(())
}

enum Listener {
	Tcp(TcpListener),
	Unix(UnixListener),
}

async fn graceful_shutdown_handler<A: axum_server::Address>(
	mut shutdown_rx: broadcast::Receiver<Duration>,
	handle: ServerHandle<A>,
) {
	if let Ok(duration) = shutdown_rx.recv().await {
		debug_info!("Attempting graceful shutdown");
		handle.graceful_shutdown(Some(duration));
	} else {
		warn!("Failed to wait on shutdown broadcast");
	}
}

/// Determine whether TLS should be used for TCP connections, or returns an
/// error if the config enables TLS but the `direct_tls` feature is missing.
fn should_use_tls(server: &Arc<Server>) -> Result<bool> {
	if server.config.tls.certs.is_some() {
		return cfg_select! {
			feature = "direct_tls" => Ok(true),
			_ => Err!(Config(
				"tls",
				"continuwuity was not built with direct TLS support (\"direct_tls\")"
			)),
		};
	}

	Ok(false)
}

/// Bind listeners from the `address` and `port` configuration options.
async fn populate_tcp_listeners(
	config: &Manager,
	listeners: &mut Vec<Listener>,
	bound_ip_addrs: &mut Vec<SocketAddr>,
) -> Result<()> {
	for addr in config.get_bind_addrs() {
		if bound_ip_addrs.contains(&addr) {
			info!(%addr, "IP address already bound, not binding again");
			continue;
		}

		match TcpListener::bind(addr).await {
			| Ok(sock) => {
				listeners.push(Listener::Tcp(sock));
				bound_ip_addrs.push(addr);
			},
			| Err(e) => {
				return Err!(warn!(%addr, ?e, "Failed to bind TCP socket"));
			},
		}
	}

	Ok(())
}

/// Spawn the task to serve the router `app` on the given `listener`.
fn spawn_listener_task(
	listener: Listener,
	server: &Server,
	shutdown_rx: broadcast::Receiver<Duration>,
	app: Router,
	join_set: &mut JoinSet<io::Result<()>>,
	tls_conf_cache: Option<&(RustlsConfig, bool)>,
) {
	match listener {
		| Listener::Tcp(tcp_listener) => {
			let server_handle = ServerHandle::new();
			server
				.runtime()
				.spawn(graceful_shutdown_handler(shutdown_rx, server_handle.clone()));

			if cfg!(feature = "direct_tls")
				&& let Some((tls_config, tls_dual_protocol)) = tls_conf_cache
			{
				#[cfg(feature = "direct_tls")]
				tls::spawn(
					tcp_listener,
					app.into_make_service_with_connect_info(),
					server_handle,
					join_set,
					server.runtime(),
					tls_config.clone(),
					*tls_dual_protocol,
				);
			} else {
				let addr = tcp_listener
					.local_addr()
					.map_or_else(|_| "Unknown".into(), |addr| addr.to_string());

				join_set.spawn_on(
					from_tcp(tcp_listener.into_std().expect("should convert"))
						.expect("should convert")
						.handle(server_handle)
						.serve(app.into_make_service_with_connect_info::<SocketAddr>()),
					server.runtime(),
				);

				info!("Listening on {addr}");
			}
		},
		| Listener::Unix(unix_listener) => {
			let server_handle = ServerHandle::new();
			server.runtime().spawn(graceful_shutdown_handler(
				shutdown_rx.resubscribe(),
				server_handle.clone(),
			));

			let addr = unix_listener
				.local_addr()
				.map_or_else(|_| "Unknown".into(), |addr| format!("{addr:?}"));

			join_set.spawn_on(
				from_unix(unix_listener.into_std().expect("should convert"))
					.expect("should convert")
					.handle(server_handle)
					.serve(app.into_make_service()),
				server.runtime(),
			);

			info!("Listening on {addr}");
		},
	}
}
