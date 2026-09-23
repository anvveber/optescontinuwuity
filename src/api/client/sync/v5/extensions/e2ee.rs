use std::collections::HashSet;

use conduwuit::{
	Result,
	utils::{IterStream, ReadyExt, stream::WidebandExt},
};
use conduwuit_service::Services;
use futures::{StreamExt, future::join};
use ruma::{
	OwnedEventId, OwnedRoomId, OwnedUserId, RoomId,
	api::client::sync::sync_events::v5,
	assign,
	events::{StateEventType, room::member::MembershipState},
};

use super::SyncContext;
use crate::client::sync::{
	DeviceListUpdates, room_device_list_updates, share_encrypted_room, shortstatehash_after,
	v5::room::added_state,
};

/// MSC4543: device lists are only sent once the client has a position.
pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
) -> Result<v5::response::E2EE> {
	let SyncContext {
		syncing_user,
		syncing_device,
		request,
		connection,
		..
	} = context;

	if request.extensions.e2ee.enabled != Some(true) {
		return Ok(v5::response::E2EE::default());
	}

	let users = &services.users;
	let (device_one_time_keys_count, unused_fallback_key_types) = join(
		users.count_one_time_keys(syncing_user, syncing_device),
		users.list_unused_fallback_key_types(syncing_user, syncing_device),
	)
	.await;

	let device_lists = match connection.count {
		| Some(since) => device_list_updates(services, context, since).await,
		| None => DeviceListUpdates::new(),
	};

	Ok(assign!(v5::response::E2EE::default(), {
		device_lists: device_lists.into(),
		device_one_time_keys_count,
		device_unused_fallback_key_types: Some(unused_fallback_key_types),
	}))
}

async fn device_list_updates(
	services: &Services,
	context: SyncContext<'_>,
	since: u64,
) -> DeviceListUpdates {
	let SyncContext { syncing_user, next_batch, .. } = context;

	let mut updates = DeviceListUpdates::new();
	updates.changed.extend(
		services
			.users
			.keys_changed(syncing_user, Some(since), Some(next_batch))
			.collect::<Vec<_>>()
			.await,
	);

	services
		.rooms
		.state_cache
		.rooms_joined(syncing_user)
		.wide_filter_map(async |room_id| {
			room_device_list_changes(services, context, &room_id, since).await
		})
		.ready_for_each(|room_updates| updates.merge(room_updates))
		.await;

	updates
		.left
		.extend(left_room_members(services, context, since).await);

	updates
}

/// Former co-members the user no longer shares an encrypted room with.
async fn left_room_members(
	services: &Services,
	context: SyncContext<'_>,
	since: u64,
) -> Vec<OwnedUserId> {
	let SyncContext { syncing_user, next_batch, .. } = context;
	let state_cache = &services.rooms.state_cache;

	let left_rooms: Vec<OwnedRoomId> = state_cache
		.rooms_left_ids(syncing_user)
		.wide_filter_map(async |room_id| {
			let left_count = state_cache
				.get_left_count(&room_id, syncing_user)
				.await
				.ok()?;
			let left_since = left_count > since && left_count <= next_batch;

			(left_since
				&& services
					.rooms
					.state_accessor
					.is_encrypted_room(&room_id)
					.await)
				.then_some(room_id)
		})
		.collect()
		.await;

	let mut members = HashSet::new();
	for room_id in &left_rooms {
		let room_members: Vec<_> = state_cache
			.room_members(room_id)
			.ready_filter(|user_id| user_id != syncing_user)
			.collect()
			.await;

		members.extend(room_members);
	}

	members
		.into_iter()
		.stream()
		.wide_filter_map(async |user_id| {
			(!share_encrypted_room(services, syncing_user, &user_id, None).await)
				.then_some(user_id)
		})
		.collect()
		.await
}

async fn room_device_list_changes(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	since: u64,
) -> Option<DeviceListUpdates> {
	let SyncContext { syncing_user, next_batch, .. } = context;
	let state_accessor = &services.rooms.state_accessor;

	if !state_accessor.is_encrypted_room(room_id).await {
		return None;
	}

	// membership only changes along with the timeline
	let active = services
		.rooms
		.timeline
		.room_activity(room_id)
		.await
		.map_or(true, |activity| activity > since);

	let mut membership_changes = Vec::new();
	let mut updates = DeviceListUpdates::new();

	if active
		&& let Ok(current) = services.rooms.state.get_room_shortstatehash(room_id).await
		&& let Ok(Some(previous)) = shortstatehash_after(services, room_id, since).await
		&& previous != current
	{
		let newly_shared = state_accessor.user_membership(previous, syncing_user).await
			!= MembershipState::Join
			|| state_accessor
				.state_get_shortid(previous, &StateEventType::RoomEncryption, "")
				.await
				.is_err();

		if newly_shared {
			// the user just joined, or the room just became encrypted
			updates.changed.extend(
				services
					.rooms
					.state_cache
					.room_members(room_id)
					.ready_filter(|user_id| user_id != syncing_user)
					.collect::<Vec<_>>()
					.await,
			);
		} else if let Ok(added) = added_state(services, previous, current).await {
			for ((event_type, _), shorteventid) in added {
				if event_type != StateEventType::RoomMember {
					continue;
				}

				if let Ok(event_id) = services
					.rooms
					.short
					.get_eventid_from_short::<OwnedEventId>(shorteventid)
					.await
					&& let Ok(pdu) = services.rooms.timeline.get_pdu(&event_id).await
				{
					membership_changes.push(pdu);
				}
			}
		}
	}

	updates.merge(
		room_device_list_updates(
			services,
			syncing_user,
			room_id,
			Some(since),
			next_batch,
			&membership_changes,
		)
		.await,
	);

	Some(updates)
}
