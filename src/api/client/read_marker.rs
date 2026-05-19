use std::collections::BTreeMap;

use axum::extract::State;
use axum_client_ip::ClientIp;
use conduwuit::{Err, Event, PduCount, Result, err};
use ruma::{
	MilliSecondsSinceUnixEpoch,
	api::client::{read_marker::set_read_marker, receipt::create_receipt},
	events::{
		RoomAccountDataEventType,
		fully_read::{FullyReadEvent, FullyReadEventContent},
		receipt::{Receipt, ReceiptEvent, ReceiptEventContent, ReceiptThread, ReceiptType},
	},
};

use crate::Ruma;

/// # `POST /_matrix/client/r0/rooms/{roomId}/read_markers`
///
/// Sets different types of read markers.
///
/// - Updates fully-read account data event to `fully_read`
/// - If `read_receipt` is set: Update private marker and public read receipt
///   EDU
pub(crate) async fn set_read_marker_route(
	State(services): State<crate::State>,
	body: Ruma<set_read_marker::v3::Request>,
) -> Result<set_read_marker::v3::Response> {
	let sender_user = body.sender_user();

	let fully_read_count =
		event_count(&services, &body.room_id, body.fully_read.as_deref()).await?;
	let read_receipt_count =
		event_count(&services, &body.room_id, body.read_receipt.as_deref()).await?;
	let private_read_count =
		event_count(&services, &body.room_id, body.private_read_receipt.as_deref()).await?;

	if let Some(event) = &body.fully_read {
		let fully_read_event = FullyReadEvent::new(FullyReadEventContent::new(event.to_owned()));

		services
			.account_data
			.update(
				Some(&body.room_id),
				sender_user,
				RoomAccountDataEventType::FullyRead,
				&serde_json::to_value(fully_read_event)?,
			)
			.await?;

		if let Some(count) = fully_read_count {
			services
				.rooms
				.user
				.set_notification_read(
					sender_user,
					&body.room_id,
					ReceiptThread::Unthreaded,
					count,
				)
				.await;
		}
	}

	// ping presence
	if services.config.allow_local_presence {
		services
			.presence
			.ping_presence(sender_user, &ruma::presence::PresenceState::Online)
			.await?;
	}

	if let Some(event) = &body.read_receipt {
		if services.config.allow_local_read_receipts
			&& !services.users.is_suspended(sender_user).await?
		{
			let mut receipt = Receipt::new(MilliSecondsSinceUnixEpoch::now());
			receipt.thread = ReceiptThread::Unthreaded;
			let receipt_content = [(
				event.to_owned(),
				BTreeMap::from_iter([(
					ReceiptType::Read,
					BTreeMap::from_iter([(sender_user.to_owned(), receipt)]),
				)]),
			)];

			services
				.rooms
				.read_receipt
				.readreceipt_update(
					sender_user,
					&body.room_id,
					&ReceiptEvent::new(
						body.room_id.clone(),
						ReceiptEventContent::from_iter(receipt_content),
					),
				)
				.await;

			if let Some(count) = read_receipt_count {
				services
					.rooms
					.user
					.set_notification_read(
						sender_user,
						&body.room_id,
						ReceiptThread::Unthreaded,
						count,
					)
					.await;
			}
		}
	}

	if body.private_read_receipt.is_some() {
		let count = private_read_count.expect("private read count is present when event exists");

		services
			.rooms
			.read_receipt
			.private_read_set(&body.room_id, sender_user, count);

		services
			.rooms
			.user
			.set_notification_read(sender_user, &body.room_id, ReceiptThread::Unthreaded, count)
			.await;
	}

	Ok(set_read_marker::v3::Response::new())
}

