use std::{
	collections::{BTreeMap, HashMap, HashSet},
	sync::Arc,
};

use conduwuit::{
	Event, PduCount, PduEvent, Result, at,
	matrix::StateKey,
	utils::{
		IterStream, ReadyExt, TryFutureExtExt,
		math::{ruma_from_u64, ruma_from_usize},
		stream::{BroadbandExt, TryIgnore, WidebandExt},
	},
	warn,
};
use conduwuit_service::{
	Services,
	rooms::short::{ShortEventId, ShortStateHash},
	sync::{RequiredState, RoomConfig, RoomState},
};
use futures::{
	FutureExt, StreamExt, TryFutureExt,
	future::{OptionFuture, join},
	pin_mut,
};
use ruma::{
	JsOption, OwnedEventId, OwnedRoomId, OwnedUserId, RoomId, UInt, UserId,
	api::client::sync::sync_events::{UnreadNotificationsCount, v5},
	assign,
	events::{
		StateEventType,
		TimelineEventType::{
			self, Beacon, CallInvite, PollStart, RoomCreate, RoomEncrypted, RoomMember,
			RoomMessage, Sticker,
		},
		room::{
			avatar::RoomAvatarEventContent,
			member::{MembershipState, RoomMemberEventContent},
			name::RoomNameEventContent,
		},
	},
};

use super::{
	SyncContext,
	lists::{RoomSelection, SelectedRoom},
};
use crate::client::{
	TimelinePdus, ignored_filter,
	sync::{build_heroes, create_dummy_leave_event, load_timeline, shortstatehash_after_count},
};

const BUMP_TYPES: &[TimelineEventType] =
	&[RoomCreate, RoomMessage, RoomEncrypted, Sticker, CallInvite, PollStart, Beacon];

const BUMP_LOOKBACK: usize = 50;

#[derive(Default)]
pub(super) struct BuiltRooms {
	pub(super) rooms: BTreeMap<OwnedRoomId, v5::response::Room>,
	pub(super) updates: HashMap<OwnedRoomId, RoomState>,
	pub(super) timelines: HashMap<OwnedRoomId, SentTimeline>,
}

#[derive(Default)]
pub(super) struct SentTimeline {
	/// The count before the first event.
	pub(super) since: Option<u64>,
	pub(super) event_ids: HashSet<OwnedEventId>,
}

struct BuiltRoom {
	room: v5::response::Room,
	timeline: SentTimeline,
}

impl From<v5::response::Room> for BuiltRoom {
	fn from(room: v5::response::Room) -> Self { Self { room, timeline: SentTimeline::default() } }
}

struct RoomView {
	/// Events at or before this are hidden.
	start: Option<PduCount>,
	end: PduCount,
	shortstatehash: ShortStateHash,
	leave: Option<PduEvent>,
}

#[derive(Default)]
struct StateChanges {
	initial: bool,
	changed: Vec<((StateEventType, StateKey), ShortEventId)>,
	name: bool,
	avatar: bool,
	members: bool,
}

impl StateChanges {
	fn initial() -> Self {
		Self {
			initial: true,
			changed: Vec::new(),
			name: true,
			avatar: true,
			members: true,
		}
	}
}

