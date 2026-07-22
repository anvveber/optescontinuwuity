use axum::extract::State;
use conduwuit::{Err, Result, err};
use futures::StreamExt;
use rand::seq::SliceRandom;
use ruma::{
	Int, OwnedServerName,
	api::federation::query::{
		get_presence_recipients, get_profile_information, get_room_information,
	},
};

use crate::Ruma;

/// # `GET /_matrix/federation/v1/query/directory`
///
/// Resolve a room alias to a room id.
pub(crate) async fn get_room_information_route(
	State(services): State<crate::State>,
	body: Ruma<get_room_information::v1::Request>,
) -> Result<get_room_information::v1::Response> {
	let room_id = services
		.rooms
		.alias
		.resolve_local_alias(&body.room_alias)
		.await
		.map_err(|_| err!(Request(NotFound("Room alias not found."))))?;

	let mut servers: Vec<OwnedServerName> = services
		.rooms
		.state_cache
		.room_servers(&room_id)
		.collect()
		.await;

	servers.sort_unstable();
	servers.dedup();

	servers.shuffle(&mut rand::rng());

	// insert our server as the very first choice if in list
	if let Some(server_index) = servers
		.iter()
		.position(|server| server == services.globals.server_name())
	{
		servers.swap_remove(server_index);
		servers.insert(0, services.globals.server_name().to_owned());
	}

	Ok(get_room_information::v1::Response::new(room_id, servers))
}

/// # `GET /_matrix/federation/v1/query/profile`
///
///
/// Gets information on a profile.
pub(crate) async fn get_profile_information_route(
	State(services): State<crate::State>,
	body: Ruma<get_profile_information::v1::Request>,
) -> Result<get_profile_information::v1::Response> {
	if !services
		.server
		.config
		.allow_inbound_profile_lookup_federation_requests
	{
		return Err!(Request(Forbidden(
			"Profile lookup over federation is not allowed on this homeserver."
		)));
	}

	if !services.globals.server_is_ours(body.user_id.server_name()) {
		return Err!(Request(InvalidParam("User does not belong to this server.")));
	}

	let response = if let Some(field) = &body.field {
		let mut response = get_profile_information::v1::Response::new();

		if let Some(value) = services
			.users
			.get_local_profile_field(&body.user_id, field.to_owned())
			.await
		{
			response.set(value.field_name().as_str().to_owned(), value.value().into_owned());
		}

		response
	} else {
		services
			.users
			.get_local_profile(&body.user_id)
			.await
			.into_iter()
			.collect()
	};

	Ok(response)
}

/// # `GET /_matrix/federation/unstable/org.continuwuity.presence_v2.msc4495/query/presence_recipients`
///
/// Gets the list of users that may access a given local user's presence
/// information.
pub(crate) async fn get_presence_recipients_route(
	State(services): State<crate::State>,
	body: Ruma<get_presence_recipients::msc4495::Request>,
) -> Result<get_presence_recipients::msc4495::Response> {
	if !services.server.config.enable_msc4495_selective_presence {
		return Err!(Request(Unrecognized("MSC4495 is not enabled on this server.")));
	}

	if body.user_id.server_name() != services.globals.server_name() {
		return Err!(Request(InvalidParam("User does not belong to this server.")));
	}

	let last_recipients_set = services.presence.get_recipients(&body.user_id).await?;

	let recipients = last_recipients_set
		.recipient_users
		.into_iter()
		.filter(|user| user.server_name() == body.identity)
		.collect();

	// We base our generated stream IDs on `last_count`, but JS Number types
	// cannot represent 64 bit integers well. We will reasonably never
	// encounter that here, and if we do, we error.
	let stream_id = Int::try_from(last_recipients_set.stream_id).expect("js_int overflow!");

	Ok(get_presence_recipients::msc4495::Response::new(stream_id, recipients))
}
