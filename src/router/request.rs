use std::{
	fmt::Debug,
	sync::{Arc, atomic::Ordering},
	time::Duration,
};

use axum::{
	extract::State,
	response::{IntoResponse, Response},
};
use conduwuit::{Result, debug, debug_error, debug_warn, err, error, trace};
use conduwuit_service::Services;
use futures::FutureExt;
use http::{Method, StatusCode, Uri};
use tokio::time::sleep;
use tracing::Span;

/// `restrict_request_hosts` and `allowed_origins`: refuses requests that a
/// website made a browser send to this server.
///
/// - Host: with `restrict_request_hosts`, the `Host` header must be an IP
///   address, `localhost`, a `.onion` / `.i2p` name or listed in
///   `allowed_hosts`. A DNS-rebinding page sends its own domain there.
/// - Origin: with `allowed_origins` set, a request carrying an `Origin`
///   header is refused unless the origin is listed or is this server's own
///   address (same-origin). Preflights are refused the same way, so they get
///   no CORS headers.
pub(crate) async fn guard(
	State(services): State<Arc<Services>>,
	req: http::Request<axum::body::Body>,
	next: axum::middleware::Next,
) -> Response {
	let config = &services.server.config;
	if !config.restrict_request_hosts && config.allowed_origins.is_none() {
		return next.run(req).await;
	}

	// HTTP/1 has the Host header; HTTP/2 has the authority in the URI.
	let authority = req
		.headers()
		.get(http::header::HOST)
		.and_then(|value| value.to_str().ok())
		.or_else(|| req.uri().authority().map(http::uri::Authority::as_str))
		.map(str::to_ascii_lowercase);

	if config.restrict_request_hosts
		&& !authority
			.as_deref()
			.is_some_and(|authority| host_allowed(authority, &config.allowed_hosts))
	{
		debug_warn!(host = ?authority, "request refused: Host not allowed");
		return refuse("This server does not answer requests for this host name.");
	}

	if let Some(allowed) = &config.allowed_origins
		&& let Some(origin) = req.headers().get(http::header::ORIGIN)
	{
		let origin = origin.to_str().unwrap_or("").to_ascii_lowercase();
		let same_origin = authority.as_deref().is_some_and(|authority| {
			origin
				.strip_prefix("http://")
				.or_else(|| origin.strip_prefix("https://"))
				.is_some_and(|rest| rest == authority)
		});
		let listed = allowed
			.iter()
			.any(|allowed| allowed.eq_ignore_ascii_case(&origin));
		if origin.is_empty() || !(same_origin || listed) {
			debug_warn!(%origin, "request refused: Origin not allowed");
			return refuse(
				"This origin may not use this server. The server admin can add it to \
				 allowed_origins.",
			);
		}
	}

	next.run(req).await
}

/// Whether `authority` (lower case, `host` or `host:port`) names a host the
/// server answers for under `restrict_request_hosts`.
fn host_allowed(authority: &str, extra: &[String]) -> bool {
	let Ok(parsed) = authority.parse::<http::uri::Authority>() else {
		return false;
	};
	let host = parsed.host();
	let bare = host.trim_start_matches('[').trim_end_matches(']');

	bare.parse::<std::net::IpAddr>().is_ok()
		|| host == "localhost"
		|| host.ends_with(".onion")
		|| host.ends_with(".i2p")
		|| extra.iter().any(|name| name.eq_ignore_ascii_case(host))
}

fn refuse(message: &str) -> Response {
	let body = serde_json::json!({ "errcode": "M_FORBIDDEN", "error": message }).to_string();
	(
		StatusCode::FORBIDDEN,
		[(http::header::CONTENT_TYPE, "application/json")],
		body,
	)
		.into_response()
}

#[cfg(test)]
mod guard_tests {
	use super::host_allowed;

	#[test]
	fn local_and_hidden_service_hosts() {
		for host in [
			"127.0.0.1:8008",
			"127.0.0.1",
			"10.8.0.1:8008",
			"[::1]:8008",
			"localhost:8008",
			"localhost",
			"abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion",
			"example.b32.i2p:443",
		] {
			assert!(host_allowed(host, &[]), "{host}");
		}
	}