/// Builds the response for each selected room that has something the client
/// has not been sent yet, along with what the connection should remember about
/// it once the client acknowledges the response.
pub(super) async fn build_rooms(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
) -> BuiltRooms {
	selection
		.rooms
		.iter()
		.stream()
		.wide_filter_map(async |(room_id, selected)| {
			let previous = context.connection.rooms.get(room_id);
			let since = previous.and_then(|previous| since(previous, selected));
			let built = build_room(services, context, room_id, selected, since)
				.await
				.inspect_err(|error| warn!(%room_id, %error, "Failed to build sliding sync room"))
				.ok()
				.flatten();

			let mut update = room_state_update(context, previous, selected, built.is_some());

			// MSC4186: a change of DM status alone is enough to send the room
			let dm_changed = previous.is_some_and(|previous| previous.is_dm != selected.is_dm);
			let built = match built {
				| Some(mut built) => {
					if built.room.initial == Some(true) || dm_changed {
						built.room.is_dm = Some(selected.is_dm);
					}

					Some(built)
				},
				| None if dm_changed => {
					// only the DM status was sent, so the room's position stays
					let previous = update.as_ref().or(previous).cloned();
					update =
						previous.map(|previous| RoomState { is_dm: selected.is_dm, ..previous });

					Some(
						assign!(v5::response::Room::new(), { is_dm: Some(selected.is_dm) })
							.into(),
					)
				},
				| None => None,
			};

			(built.is_some() || update.is_some()).then(|| (room_id.clone(), built, update))
		})
		.ready_fold(BuiltRooms::default(), |mut rooms, (room_id, built, update)| {
			if let Some(BuiltRoom { room, timeline }) = built {
				rooms.timelines.insert(room_id.clone(), timeline);
				rooms.rooms.insert(room_id.clone(), room);
			}

			if let Some(update) = update {
				rooms.updates.insert(room_id, update);
			}

			rooms
		})
		.await
}

/// Returns the position to send a previously sent room's changes from, or
/// `None` if it must be sent in full again because the user's membership
/// changed or the room's config now asks for more than was sent.
fn since(previous: &RoomState, selected: &SelectedRoom) -> Option<u64> {
	let comparable = match &selected.membership {
		// a leave is sent as a change to whatever was sent before
		| MembershipState::Leave | MembershipState::Ban => true,
		| membership => previous.membership == *membership,
	};

	(comparable && !selected.config.expands(&previous.config)).then_some(previous.sent_at)
}

/// Returns what the connection should remember about the room once the client
/// acknowledges this response, or `None` if nothing changed.
fn room_state_update(
	context: SyncContext<'_>,
	previous: Option<&RoomState>,
	selected: &SelectedRoom,
	sent: bool,
) -> Option<RoomState> {
	if sent {
		return Some(RoomState {
			sent_at: context.next_batch,
			membership: selected.membership.clone(),
			is_dm: selected.is_dm,
			config: Arc::clone(&selected.config),
		});
	}

	// remember a narrower config, so widening it again resends what the
	// client does not have
	previous
		.filter(|previous| {
			*previous.config != *selected.config && !selected.config.expands(&previous.config)
		})
		.map(|previous| RoomState {
			config: Arc::clone(&selected.config),
			..previous.clone()
		})
}

async fn build_room(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	selected: &SelectedRoom,
	since: Option<u64>,
) -> Result<Option<BuiltRoom>> {
	let config = &selected.config;
	match &selected.membership {
		| MembershipState::Join => build_joined(services, context, room_id, config, since).await,
		| MembershipState::Invite | MembershipState::Knock =>
			build_stripped(services, context, room_id, &selected.membership, since).await,
		| MembershipState::Leave | MembershipState::Ban =>
			build_left(services, context, room_id, config, since).await,
		| _ => Ok(None),
	}
}

async fn build_joined(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	config: &RoomConfig,
	since: Option<u64>,
) -> Result<Option<BuiltRoom>> {
	let SyncContext { syncing_user, next_batch, .. } = context;

	if let Some(since) = since
		&& !has_updates(services, syncing_user, room_id, since).await
	{
		return Ok(None);
	}

	let shortstatehash = services
		.rooms
		.state
		.get_room_shortstatehash(room_id)
		.await?;
	let view = RoomView {
		start: None,
		end: PduCount::Normal(next_batch),
		shortstatehash,
		leave: None,
	};

	build_timeline_room(services, context, room_id, config, since, view).await
}

async fn has_updates(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	since: u64,
) -> bool {
	let activity = services
		.rooms
		.timeline
		.room_activity(room_id)
		.map_ok(|activity| activity > since)
		.unwrap_or_else(|_| true);

	let notifications_read = services
		.rooms
		.user
		.last_notification_read(user_id, room_id)
		.map(|last_read| last_read > since);

	let (activity, notifications_read) = join(activity, notifications_read).await;
	activity || notifications_read
}

