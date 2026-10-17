mod v3;
mod v5;

use std::collections::{HashSet, VecDeque};

use conduwuit::{
	Event, PduCount, Result, debug_warn, err,
	matrix::pdu::{EventHash, PduEvent},
	ref_at,
	result::LogErr,
	trace,
	utils::{
		self,
		stream::{BroadbandExt, ReadyExt, TryIgnore, WidebandExt},
	},
};
use conduwuit_service::{Services, rooms::short::ShortStateHash};
use futures::{StreamExt, pin_mut};
use ruma::{
	EventId, OwnedUserId, RoomId, UserId,
	events::{StateEventType, TimelineEventType},
	uint,
};
use serde_json::value::RawValue;

pub(crate) use self::{v3::sync_events_route, v5::sync_events_v5_route};

#[derive(Default)]
pub(crate) struct TimelinePdus {
	pub pdus: VecDeque<(PduCount, PduEvent)>,
	pub limited: bool,
}

impl TimelinePdus {
	fn senders(&self) -> impl Iterator<Item = OwnedUserId> {
		self.pdus
			.iter()
			.map(ref_at!(1))
			.map(Event::sender)
			.map(Into::into)
	}
}

/// Load up to `limit` PDUs in the range (starting_count, ending_count].
async fn load_timeline(
	services: &Services,
	sender_user: &UserId,
	room_id: &RoomId,
	starting_count: Option<PduCount>,
	ending_count: Option<PduCount>,
	limit: usize,
) -> Result<TimelinePdus> {
	if let (Some(starting_count), Some(ending_count)) = (starting_count, ending_count) {
		debug_assert!(
			starting_count <= ending_count,
			"starting count {starting_count} > ending count {ending_count}"
		);
	}

	let mut pdu_stream = match starting_count {
		| Some(starting_count) => {
			let last_timeline_count = services
				.rooms
				.timeline
				.last_timeline_count(room_id)
				.await
				.map_err(|err| {
					err!(Database(warn!("Failed to fetch end of room timeline: {}", err)))
				})?;

			if last_timeline_count <= starting_count {
				// no messages have been sent in this room since
				// `starting_count`
				return Ok(TimelinePdus::default());
			}

			// for incremental sync, stream from the DB all PDUs which were sent
			// after `starting_count` but before `ending_count`, including
			// `ending_count` but not `starting_count`. this code is pretty
			// similar to the initial sync branch, they're separate to allow
			// for future optimization
			services
				.rooms
				.timeline
				.pdus_rev(room_id, ending_count.map(|count| count.saturating_add(1)))
				.ignore_err()
				.ready_take_while(move |&(pducount, _)| pducount > starting_count)
				.wide_filter_map(async |mut pdu| {
					let ctx = services
						.rooms
						.timeline
						.get_unsigned_context(&pdu.1, Some(sender_user))
						.await;
					pdu.1.set_unsigned(Some(ctx)).log_err().ok();
					Some(pdu)
				})
				.then(async move |mut pdu| {
					if let Err(e) = services
						.rooms
						.pdu_metadata
						.add_bundled_aggregations_to_pdu(sender_user, &mut pdu.1)
						.await
					{
						debug_warn!("Failed to add bundled aggregations: {e}");
					}
					pdu
				})
				.boxed()
		},
		| None => {
			// For initial sync, stream from the DB all PDUs before and
			// including `ending_count` in reverse order
			services
				.rooms
				.timeline
				.pdus_rev(room_id, ending_count.map(|count| count.saturating_add(1)))
				.ignore_err()
				.wide_filter_map(async |mut pdu| {
					let ctx = services
						.rooms
						.timeline
						.get_unsigned_context(&pdu.1, Some(sender_user))
						.await;
					pdu.1.set_unsigned(Some(ctx)).log_err().ok();
					Some(pdu)
				})
				.then(async move |mut pdu| {
					if let Err(e) = services
						.rooms
						.pdu_metadata
						.add_bundled_aggregations_to_pdu(sender_user, &mut pdu.1)
						.await
					{
						debug_warn!("Failed to add bundled aggregations: {e}");
					}
					pdu
				})
				.boxed()
		},
	};

	// Return at most `limit` PDUs from the stream
	let pdus = pdu_stream
		.by_ref()
		.take(limit)
		.ready_fold(VecDeque::with_capacity(limit), |mut pdus, item| {
			pdus.push_front(item);
			pdus
		})
		.await;

	// The timeline is limited if there are still more PDUs in the stream
	let limited = pdu_stream.next().await.is_some();

	trace!(
		"syncing {:?} timeline pdus from {:?} to {:?} (limited = {:?})",
		pdus.len(),
		starting_count,
		ending_count,
		limited,
	);

	Ok(TimelinePdus { pdus, limited })
}