	#[test]
	fn dns_names_are_refused_unless_listed() {
		for host in [
			"evil.example:8008",
			"evil.example",
			"127.0.0.1.evil.example:8008",
			"localhost.evil.example",
			"notlocalhost:8008",
			"onion",
			"evil.onion.example",
			"",
			"a b",
		] {
			assert!(!host_allowed(host, &[]), "{host}");
		}
		assert!(host_allowed("matrix.example.org:8448", &["matrix.example.org".to_owned()]));
		assert!(!host_allowed("other.example.org", &["matrix.example.org".to_owned()]));
	}
}

#[tracing::instrument(name = "request", level = "debug", skip_all)]
pub(crate) async fn handle(
	State(services): State<Arc<Services>>,
	req: http::Request<axum::body::Body>,
	next: axum::middleware::Next,
) -> Result<Response, StatusCode> {
	if !services.server.running() {
		debug_warn!(
			method = %req.method(),
			uri = %req.uri(),
			"unavailable pending shutdown"
		);

		return Err(StatusCode::SERVICE_UNAVAILABLE);
	}

	let uri = req.uri().clone();
	let method = req.method().clone();
	let services_ = services.clone();
	let parent = Span::current();
	let task = services.server.runtime().spawn(async move {
		tokio::select! {
			response = execute(&services_, req, next, &parent) => response,
			response = services_.server.until_shutdown()
				.then(|()| {
					let timeout = services_.server.config.client_shutdown_timeout;
					let timeout = Duration::from_secs(timeout);
					sleep(timeout)
				})
				.map(|()| StatusCode::SERVICE_UNAVAILABLE)
				.map(IntoResponse::into_response) => response,
		}
	});

	task.await
		.map_err(unhandled)
		.and_then(move |result| handle_result(&method, &uri, result))
}

#[tracing::instrument(
	name = "handle",
	level = "debug",
	parent = parent,
	skip_all,
	fields(
		active = %services
			.server
			.metrics
			.requests_handle_active
			.fetch_add(1, Ordering::Relaxed),
		handled = %services
			.server
			.metrics
			.requests_handle_finished
			.load(Ordering::Relaxed),
	)
)]
async fn execute(
	// we made a safety contract that Services will not go out of scope
	// during the request; this ensures a reference is accounted for at
	// the base frame of the task regardless of its detachment.
	services: &Arc<Services>,
	req: http::Request<axum::body::Body>,
	next: axum::middleware::Next,
	parent: &Span,
) -> Response {
	#[cfg(debug_assertions)]
	conduwuit::defer! {{
		_ = services.server
			.metrics
			.requests_handle_finished
			.fetch_add(1, Ordering::Relaxed);
		_ = services.server
			.metrics
			.requests_handle_active
			.fetch_sub(1, Ordering::Relaxed);
	}};

	next.run(req).await
}

fn handle_result(method: &Method, uri: &Uri, result: Response) -> Result<Response, StatusCode> {
	let status = result.status();
	let code = status.as_u16();
	let reason = status.canonical_reason().unwrap_or("Unknown Reason");

	if status.is_server_error() {
		error!(%method, %uri, "{code} {reason}");
	} else if status.is_client_error() {
		debug_error!(%method, %uri, "{code} {reason}");
	} else if status.is_redirection() {
		debug!(%method, %uri, "{code} {reason}");
	} else {
		trace!(%method, %uri, "{code} {reason}");
	}

	if status == StatusCode::METHOD_NOT_ALLOWED {
		return Ok(
			err!(Request(Unrecognized("Method not allowed"), METHOD_NOT_ALLOWED)).into_response()
		);
	}

	Ok(result)
}

#[cold]
fn unhandled<Error: Debug>(e: Error) -> StatusCode {
	error!("unhandled error or panic during request: {e:?}");

	StatusCode::INTERNAL_SERVER_ERROR
}