async fn build_stripped(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	membership: &MembershipState,
	since: Option<u64>,
) -> Result<Option<BuiltRoom>> {
	let SyncContext { syncing_user, next_batch, .. } = context;
	let state_cache = &services.rooms.state_cache;

	let count = match membership {
		| MembershipState::Knock => state_cache.get_knock_count(room_id, syncing_user).await?,
		| _ => state_cache.get_invite_count(room_id, syncing_user).await?,
	};

	if count > next_batch || since.is_some_and(|since| count <= since) {
		return Ok(None);
	}

	let stripped_state = match membership {
		| MembershipState::Knock => state_cache.knock_state(syncing_user, room_id).await?,
		| _ => state_cache.invite_state(syncing_user, room_id).await?,
	};

	// MSC4186 uses the time of the invite or knock as the bump stamp
	let bump_stamp = services
		.rooms
		.state_accessor
		.room_state_get(room_id, &StateEventType::RoomMember, syncing_user.as_str())
		.await
		.ok()
		.filter(|pdu| {
			pdu.get_content::<RoomMemberEventContent>()
				.is_ok_and(|content| content.membership == *membership)
		})
		.map(|pdu| pdu.origin_server_ts);

	Ok(Some(
		assign!(v5::response::Room::new(), {
			initial: Some(since.is_none()),
			invite_state: Some(stripped_state),
			bump_stamp,
		})
		.into(),
	))
}

async fn build_left(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	config: &RoomConfig,
	since: Option<u64>,
) -> Result<Option<BuiltRoom>> {
	let SyncContext { syncing_user, next_batch, .. } = context;
	let state_cache = &services.rooms.state_cache;

	let left_count = state_cache.get_left_count(room_id, syncing_user).await?;
	if left_count > next_batch || since.is_some_and(|since| left_count <= since) {
		return Ok(None);
	}

	let Some(leave_pdu) = state_cache.left_state(syncing_user, room_id).await? else {
		// removed by a server admin
		let leave_pdu = create_dummy_leave_event(services, syncing_user, room_id);
		return Ok(Some(leave_only(since, leave_pdu).into()));
	};

	// e.g. a rejected invite to a room nobody here is in
	if !services.rooms.metadata.exists(room_id).await {
		return Ok(Some(leave_only(since, leave_pdu).into()));
	}

	let end = services
		.rooms
		.timeline
		.get_pdu_count(&leave_pdu.event_id)
		.await?;

	// the state immediately before the leave
	let state_accessor = &services.rooms.state_accessor;
	let shortstatehash = state_accessor
		.pdu_shortstatehash(&leave_pdu.event_id)
		.await?;

	// not joined before leaving, e.g. a rejected invite
	let Ok(join_pdu) = state_accessor
		.state_get(shortstatehash, &StateEventType::RoomMember, syncing_user.as_str())
		.await
	else {
		return Ok(Some(leave_only(since, leave_pdu).into()));
	};

	// an unreadable membership must not reveal the history after it
	if !join_pdu
		.get_content::<RoomMemberEventContent>()
		.is_ok_and(|content| content.membership == MembershipState::Join)
	{
		return Ok(Some(leave_only(since, leave_pdu).into()));
	}

	let join_count = services
		.rooms
		.timeline
		.get_pdu_count(&join_pdu.event_id)
		.await?;

	let view = RoomView {
		start: Some(join_count.saturating_sub(1)),
		end,
		shortstatehash,
		leave: Some(leave_pdu),
	};

	build_timeline_room(services, context, room_id, config, since, view).await
}

/// Returns a room holding only the leave event, for a left room whose history
/// the user cannot see.
fn leave_only(since: Option<u64>, leave_pdu: PduEvent) -> v5::response::Room {
	assign!(v5::response::Room::new(), {
		initial: Some(since.is_none()),
		required_state: vec![leave_pdu.into_format()],
	})
}

