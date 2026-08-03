#![cfg(feature = "direct_tls")]

use std::{io, net::SocketAddr, sync::Arc};

use axum::{Router, extract::connect_info::IntoMakeServiceWithConnectInfo};
use axum_server::{Handle as ServerHandle, from_tcp_rustls};
use axum_server_dual_protocol::{ServerExt, axum_server::tls_rustls::RustlsConfig};
use conduwuit::{Result, Server, err};
use tokio::{net::TcpListener, runtime, task::JoinSet};
use tracing::{debug, info, warn};

pub(super) async fn prepare(server: &Arc<Server>) -> Result<(RustlsConfig, bool)> {
	let tls = &server.config.tls;
	let certs = tls.certs.as_ref().ok_or_else(|| {
		err!(Config("tls.certs", "Missing required value in tls config section"))
	})?;
	let key = tls
		.key
		.as_ref()
		.ok_or_else(|| err!(Config("tls.key", "Missing required value in tls config section")))?;
	info!(
		"Note: It is strongly recommended that you use a reverse proxy instead of running \
		 conduwuit directly with TLS."
	);
	debug!("Using direct TLS. Certificate path {certs} and certificate private key path {key}",);
	let conf = RustlsConfig::from_pem_file(certs, key)
		.await
		.map_err(|e| err!(Config("tls", "Failed to load certificates or key: {e}")))?;

	Ok((conf, tls.dual_protocol))
}

pub(super) fn spawn(
	listener: TcpListener,
	app: IntoMakeServiceWithConnectInfo<Router, SocketAddr>,
	server_handle: ServerHandle<SocketAddr>,
	join_set: &mut JoinSet<io::Result<()>>,
	runtime: &runtime::Handle,
	config: RustlsConfig,
	dual_protocol: bool,
) {
	let listener = listener.into_std().expect("should convert");
	let addr = listener.local_addr().expect("should have a local addr");

	if dual_protocol {
		join_set.spawn_on(
			axum_server_dual_protocol::from_tcp_dual_protocol(listener, config)
				.expect("should convert")
				.set_upgrade(false)
				.handle(server_handle)
				.serve(app),
			runtime,
		);
	} else {
		join_set.spawn_on(
			from_tcp_rustls(listener, config)
				.expect("should convert")
				.handle(server_handle)
				.serve(app),
			runtime,
		);
	}

	if dual_protocol {
		warn!(
			"Listening on {addr} with TLS and supporting plain text (HTTP) connections too \
			 (insecure!)",
		);
	} else {
		info!("Listening on {addr} with TLS");
	}
}
