use axum::extract::State;
use conduwuit::{Err, Result, err};
use jwt_service_core::{join_token, participant_identity, room_alias};
use ruma::OwnedRoomId;
use ruminuwuity::matrix_rtc::livekit::get_token;

use crate::Ruma;

pub(crate) async fn get_livekit_token_route(
	State(services): State<crate::State>,
	body: Ruma<get_token::Request>,
) -> Result<get_token::Response> {
	let origin = &body.identity;
	let request = &body.body;
	let room_id = OwnedRoomId::try_from(request.room_id.as_str())
		.map_err(|e| err!(Request(InvalidParam("Invalid room ID: {e}"))))?;

	if request.user_id.server_name() != origin {
		return Err!(Request(Unauthorized(
			"The requested user does not belong to the signing server."
		)));
	}

	if !services
		.rooms
		.state_cache
		.server_in_room(services.globals.server_name(), &room_id)
		.await || !services
		.rooms
		.state_cache
		.server_in_room(origin, &room_id)
		.await
	{
		return Err!(Request(Unauthorized("Both homeservers must be joined to this room.")));
	}

	let config = services
		.config
		.matrix_rtc
		.livekit
		.as_ref()
		.ok_or_else(|| err!(Request(NotFound("LiveKit is not configured."))))?;
	if !config.allows_homeserver(origin) {
		return Err!(Request(Forbidden(
			"This server does not issue LiveKit tokens to the signing server."
		)));
	}

	let secret = services
		.globals
		.livekit_secret
		.as_deref()
		.expect("a LiveKit secret is resolved whenever LiveKit is configured");
	let identity = participant_identity(
		request.user_id.as_str(),
		&request.member.claimed_device_id,
		&request.member.id,
	);
	let jwt = join_token(
		&config.key,
		secret,
		&room_alias(room_id.as_str(), &request.slot_id),
		&identity,
		true,
		config.auto_create,
	)
	.map_err(|e| err!("Failed to create a LiveKit token: {e}"))?;

	Ok(get_token::Response::new(config.url.clone(), jwt))
}