async fn build_timeline_room(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	config: &RoomConfig,
	since: Option<u64>,
	RoomView { start, end, shortstatehash, leave }: RoomView,
) -> Result<Option<BuiltRoom>> {
	let SyncContext { syncing_user, .. } = context;
	let initial = since.is_none();

	let timeline = load_timeline(
		services,
		syncing_user,
		room_id,
		since
			.map(PduCount::Normal)
			.max(start)
			.map(|start| start.min(end)),
		Some(end),
		config.timeline_limit,
	)
	.await?;

	let changes = match since {
		| None => StateChanges::initial(),
		| Some(since) => state_changes(services, room_id, since, shortstatehash).await?,
	};

	let joined = leave.is_none();
	let counts_changed = joined
		&& notification_counts_changed(services, syncing_user, room_id, since, &timeline).await;

	let mut required_state = required_state(
		services,
		syncing_user,
		shortstatehash,
		&config.required_state,
		&timeline,
		&changes,
	)
	.await;

	if !initial
		&& leave.is_none()
		&& timeline.pdus.is_empty()
		&& !timeline.limited
		&& required_state.is_empty()
		&& !(changes.name || changes.avatar || changes.members)
		&& !counts_changed
	{
		return Ok(None);
	}

	// clients take missing counts as zero
	let unread_notifications = if joined {
		notification_counts(services, syncing_user, room_id).await
	} else {
		UnreadNotificationsCount::new()
	};

	let state_accessor = &services.rooms.state_accessor;
	let room_name = OptionFuture::from((changes.name || changes.members).then(|| {
		state_accessor
			.state_get_content(shortstatehash, &StateEventType::RoomName, "")
			.map_ok(|content: RoomNameEventContent| content.name)
			.ok()
	}))
	.await
	.flatten();

	let heroes = OptionFuture::from(
		// a left user must not learn who is in the room now
		(joined && (changes.members || changes.name) && room_name.is_none())
			.then(|| heroes(services, syncing_user, room_id, shortstatehash)),
	)
	.await;

	let avatar = if changes.avatar {
		state_accessor
			.state_get_content(shortstatehash, &StateEventType::RoomAvatar, "")
			.await
			.map_or(JsOption::Null, |content: RoomAvatarEventContent| {
				JsOption::from_option(content.url)
			})
	} else {
		JsOption::Undefined
	};

	let (joined_count, invited_count) = if joined && changes.members {
		let state_cache = &services.rooms.state_cache;
		let (joined, invited) =
			join(state_cache.room_joined_count(room_id), state_cache.room_invited_count(room_id))
				.await;

		(
			Some(ruma_from_u64(joined.unwrap_or(0))),
			Some(ruma_from_u64(invited.unwrap_or(0))),
		)
	} else {
		(None, None)
	};

	let mut bump_stamp = timeline
		.pdus
		.iter()
		.rev()
		.find(|(_, pdu)| BUMP_TYPES.contains(&pdu.kind))
		.map(|(_, pdu)| pdu.origin_server_ts);

	if joined && (initial || timeline.limited) && bump_stamp.is_none() {
		let before = timeline
			.pdus
			.front()
			.map_or_else(|| end.saturating_add(1), at!(0));
		let after = since.map(PduCount::Normal);
		bump_stamp = latest_bump_stamp(services, room_id, before, after).await;
	}

	if let Some(leave) = leave {
		required_state.retain(|pdu| {
			!(pdu.kind == RoomMember && pdu.state_key() == Some(syncing_user.as_str()))
		});

		bump_stamp = Some(leave.origin_server_ts);
		required_state.push(leave);
	}

	// clients drop their pagination token for the room when a response has no
	// `prev_batch`, so an empty timeline points back at `since`
	let prev_batch = timeline
		.pdus
		.front()
		.map(|(count, _)| count.to_string())
		.or_else(|| since.map(|since| since.to_string()));
	let limited = timeline.limited;

	let timeline: Vec<_> = timeline
		.pdus
		.into_iter()
		.stream()
		.wide_filter_map(|item| ignored_filter(services, item, syncing_user))
		.collect()
		.await;

	let num_live = num_live_events(timeline.iter().map(|(count, _)| count), context.since());
	let sent_timeline = SentTimeline {
		since: timeline.iter().find_map(|(count, _)| match count {
			| PduCount::Normal(count) => Some(count.saturating_sub(1)),
			| PduCount::Backfilled(_) => None,
		}),
		event_ids: timeline
			.iter()
			.map(|(_, pdu)| pdu.event_id.clone())
			.collect(),
	};

	let room = assign!(v5::response::Room::new(), {
		name: room_name.filter(|_| changes.name),
		avatar,
		initial: Some(initial),
		unread_notifications,
		timeline: timeline.into_iter().map(at!(1)).map(Event::into_format).collect(),
		required_state: required_state.into_iter().map(Event::into_format).collect(),
		prev_batch,
		limited,
		joined_count,
		invited_count,
		num_live: Some(ruma_from_usize(num_live)),
		bump_stamp,
		heroes,
	});

	Ok(Some(BuiltRoom { room, timeline: sent_timeline }))
}

