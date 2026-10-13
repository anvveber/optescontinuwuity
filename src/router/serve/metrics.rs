use std::{sync::Arc, time::Duration};

use axum::{Router, extract::State, response::IntoResponse, routing::get};
use axum_server::{Handle as ServerHandle, bind};
use conduwuit::{
	Result, err, error, info, metrics::PROMETHEUS_CONTENT_TYPE, utils::string::constant_time_eq,
};
use conduwuit_service::Services;
use http::{HeaderMap, StatusCode, header};
use tokio::sync::broadcast;

pub(super) async fn serve(
	services: &Arc<Services>,
	mut shutdown: broadcast::Receiver<()>,
) -> Result {
	let server = &services.server;
	let config = &server.config;
	if !config.allow_prometheus_metrics {
		return Ok(());
	}

	let addr = config.prometheus_metrics_address;
	let app = Router::new()
		.route("/metrics", get(scrape))
		.with_state(services.clone())
		.into_make_service();

	let handle = ServerHandle::new();
	let mut task = server
		.runtime()
		.spawn(bind(addr).handle(handle.clone()).serve(app));

	info!("Serving metrics on {addr}");
	tokio::select! {
		result = &mut task => result,
		_ = shutdown.recv() => {
			handle.graceful_shutdown(Some(Duration::from_secs(config.client_shutdown_timeout)));
			task.await
		},
	}
	.map_err(|e| err!(error!("metrics listener task failed: {e}")))?
	.map_err(|e| err!(Config("prometheus_metrics_address", "{e}")))
}

async fn scrape(
	State(services): State<Arc<Services>>,
	headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
	if let Some(token) = &services.globals.prometheus_metrics_token {
		let authorized = headers
			.get(header::AUTHORIZATION)
			.and_then(|value| value.to_str().ok())
			.and_then(|value| value.split_once(' '))
			.is_some_and(|(scheme, provided)| {
				scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(provided, token)
			});

		if !authorized {
			return Err(StatusCode::UNAUTHORIZED);
		}
	}

	let body = services.server.metrics.render().map_err(|e| {
		error!("Failed to render metrics: {e}");
		StatusCode::INTERNAL_SERVER_ERROR
	})?;

	Ok(([(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], body))
}
