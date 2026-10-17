use std::collections::HashSet;

use conduwuit::{
	Event, Result,
	matrix::pdu::{PduCount, PduEvent, sticky},
	result::LogErr,
	utils::{
		IterStream,
		math::usize_from_ruma,
		stream::{TryIgnore, WidebandExt},
	},
};
use conduwuit_service::Services;
use futures::{StreamExt, pin_mut};
use ruma::{
	MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedRoomId, UserId,
	api::client::sync::sync_events::v5, events::room::member::MembershipState,
};

use super::SyncContext;
use crate::client::sync::v5::{lists::RoomSelection, room::BuiltRooms};

const DEFAULT_LIMIT: usize = 100;

/// Builds the [MSC4480] sticky events extension: the sticky events sent since
/// the extension's `since` token in every joined room a list or subscription
/// matches, including rooms outside the lists' ranges.
///
/// [MSC4480]: https://github.com/matrix-org/matrix-spec-proposals/pull/4480
pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
	rooms: &BuiltRooms,
) -> Result<v5::response::StickyEvents> {
	let SyncContext { syncing_user, request, next_batch, .. } = context;
	let config = &request.extensions.sticky_events;

	if !services.config.allow_sticky_events || config.enabled != Some(true) {
		return Ok(v5::response::StickyEvents::default());
	}

	let since = config
		.since
		.as_deref()
		.and_then(|token| token.parse::<u64>().ok())
		.unwrap_or(0);
	let limit = config
		.limit
		.map_or(DEFAULT_LIMIT, usize_from_ruma)
		.clamp(1, DEFAULT_LIMIT);
	let now = u64::from(MilliSecondsSinceUnixEpoch::now().get());

	let joined_rooms = selection
		.matched
		.iter()
		.filter(|(_, membership)| **membership == MembershipState::Join)
		.map(|(room_id, _)| room_id)
		.chain(
			selection
				.subscriptions
				.iter()
				.filter(|room_id| selection.is_joined(room_id)),
		)
		.collect::<HashSet<_>>();

	let timeline_event_ids: HashSet<&OwnedEventId> = rooms
		.timelines
		.values()
		.flat_map(|timeline| &timeline.event_ids)
		.collect();

	let found: Vec<_> = joined_rooms
		.into_iter()
		.stream()
		.wide_then(async |room_id| {
			room_sticky_events(services, context, rooms, &timeline_event_ids, since, now, room_id)
				.await
		})
		.collect()
		.await;

	let (mut stream, backlog): (Vec<_>, Vec<_>) = found
		.into_iter()
		.flatten()
		.partition(|(count, ..)| *count > since);

	stream.sort_by_key(|(count, ..)| *count);

	let mut response = v5::response::StickyEvents::default();

	// MSC4480 lets us ignore the limit for a newly visible room's backlog
	for (_, room_id, pdu) in backlog {
		push_sticky_event(services, &mut response, syncing_user, room_id, pdu).await;
	}

	// with more events waiting, the batch ends at the last one returned
	let token = match stream.get(limit) {
		| Some(_) => stream[limit.saturating_sub(1)].0,
		| None => next_batch,
	};

	for (_, room_id, pdu) in stream.into_iter().take(limit) {
		push_sticky_event(services, &mut response, syncing_user, room_id, pdu).await;
	}

	// MSC4480 requires a `next_batch` even when nothing was sent, and the
	// client sending it back keeps the next request from scanning again
	response.next_batch = Some(token.to_string());

	Ok(response)
}

/// Returns the sticky events of `room_id` the client does not have yet, with
/// their stream counts, newest first.
///
/// A room the user just joined gets every event that is still sticky, as a
/// single stream position cannot express its backlog. Events already sent in
/// a timeline (`timeline_event_ids`) are left out.
async fn room_sticky_events(
	services: &Services,
	context: SyncContext<'_>,
	rooms: &BuiltRooms,
	timeline_event_ids: &HashSet<&OwnedEventId>,
	since: u64,
	now: u64,
	room_id: &OwnedRoomId,
) -> Vec<(u64, OwnedRoomId, PduEvent)> {
	let SyncContext { syncing_user, next_batch, .. } = context;
	let oldest_sticky_ts = now.saturating_sub(sticky::MAX_DURATION_MS);

	let initial = rooms
		.rooms
		.get(room_id)
		.is_some_and(|room| room.initial == Some(true))
		&& context
			.connection
			.rooms
			.get(room_id)
			.is_none_or(|previous| previous.membership != MembershipState::Join);

	let active = initial
		|| services
			.rooms
			.timeline
			.room_activity(room_id)
			.await
			.map_or(true, |activity| activity > since);

	if !active {
		return Vec::new();
	}

	let pdus = services
		.rooms
		.timeline
		.pdus_rev(room_id, Some(PduCount::Normal(next_batch.saturating_add(1))))
		.ignore_err();
	pin_mut!(pdus);

	let mut found = Vec::new();
	while let Some((count, pdu)) = pdus.next().await {
		// nothing older than this can still be sticky, and on the
		// stream we have already delivered everything up to `since`
		if u64::from(pdu.origin_server_ts) < oldest_sticky_ts
			|| (!initial && count <= PduCount::Normal(since))
		{
			break;
		}

		let PduCount::Normal(count) = count else {
			continue;
		};

		let is_sticky = pdu
			.sticky
			.as_deref()
			.is_some_and(|sticky| sticky::is_sticky(pdu.origin_server_ts, sticky, now));

		if !is_sticky
			|| timeline_event_ids.contains(&pdu.event_id)
			|| services
				.users
				.user_is_ignored(pdu.sender(), syncing_user)
				.await
		{
			continue;
		}

		found.push((count, room_id.clone(), pdu));
	}

	found
}

async fn push_sticky_event(
	services: &Services,
	response: &mut v5::response::StickyEvents,
	syncing_user: &UserId,
	room_id: OwnedRoomId,
	mut pdu: PduEvent,
) {
	let ctx = services
		.rooms
		.timeline
		.get_unsigned_context(&pdu, Some(syncing_user))
		.await;
	pdu.set_unsigned(Some(ctx)).log_err().ok();
	response
		.rooms
		.entry(room_id)
		.or_default()
		.events
		.push(Event::into_format(pdu));
}