async fn state_changes(
	services: &Services,
	room_id: &RoomId,
	since: u64,
	shortstatehash: ShortStateHash,
) -> Result<StateChanges> {
	let Some(since_shortstatehash) = shortstatehash_after_count(services, room_id, since).await?
	else {
		return Ok(StateChanges::default());
	};

	if since_shortstatehash == shortstatehash {
		return Ok(StateChanges::default());
	}

	let changed = added_state(services, since_shortstatehash, shortstatehash).await?;
	let changed_type = |event_type: StateEventType| {
		changed
			.iter()
			.any(|((changed_type, _), _)| *changed_type == event_type)
	};

	Ok(StateChanges {
		initial: false,
		name: changed_type(StateEventType::RoomName),
		avatar: changed_type(StateEventType::RoomAvatar),
		members: changed_type(StateEventType::RoomMember),
		changed,
	})
}

pub(super) async fn added_state(
	services: &Services,
	from: ShortStateHash,
	to: ShortStateHash,
) -> Result<Vec<((StateEventType, StateKey), ShortEventId)>> {
	let (shortstatekeys, shorteventids): (Vec<_>, Vec<_>) = services
		.rooms
		.state_accessor
		.state_added((from, to))
		.await?
		.into_iter()
		.unzip();

	Ok(services
		.rooms
		.short
		.multi_get_statekey_from_short(shortstatekeys.into_iter().stream())
		.zip(shorteventids.into_iter().stream())
		.ready_filter_map(|(key, shorteventid)| Some((key.ok()?, shorteventid)))
		.collect()
		.await)
}

async fn required_state(
	services: &Services,
	syncing_user: &UserId,
	shortstatehash: ShortStateHash,
	required_state: &RequiredState,
	timeline: &TimelinePdus,
	changes: &StateChanges,
) -> Vec<PduEvent> {
	// MSC4186 sends the members of the timeline's senders, and of the targets
	// of its membership events
	let lazy_members: HashSet<OwnedUserId> = if required_state.lazy_members() {
		timeline
			.senders()
			.chain(timeline.pdus.iter().filter_map(|(_, pdu)| {
				(pdu.kind == RoomMember)
					.then(|| pdu.state_key.as_deref()?.try_into().ok())
					.flatten()
			}))
			.collect()
	} else {
		HashSet::new()
	};

	let lazy_keys = lazy_members
		.iter()
		.map(|user_id| (&StateEventType::RoomMember, user_id.as_str()));

	// look up exact keys rather than scanning rooms with many members
	let lookups: Vec<_> = if changes.initial {
		required_state
			.exact_keys(syncing_user)
			.chain(lazy_keys)
			.collect()
	} else {
		lazy_keys.collect()
	};

	let mut shorteventids: HashSet<ShortEventId> = lookups
		.into_iter()
		.stream()
		.broad_filter_map(async |(event_type, state_key)| {
			services
				.rooms
				.state_accessor
				.state_get_shortid(shortstatehash, event_type, state_key)
				.await
				.ok()
		})
		.collect()
		.await;

	let matches = |event_type: &StateEventType, state_key: &StateKey| {
		required_state.matches(event_type, state_key.as_str(), syncing_user)
	};

	// MSC4186: without a gap in the timeline, lazy loading also sends every
	// membership change
	let member_changes = required_state.lazy_members() && !timeline.limited;

	if !changes.initial {
		shorteventids.extend(
			changes
				.changed
				.iter()
				.filter(|((event_type, state_key), _)| {
					matches(event_type, state_key)
						|| (member_changes && *event_type == StateEventType::RoomMember)
				})
				.map(at!(1)),
		);
	} else if required_state.has_wildcard() {
		let (shortstatekeys, full_state): (Vec<_>, Vec<_>) = services
			.rooms
			.state_accessor
			.state_full_shortids(shortstatehash)
			.ignore_err()
			.unzip()
			.await;

		services
			.rooms
			.short
			.multi_get_statekey_from_short(shortstatekeys.into_iter().stream())
			.zip(full_state.into_iter().stream())
			.ready_filter_map(|(key, shorteventid)| {
				let (event_type, state_key) = key.ok()?;
				matches(&event_type, &state_key).then_some(shorteventid)
			})
			.ready_for_each(|shorteventid| {
				shorteventids.insert(shorteventid);
			})
			.await;
	}

	services
		.rooms
		.short
		.multi_get_eventid_from_short::<'_, OwnedEventId, _>(shorteventids.into_iter().stream())
		.ignore_err()
		.broad_filter_map(async |event_id| services.rooms.timeline.get_pdu(&event_id).await.ok())
		.collect()
		.await
}