/// # `POST /_matrix/client/r0/rooms/{roomId}/receipt/{receiptType}/{eventId}`
///
/// Sets private read marker and public read receipt EDU.
pub(crate) async fn create_receipt_route(
	State(services): State<crate::State>,
	ClientIp(client_ip): ClientIp,
	body: Ruma<create_receipt::v3::Request>,
) -> Result<create_receipt::v3::Response> {
	let sender_user = body.sender_user();
	validate_receipt_thread(&body.receipt_type, &body.thread)?;

	let count = event_count(&services, &body.room_id, Some(&body.event_id)).await?;
	let count = count.expect("receipt event count is present when event exists");

	services
		.users
		.update_device_last_seen(sender_user, body.sender_device.as_deref(), client_ip)
		.await;

	// ping presence
	if services.config.allow_local_presence {
		services
			.presence
			.ping_presence(sender_user, &ruma::presence::PresenceState::Online)
			.await?;
	}

	match body.receipt_type {
		| create_receipt::v3::ReceiptType::FullyRead => {
			let fully_read_event =
				FullyReadEvent::new(FullyReadEventContent::new(body.event_id.clone()));
			services
				.account_data
				.update(
					Some(&body.room_id),
					sender_user,
					RoomAccountDataEventType::FullyRead,
					&serde_json::to_value(fully_read_event)?,
				)
				.await?;

			services
				.rooms
				.user
				.set_notification_read(
					sender_user,
					&body.room_id,
					ReceiptThread::Unthreaded,
					count,
				)
				.await;
		},
		| create_receipt::v3::ReceiptType::Read => {
			let mut receipt = Receipt::new(MilliSecondsSinceUnixEpoch::now());
			receipt.thread = body.thread.clone();
			let receipt_content = [(
				body.event_id.clone(),
				BTreeMap::from_iter([(
					ReceiptType::Read,
					BTreeMap::from_iter([(sender_user.to_owned(), receipt)]),
				)]),
			)];

			services
				.rooms
				.read_receipt
				.readreceipt_update(
					sender_user,
					&body.room_id,
					&ReceiptEvent::new(
						body.room_id.clone(),
						ReceiptEventContent::from_iter(receipt_content),
					),
				)
				.await;

			services
				.rooms
				.user
				.set_notification_read(sender_user, &body.room_id, body.thread.clone(), count)
				.await;
		},
		| create_receipt::v3::ReceiptType::ReadPrivate => {
			services
				.rooms
				.read_receipt
				.private_read_set(&body.room_id, sender_user, count);

			services
				.rooms
				.user
				.set_notification_read(sender_user, &body.room_id, body.thread.clone(), count)
				.await;
		},
		| _ => {
			return Err!(Request(InvalidParam(warn!(
				"Received unknown read receipt type: {}",
				&body.receipt_type
			))));
		},
	}

	Ok(create_receipt::v3::Response::new())
}

async fn event_count(
	services: &crate::State,
	room_id: &ruma::RoomId,
	event_id: Option<&ruma::EventId>,
) -> Result<Option<u64>> {
	let Some(event_id) = event_id else {
		return Ok(None);
	};

	let count = services
		.rooms
		.timeline
		.get_pdu_count(event_id)
		.await
		.map_err(|_| err!(Request(NotFound("Event not found."))))?;

	let PduCount::Normal(count) = count else {
		return Err!(Request(InvalidParam(
			"Event is a backfilled PDU and cannot be marked as read."
		)));
	};

	let pdu = services
		.rooms
		.timeline
		.get_pdu(event_id)
		.await
		.map_err(|_| err!(Request(NotFound("Event not found."))))?;

	if pdu
		.room_id()
		.is_none_or(|event_room_id| event_room_id != room_id)
	{
		return Err!(Request(InvalidParam("Event does not belong to the target room.")));
	}

	Ok(Some(count))
}

fn validate_receipt_thread(
	receipt_type: &create_receipt::v3::ReceiptType,
	thread: &ReceiptThread,
) -> Result {
	if matches!(receipt_type, create_receipt::v3::ReceiptType::FullyRead)
		&& !matches!(thread, ReceiptThread::Unthreaded)
	{
		return Err!(Request(InvalidParam(
			"`m.fully_read` receipts must use the unthreaded marker."
		)));
	}

	Ok(())
}
