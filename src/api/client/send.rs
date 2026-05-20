use std::collections::BTreeMap;

use axum::extract::State;
use conduwuit::{
	Err, Result, err,
	pdu::{PartialPdu, sticky},
	utils,
};
use ruma::api::client::message::send_message_event;
use serde_json::value::RawValue;

use crate::{Ruma, client_ip::ClientIp};

/// # `PUT /_matrix/client/v3/rooms/{roomId}/send/{eventType}/{txnId}`
///
/// Send a message event into the room.
///
/// - Is a NOOP if the txn id was already used before and returns the same event
///   id again
/// - The only requirement for the content is that it has to be valid json
/// - Tries to send the event into the room, auth rules will determine if it is
///   allowed
pub(crate) async fn send_message_event_route(
	State(services): State<crate::State>,
	ClientIp(client_ip): ClientIp, // NOTE: required for updating device metadata
	body: Ruma<send_message_event::v3::Request>,
) -> Result<send_message_event::v3::Response> {
	let sender_user = body.identity.expect_sender_user()?;
	let sender_device = body.identity.sender_device();

	if services.users.is_suspended(sender_user).await? {
		return Err!(Request(UserSuspended("You cannot perform this action while suspended.")));
	}

	services
		.users
		.update_device_last_seen(sender_user, sender_device, client_ip)
		.await;

	let state_lock = services.rooms.state.mutex.lock(body.room_id.as_str()).await;

	// Check if this is a new transaction id
	if let Ok(response) = services
		.transactions
		.get_client_txn(sender_user, sender_device, &body.txn_id)
		.await
	{
		// The client might have sent a txnid of the /sendToDevice endpoint
		// This txnid has no response associated with it
		if response.is_empty() {
			return Err!(Request(InvalidParam(
				"Tried to use txn id already used for an incompatible endpoint."
			)));
		}

		let event_id = utils::string_from_bytes(&response)
			.map(TryInto::try_into)
			.map_err(|e| err!(Database("Invalid event_id in txnid data: {e:?}")))??;

		return Ok(send_message_event::v3::Response::new(event_id));
	}

	let content: Box<RawValue> = serde_json::from_str(body.body.body.json().get())
		.map_err(|e| err!(Request(BadJson("Invalid JSON body: {e}"))))?;

	let timestamp = if body.identity.is_appservice() {
		body.timestamp
	} else {
		None
	};

	let mut unsigned = BTreeMap::new();
	unsigned.insert("transaction_id".to_owned(), body.txn_id.to_string().into());

	let partial_pdu = PartialPdu {
		event_type: body.event_type.to_string().into(),
		content,
		timestamp,
		unsigned: Some(unsigned),
		sticky: body.sticky_duration_ms.map(sticky::object),
		..Default::default()
	};

	let event_id = services
		.rooms
		.timeline
		.send_timeline_event(sender_user, &body.room_id, &state_lock, partial_pdu)
		.await?;

	services.transactions.add_client_txnid(
		sender_user,
		sender_device,
		&body.txn_id,
		event_id.as_bytes(),
	);

	drop(state_lock);

	Ok(send_message_event::v3::Response::new(event_id))
}