async fn notification_counts_changed(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	since: Option<u64>,
	timeline: &TimelinePdus,
) -> bool {
	match since {
		| Some(since) if timeline.pdus.is_empty() && !timeline.limited =>
			services
				.rooms
				.user
				.last_notification_read(user_id, room_id)
				.await
				> since,
		| _ => true,
	}
}

async fn notification_counts(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
) -> UnreadNotificationsCount {
	let user = &services.rooms.user;
	let (notification_count, highlight_count) = join(
		user.notification_count(user_id, room_id),
		user.highlight_count(user_id, room_id),
	)
	.await;

	assign!(UnreadNotificationsCount::new(), {
		notification_count: Some(ruma_from_u64(notification_count)),
		highlight_count: Some(ruma_from_u64(highlight_count)),
	})
}

async fn heroes(
	services: &Services,
	syncing_user: &UserId,
	room_id: &RoomId,
	shortstatehash: ShortStateHash,
) -> Vec<v5::response::Hero> {
	let mut hero_ids: Vec<_> = build_heroes(services, room_id, syncing_user, shortstatehash)
		.await
		.into_iter()
		.collect();

	hero_ids.sort_unstable();

	hero_ids
		.into_iter()
		.stream()
		.wide_then(async |user_id| {
			let member: Option<RoomMemberEventContent> = services
				.rooms
				.state_accessor
				.state_get_content(shortstatehash, &StateEventType::RoomMember, user_id.as_str())
				.await
				.ok();

			let (name, avatar) = member
				.map(|member| (member.displayname, member.avatar_url))
				.unwrap_or_default();

			assign!(v5::response::Hero::new(user_id), { name, avatar })
		})
		.collect()
		.await
}

async fn latest_bump_stamp(
	services: &Services,
	room_id: &RoomId,
	before: PduCount,
	after: Option<PduCount>,
) -> Option<UInt> {
	let bump_stamps = services
		.rooms
		.timeline
		.pdus_rev(room_id, Some(before))
		.ignore_err()
		.ready_take_while(move |(count, _)| after.is_none_or(|after| *count > after))
		.take(BUMP_LOOKBACK)
		.ready_filter_map(|(_, pdu)| {
			BUMP_TYPES
				.contains(&pdu.kind)
				.then_some(pdu.origin_server_ts)
		});

	pin_mut!(bump_stamps);
	bump_stamps.next().await
}

