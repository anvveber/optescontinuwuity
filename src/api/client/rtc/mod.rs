//! MatrixRTC LiveKit token service, equivalent to a separately deployed
//! [lk-jwt-service].
//!
//! [lk-jwt-service]: https://github.com/element-hq/lk-jwt-service

use axum::{
	Json, Router,
	extract::{Request, State},
	middleware::{self, Next},
	response::{IntoResponse, Response},
	routing::post,
};
use conduwuit::{Err, Error, Result, debug_warn, err};
use conduwuit_service::users::AccessTokenStatus;
use jwt_service_core::{
	LEGACY_SLOT_ID, join_token, legacy_participant_identity, legacy_room_alias,
	participant_identity, room_alias,
	wire::{GetTokenRequest, LegacySfuRequest, Msc4195GetTokenRequest, OpenIdToken, SfuResponse},
};
use ruma::{
	OwnedDeviceId, OwnedRoomId, OwnedServerName, OwnedUserId, UserId,
	api::{
		error::{ErrorKind, UnknownTokenErrorData},
		federation::openid::get_openid_userinfo,
	},
	assign,
};
use ruminuwuity::matrix_rtc::livekit::get_token as federation_get_token;

/// Advertised to clients as `livekit_service_url`, to which they append
/// `/get_token` or `/sfu/get`.
pub(crate) const BASE_PATH: &str = "/_matrix/client/unstable/org.matrix.msc4195/livekit";

pub(crate) fn router(state: crate::State) -> Router<crate::State> {
	Router::new()
		.route(const_str::concat!(BASE_PATH, "/get_token"), post(get_token_route))
		.route(const_str::concat!(BASE_PATH, "/sfu/get"), post(legacy_sfu_get_route))
		.route("/_matrix/client/v1/rtc/livekit/get_token", post(msc4195_get_token_route))
		.route(
			"/_matrix/client/unstable/org.matrix.msc4195/rtc/livekit/get_token",
			post(msc4195_get_token_route),
		)
		.layer(middleware::from_fn_with_state(
			state,
			async |State(state): State<crate::State>, request: Request, next: Next| -> Response {
				if state.config.matrix_rtc.livekit.is_some() {
					next.run(request).await
				} else {
					let unconfigured: Result<()> = Err!(Request(NotFound(
						"This server is not configured with a LiveKit token service."
					)));

					unconfigured.into_response()
				}
			},
		))
}

/// # `POST /_matrix/client/unstable/org.matrix.msc4195/livekit/get_token`
///
/// Issues a LiveKit access token, authenticated by an OpenID token.
pub(crate) async fn get_token_route(
	State(services): State<crate::State>,
	body: String,
) -> Result<Json<SfuResponse>> {
	let body: GetTokenRequest = parse_body(&body)?;

	let user_id = resolve_openid_token(&services, &body.openid_token).await?;

	if !body.member.claimed_user_id.is_empty() && body.member.claimed_user_id != user_id.as_str()
	{
		return Err!(Request(Unauthorized(
			"The claimed user ID does not own this OpenID token."
		)));
	}

	let room_id = check_membership(&services, &user_id, &body.room_id).await?;
	let identity = legacy_participant_identity(
		user_id.as_str(),
		&body.member.claimed_device_id,
		&body.member.id,
	);

	create_token(services, &legacy_room_alias(room_id.as_str(), &body.slot_id), &identity, true)
}

/// # `POST /_matrix/client/unstable/org.matrix.msc4195/livekit/sfu/get`
///
/// Deprecated pre-Matrix-2.0 token endpoint. It has no slot or member concept,
/// so the slot is the whole-room call and the identity is the plaintext
/// `{user_id}:{device_id}`, as in lk-jwt-service.
pub(crate) async fn legacy_sfu_get_route(
	State(services): State<crate::State>,
	body: String,
) -> Result<Json<SfuResponse>> {
	let body: LegacySfuRequest = parse_body(&body)?;

	let user_id = resolve_openid_token(&services, &body.openid_token).await?;
	let room_id = check_membership(&services, &user_id, &body.room).await?;

	create_token(
		services,
		&legacy_room_alias(room_id.as_str(), LEGACY_SLOT_ID),
		&format!("{user_id}:{}", body.device_id),
		true,
	)
}