async fn share_encrypted_room(
	services: &Services,
	sender_user: &UserId,
	user_id: &UserId,
	ignore_room: Option<&RoomId>,
) -> bool {
	services
		.rooms
		.state_cache
		.get_shared_rooms(sender_user, user_id)
		.ready_filter(|room_id| Some(room_id.as_ref()) != ignore_room)
		.broad_any(|other_room_id| async move {
			services
				.rooms
				.state_accessor
				.is_encrypted_room(&other_room_id)
				.await
		})
		.await
}

/// Returns the room's state as of `count`, taken from the state before the
/// first event after it. Returns `None` if no event has been sent since
/// `count`, or if the first one is the room's create event.
async fn shortstatehash_after_count(
	services: &Services,
	room_id: &RoomId,
	count: u64,
) -> Result<Option<ShortStateHash>> {
	let pdus = services
		.rooms
		.timeline
		.pdus(room_id, Some(PduCount::Normal(count)))
		.ignore_err();
	pin_mut!(pdus);

	let Some((_, pdu_after_count)) = pdus.next().await else {
		return Ok(None);
	};

	trace!(?pdu_after_count.event_id, "pdu after count");

	if pdu_after_count.kind == TimelineEventType::RoomCreate {
		return Ok(None);
	}

	services
		.rooms
		.state_accessor
		.pdu_shortstatehash(&pdu_after_count.event_id)
		.await
		.map(Some)
		.map_err(|err| err!("PDU after count has no shortstatehash: {err}"))
}

/// Fetch the user IDs to include in the `m.heroes` property of the room
/// summary.
async fn build_heroes(
	services: &Services,
	room_id: &RoomId,
	syncing_user: &UserId,
	current_shortstatehash: ShortStateHash,
) -> HashSet<OwnedUserId> {
	const MAX_HERO_COUNT: usize = 5;

	// fetch joined members from the state cache first
	let joined_members_stream = services.rooms.state_cache.room_members(room_id);

	// then fetch invited members
	let invited_members_stream = services.rooms.state_cache.room_members_invited(room_id);

	// then as a last resort fetch every membership event
	let all_members_stream = services
		.rooms
		.short
		.multi_get_statekey_from_short(
			services
				.rooms
				.state_accessor
				.state_full_shortids(current_shortstatehash)
				.ignore_err()
				.ready_filter_map(|(key, _)| Some(key)),
		)
		.ignore_err()
		.ready_filter_map(|(event_type, state_key)| {
			if event_type == StateEventType::RoomMember {
				state_key.to_string().try_into().ok()
			} else {
				None
			}
		});

	joined_members_stream
		.chain(invited_members_stream)
		.chain(all_members_stream)
		// the hero list should never include the syncing user
		.ready_filter(|user_id| user_id != syncing_user)
		.take(MAX_HERO_COUNT)
		.collect()
		.await
}

fn create_dummy_leave_event(
	services: &Services,
	syncing_user: &UserId,
	room_id: &RoomId,
) -> PduEvent {
	// TODO: because this event ID is random, it could cause caching issues with
	// clients. perhaps a database table could be created to hold these dummy
	// events, or they could be stored as outliers?
	PduEvent {
		event_id: EventId::new_v1(services.globals.server_name()),
		sender: syncing_user.to_owned(),
		origin: None,
		origin_server_ts: utils::millis_since_unix_epoch()
			.try_into()
			.expect("Timestamp is valid js_int value"),
		kind: TimelineEventType::RoomMember,
		content: RawValue::from_string(r#"{"membership": "leave"}"#.to_owned()).unwrap(),
		sticky: None,
		state_key: Some(syncing_user.as_str().into()),
		unsigned: None,
		// The following keys are dropped on conversion
		room_id: Some(room_id.to_owned()),
		prev_events: vec![],
		depth: uint!(1),
		auth_events: vec![],
		redacts: None,
		hashes: EventHash { sha256: String::new() },
		signatures: None,
	}
}