/// Adds the membership events of the senders of the sticky events being sent
/// to rooms that lazy load their members, as [MSC4480] counts sticky events as
/// timeline events for lazy loading.
///
/// [MSC4480]: https://github.com/matrix-org/matrix-spec-proposals/pull/4480
pub(super) async fn add_sticky_senders(
	services: &Services,
	selection: &RoomSelection,
	rooms: &mut BTreeMap<OwnedRoomId, v5::response::Room>,
	sticky: &v5::response::StickyEvents,
) {
	for (room_id, sticky_room) in &sticky.rooms {
		if !selection
			.rooms
			.get(room_id)
			.is_some_and(|selected| selected.config.required_state.lazy_members())
		{
			continue;
		}

		let senders: HashSet<OwnedUserId> = sticky_room
			.events
			.iter()
			.filter_map(|event| event.get_field("sender").ok().flatten())
			.collect();

		let members: Vec<_> = senders
			.into_iter()
			.stream()
			.wide_filter_map(async |sender| {
				services
					.rooms
					.state_accessor
					.room_state_get(room_id, &StateEventType::RoomMember, sender.as_str())
					.await
					.ok()
					.map(Event::into_format)
			})
			.collect()
			.await;

		if !members.is_empty() {
			rooms
				.entry(room_id.clone())
				.or_default()
				.required_state
				.extend(members);
		}
	}
}

/// Counts the events at the end of the timeline that arrived after `since`.
/// Nothing is live on the first response of a connection.
fn num_live_events<'a>(
	counts: impl DoubleEndedIterator<Item = &'a PduCount>,
	since: u64,
) -> usize {
	if since == 0 {
		return 0;
	}

	counts
		.rev()
		.take_while(|count| matches!(count, PduCount::Normal(count) if *count > since))
		.count()
}

#[cfg(test)]
mod tests {
	use ruma::uint;

	use super::*;

	fn selected(membership: MembershipState, timeline_limit: UInt) -> SelectedRoom {
		SelectedRoom {
			membership,
			is_dm: false,
			config: Arc::new(RoomConfig::new(&[], timeline_limit)),
		}
	}

	fn previous(membership: MembershipState, timeline_limit: UInt) -> RoomState {
		RoomState {
			sent_at: 10,
			membership,
			is_dm: false,
			config: Arc::new(RoomConfig::new(&[], timeline_limit)),
		}
	}

	#[test]
	fn an_unchanged_room_continues_from_where_it_was_sent() {
		let previous = previous(MembershipState::Join, uint!(10));

		assert_eq!(since(&previous, &selected(MembershipState::Join, uint!(10))), Some(10));
		assert_eq!(since(&previous, &selected(MembershipState::Join, uint!(1))), Some(10));
	}

	#[test]
	fn a_wider_config_resends_the_room_in_full() {
		let previous = previous(MembershipState::Join, uint!(1));

		assert_eq!(since(&previous, &selected(MembershipState::Join, uint!(20))), None);
	}

	#[test]
	fn a_join_after_an_invite_is_sent_in_full_but_a_leave_is_a_change() {
		let invited = previous(MembershipState::Invite, uint!(1));
		let joined = previous(MembershipState::Join, uint!(1));

		assert_eq!(since(&invited, &selected(MembershipState::Join, uint!(1))), None);
		assert_eq!(since(&invited, &selected(MembershipState::Leave, uint!(1))), Some(10));
		assert_eq!(since(&joined, &selected(MembershipState::Ban, uint!(1))), Some(10));
	}

	#[test]
	fn num_live_events_counts_the_new_normal_suffix() {
		let counts = [
			PduCount::Backfilled(-2),
			PduCount::Normal(8),
			PduCount::Normal(11),
			PduCount::Normal(12),
		];

		assert_eq!(num_live_events(counts.iter(), 0), 0);
		assert_eq!(num_live_events(counts.iter(), 10), 2);
		assert_eq!(num_live_events(counts.iter(), 12), 0);
	}

	#[test]
	fn num_live_events_stops_at_history() {
		let counts = [PduCount::Normal(11), PduCount::Normal(9), PduCount::Normal(12)];

		assert_eq!(num_live_events(counts.iter(), 10), 1);
	}
}