/// # `POST /_matrix/client/v1/rtc/livekit/get_token`
///
/// MSC4195 token endpoint, authenticated with an ordinary access token.
pub(crate) async fn msc4195_get_token_route(
	State(services): State<crate::State>,
	headers: http::HeaderMap,
	body: String,
) -> Result<Json<SfuResponse>> {
	let body: Msc4195GetTokenRequest = parse_body(&body)?;
	let (user_id, device_id) = resolve_access_token(&services, &headers).await?;

	if body.member.claimed_device_id != device_id.as_str() {
		return Err!(Request(Forbidden(
			"The claimed device ID does not match the authenticated device."
		)));
	}

	let room_id = check_membership(&services, &user_id, &body.room_id).await?;
	if let Some(server_name) = body.server_name {
		let server_name = OwnedServerName::try_from(server_name.as_str())
			.map_err(|e| err!(Request(InvalidParam("Invalid server_name: {e}"))))?;
		if server_name != services.server.name {
			let response = services
				.federation
				.execute(
					&server_name,
					federation_get_token::Request::new(
						user_id,
						room_id.to_string(),
						body.slot_id,
						federation_get_token::Member {
							id: body.member.id,
							claimed_device_id: body.member.claimed_device_id,
						},
					),
				)
				.await?;

			return Ok(Json(SfuResponse { url: response.url, jwt: response.jwt }));
		}
	}

	let identity =
		participant_identity(user_id.as_str(), &body.member.claimed_device_id, &body.member.id);

	create_token(services, &room_alias(room_id.as_str(), &body.slot_id), &identity, true)
}

/// Axum's `Json` rejection is plain text and names the struct's fields.
fn parse_body<T: serde::de::DeserializeOwned>(body: &str) -> Result<T> {
	serde_json::from_str(body).map_err(|e| {
		debug_warn!("Failed to parse request body: {e}");
		err!(Request(NotJson("Error reading request")))
	})
}

fn create_token(
	services: crate::State,
	room_alias: &str,
	identity: &str,
	can_publish: bool,
) -> Result<Json<SfuResponse>> {
	let config = services
		.config
		.matrix_rtc
		.livekit
		.as_ref()
		.expect("the router layer rejects requests when LiveKit is unconfigured");

	let secret = services
		.globals
		.livekit_secret
		.as_deref()
		.expect("a LiveKit secret is resolved whenever LiveKit is configured");

	let jwt = join_token(
		&config.key,
		secret,
		room_alias,
		identity,
		can_publish,
		can_publish && config.auto_create,
	)
	.map_err(|e| err!("Failed to create a LiveKit token: {e}"))?;

	Ok(Json(SfuResponse { url: config.url.clone(), jwt }))
}

async fn resolve_openid_token(
	services: &crate::State,
	token: &OpenIdToken,
) -> Result<OwnedUserId> {
	let config = services
		.config
		.matrix_rtc
		.livekit
		.as_ref()
		.expect("the router layer rejects requests when LiveKit is unconfigured");

	let server_name = OwnedServerName::try_from(token.matrix_server_name.as_str())
		.map_err(|e| err!(Request(InvalidParam("Invalid matrix_server_name: {e}"))))?;

	let is_local = server_name == services.server.name;

	if !is_local && !config.allows_homeserver(&server_name) {
		return Err!(Request(Forbidden(
			"This server does not issue LiveKit tokens to users of {server_name}."
		)));
	}

	let user_id = if is_local {
		services
			.users
			.find_from_openid_token(&token.access_token)
			.await?
	} else {
		services
			.federation
			.execute_unauthenticated(
				&server_name,
				get_openid_userinfo::v1::Request::new(token.access_token.clone()),
			)
			.await
			.map_err(|e| {
				err!(Request(Unauthorized(debug_warn!(
					"Failed to verify OpenID token with {server_name}: {e}"
				))))
			})?
			.sub
	};

	// The allowlist check above was made against matrix_server_name, so that
	// server may only authenticate its own users.
	if user_id.server_name() != server_name {
		return Err!(Request(Forbidden(
			"{server_name} returned a user ID belonging to another server."
		)));
	}

	Ok(user_id)
}

async fn resolve_access_token(
	services: &crate::State,
	headers: &http::HeaderMap,
) -> Result<(OwnedUserId, OwnedDeviceId)> {
	let token = headers
		.get(http::header::AUTHORIZATION)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.strip_prefix("Bearer "))
		.filter(|token| !token.is_empty());

	let Some(token) = token else {
		return Err!(Request(MissingToken("Missing access token.")));
	};

	match services.users.find_from_token(token).await {
		| Some((user_id, device_id, AccessTokenStatus::Valid)) => Ok((user_id, device_id)),
		| Some((.., AccessTokenStatus::Expired)) => Err(Error::Request(
			ErrorKind::UnknownToken(assign!(UnknownTokenErrorData::new(), {
				soft_logout: true
			})),
			"This token has expired".into(),
			http::StatusCode::UNAUTHORIZED,
		)),
		| None => Err(Error::Request(
			ErrorKind::UnknownToken(UnknownTokenErrorData::new()),
			"Unrecognised access token".into(),
			http::StatusCode::UNAUTHORIZED,
		)),
	}
}

async fn check_membership(
	services: &crate::State,
	user_id: &UserId,
	room_id: &str,
) -> Result<OwnedRoomId> {
	let room_id = OwnedRoomId::try_from(room_id)
		.map_err(|e| err!(Request(InvalidParam("Invalid room ID: {e}"))))?;

	if !services
		.rooms
		.state_cache
		.is_joined(user_id, &room_id)
		.await
	{
		// MSC4195 specifies 401 here rather than the 403 this would warrant.
		return Err!(Request(Unauthorized("You are not joined to this room.")));
	}

	Ok(room_id)
}
