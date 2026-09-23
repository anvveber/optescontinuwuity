use std::collections::HashSet;

use conduwuit::utils::{IterStream, ReadyExt, stream::WidebandExt};
use conduwuit_service::{Services, rooms::read_receipt::pack_receipts, sync::ExtensionScope};
use futures::{StreamExt, future::OptionFuture};
use ruma::{
	OwnedEventId, RoomId, UserId,
	api::client::sync::sync_events::v5,
	assign,
	events::{AnySyncEphemeralRoomEvent, SyncEphemeralRoomEvent, receipt::ReceiptEventContent},
	serde::Raw,
};

use super::{SyncContext, scope, scope_update};
use crate::client::sync::v5::{
	lists::RoomSelection,
	room::{BuiltRooms, SentTimeline},
};

/// A room entering the scope only gets the receipts for its returned timeline
/// and the user's own, rather than one for every member.
pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
	rooms: &BuiltRooms,
) -> (v5::response::Receipts, Option<ExtensionScope>) {
	let SyncContext { syncing_user, request, connection, .. } = context;
	let config = &request.extensions.receipts;
	let previous = &connection.receipts;
	let enabled = config.enabled == Some(true);

	if !enabled {
		return (
			v5::response::Receipts::default(),
			scope_update(context, previous, false, HashSet::new()),
		);
	}

	// a room joined later enters the scope then, getting its full state
	let scope: HashSet<_> = scope(selection, config.lists.as_deref(), config.rooms.as_deref())
		.into_iter()
		.filter(|room_id| selection.is_joined(room_id))
		.collect();

	let receipts = scope
		.iter()
		.stream()
		.wide_filter_map(async |room_id| {
			let since = previous.since(room_id, context.since());
			let timeline = rooms.timelines.get(room_id);
			let receipts = room_receipts(services, syncing_user, room_id, since, timeline).await;

			(!receipts.is_empty())
				.then(|| (room_id.clone(), pack_receipts(Box::new(receipts.into_iter()))))
		})
		.collect()
		.await;

	let receipts = assign!(v5::response::Receipts::default(), { rooms: receipts });

	(receipts, scope_update(context, previous, true, scope))
}

async fn room_receipts(
	services: &Services,
	syncing_user: &UserId,
	room_id: &RoomId,
	since: u64,
	timeline: Option<&SentTimeline>,
) -> Vec<Raw<AnySyncEphemeralRoomEvent>> {
	let read_receipt = &services.rooms.read_receipt;
	let entering = since == 0;

	// a receipt for a timeline event can only be newer than the timeline
	let scan_from = if entering {
		timeline.and_then(|timeline| timeline.since)
	} else {
		Some(since)
	};

	let mut receipts: Vec<_> = OptionFuture::from(scan_from.map(|scan_from| {
		read_receipt
			.readreceipts_since(room_id, Some(scan_from))
			.ready_filter(|(read_user, _, receipt)| {
				!entering
					|| (read_user != syncing_user
						&& timeline
							.is_some_and(|timeline| receipts_any(receipt, &timeline.event_ids)))
			})
			.filter_map(async |(read_user, _, receipt)| {
				(!services
					.users
					.user_is_ignored(&read_user, syncing_user)
					.await)
					.then_some(receipt)
			})
			.collect::<Vec<_>>()
	}))
	.await
	.unwrap_or_default();

	if entering {
		receipts.extend(read_receipt.user_readreceipt(room_id, syncing_user).await);
	}

	if read_receipt
		.last_privateread_update(syncing_user, room_id)
		.await
		> since
	{
		receipts.extend(
			read_receipt
				.private_read_get(room_id, syncing_user)
				.await
				.ok(),
		);
	}

	receipts
}

fn receipts_any(
	receipt: &Raw<AnySyncEphemeralRoomEvent>,
	event_ids: &HashSet<OwnedEventId>,
) -> bool {
	receipt
		.deserialize_as_unchecked::<SyncEphemeralRoomEvent<ReceiptEventContent>>()
		.is_ok_and(|receipt| {
			receipt
				.content
				.keys()
				.any(|event_id| event_ids.contains(event_id))
		})
}
