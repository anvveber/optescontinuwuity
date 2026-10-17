use std::{
	collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry},
	ops::Range,
	sync::Arc,
};

use conduwuit::{
	Event, PduEvent, Result,
	utils::{
		FutureBoolExt, IterStream,
		math::{ruma_from_usize, usize_from_ruma},
		stream::WidebandExt,
	},
	warn,
};
use conduwuit_service::{
	Services,
	sync::{LeftRoom, RoomConfig},
};
use futures::{StreamExt, future::join4, pin_mut};
use ruma::{
	OwnedRoomId, RoomId, UInt, UserId,
	api::client::sync::sync_events::v5,
	directory::RoomTypeFilter,
	events::{
		AnyStrippedStateEvent, GlobalAccountDataEventType, StateEventType,
		direct::DirectEvent,
		room::{
			create::RoomCreateEventContent,
			member::{MembershipState, RoomMemberEventContent},
		},
	},
	serde::Raw,
};

use super::SyncContext;
use crate::client::is_ignored_invite;

#[derive(Default)]
pub(super) struct RoomSelection {
	counts: BTreeMap<String, usize>,
	pub(super) in_range: BTreeMap<String, Vec<OwnedRoomId>>,
	pub(super) subscriptions: BTreeSet<OwnedRoomId>,
	pub(super) rooms: BTreeMap<OwnedRoomId, SelectedRoom>,
	pub(super) left_rooms: Option<HashMap<OwnedRoomId, LeftRoom>>,
}

pub(super) struct SelectedRoom {
	pub(super) membership: MembershipState,
	pub(super) is_dm: bool,
	pub(super) config: Arc<RoomConfig>,
}

/// A room the user is or was a member of.
struct Membership {
	room_id: OwnedRoomId,
	membership: MembershipState,
	/// When the user left, which orders a left room in the lists. `None` for
	/// other memberships, whose activity is looked up later.
	left_count: Option<u64>,
}

struct Candidate {
	room_id: OwnedRoomId,
	membership: MembershipState,
	activity: u64,
	is_dm: bool,
	/// Only loaded if a list filters on it.
	is_encrypted: bool,
	/// Only loaded if a list filters on it. `None` if unknown.
	room_type: Option<RoomTypeFilter>,
}

impl RoomSelection {
	pub(super) fn counts(&self) -> &BTreeMap<String, usize> { &self.counts }

	pub(super) fn lists(&self) -> BTreeMap<String, v5::response::List> {
		self.counts
			.iter()
			.map(|(list_id, count)| {
				let mut list = v5::response::List::default();
				list.count = ruma_from_usize(*count);
				(list_id.clone(), list)
			})
			.collect()
	}

	pub(super) fn is_joined(&self, room_id: &RoomId) -> bool {
		self.rooms
			.get(room_id)
			.is_some_and(|room| room.membership == MembershipState::Join)
	}

	fn select(
		&mut self,
		room_id: &RoomId,
		membership: &MembershipState,
		is_dm: bool,
		config: &Arc<RoomConfig>,
	) {
		match self.rooms.entry(room_id.to_owned()) {
			| Entry::Vacant(entry) => {
				entry.insert(SelectedRoom {
					membership: membership.clone(),
					is_dm,
					config: Arc::clone(config),
				});
			},
			| Entry::Occupied(mut entry) => {
				Arc::make_mut(&mut entry.get_mut().config).merge(config);
			},
		}
	}
}

/// Selects the rooms this response covers: those in range of each list, sorted
/// by activity, and the room subscriptions, each with the config combined from
/// every list and subscription that selected it.
pub(super) async fn select_rooms(
	services: &Services,
	context: SyncContext<'_>,
) -> Result<RoomSelection> {
	let SyncContext { request, .. } = context;

	let filters = || {
		request
			.lists
			.values()
			.filter_map(|list| list.filters.as_ref())
	};
	// also needed for `is_dm` on room results
	let direct_rooms = direct_rooms(services, context.syncing_user).await;

	let load_encryption = filters().any(|filters| filters.is_encrypted.is_some());
	let load_room_type = filters()
		.any(|filters| !filters.room_types.is_empty() || !filters.not_room_types.is_empty());

	let (memberships, left_rooms) = memberships(services, context).await;
	let mut candidates: Vec<Candidate> = memberships
		.into_iter()
		.stream()
		.wide_then(|membership| {
			candidate(
				services,
				context,
				membership,
				&direct_rooms,
				(load_encryption, load_room_type),
			)
		})
		.collect()
		.await;

	// most recently active first, with a stable order between equals
	candidates.sort_unstable_by(|left, right| {
		right
			.activity
			.cmp(&left.activity)
			.then_with(|| left.room_id.cmp(&right.room_id))
	});

	let mut selection = RoomSelection { left_rooms, ..Default::default() };
	for (list_id, list) in &request.lists {
		let config = Arc::new(RoomConfig::new(
			&list.room_details.required_state,
			list.room_details.timeline_limit,
		));

		let matching: Vec<&Candidate> = candidates
			.iter()
			.filter(|candidate| matches_filters(candidate, list.filters.as_ref()))
			.collect();

		let mut in_range = Vec::new();
		for range in list_indices(matching.len(), &list.ranges) {
			for candidate in &matching[range] {
				selection.select(
					&candidate.room_id,
					&candidate.membership,
					candidate.is_dm,
					&config,
				);
				in_range.push(candidate.room_id.clone());
			}
		}

		selection.counts.insert(list_id.clone(), matching.len());
		selection.in_range.insert(list_id.clone(), in_range);
	}

	let listed: HashMap<&RoomId, &MembershipState> = candidates
		.iter()
		.map(|candidate| (candidate.room_id.as_ref(), &candidate.membership))
		.collect();

	for (room_id, subscription) in &request.room_subscriptions {
		let membership = listed
			.get::<&RoomId>(&room_id.as_ref())
			.map(|&membership| membership.clone());

		let Some(membership) =
			subscription_membership(services, context, room_id, membership).await
		else {
			continue;
		};

		let config =
			Arc::new(RoomConfig::new(&subscription.required_state, subscription.timeline_limit));

		selection.select(room_id, &membership, direct_rooms.contains(room_id), &config);
		selection.subscriptions.insert(room_id.clone());
	}

	Ok(selection)
}

/// Returns the rooms the lists can show, and the user's left rooms if they
/// changed since the connection last saw them.
///
/// [MSC4186] lists rooms the user was kicked or banned from after joining, but
/// rooms they left themselves only once sent on this connection.
///
/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
async fn memberships(
	services: &Services,
	context: SyncContext<'_>,
) -> (Vec<Membership>, Option<HashMap<OwnedRoomId, LeftRoom>>) {
	let SyncContext { syncing_user, connection, .. } = context;
	let state_cache = &services.rooms.state_cache;

	let joined = state_cache
		.rooms_joined(syncing_user)
		.map(|room_id| Membership {
			room_id,
			membership: MembershipState::Join,
			left_count: None,
		})
		.collect::<Vec<_>>();

	let invited = state_cache
		.rooms_invited(syncing_user)
		.wide_filter_map(async |(room_id, _)| {
			(!is_ignored_invite(services, syncing_user, &room_id).await).then_some(Membership {
				room_id,
				membership: MembershipState::Invite,
				left_count: None,
			})
		})
		.collect::<Vec<_>>();

	let knocked = state_cache
		.rooms_knocked(syncing_user)
		.map(|(room_id, _)| Membership {
			room_id,
			membership: MembershipState::Knock,
			left_count: None,
		})
		.collect::<Vec<_>>();

	let left = state_cache
		.rooms_left_ids(syncing_user)
		.wide_filter_map(async |room_id| {
			let left_count = state_cache
				.get_left_count(&room_id, syncing_user)
				.await
				.ok()?;

			let (left, loaded) = match connection.left_rooms.get(&room_id) {
				| Some(left) if left.left_count == left_count => (left.clone(), false),
				| _ => (load_left_room(services, syncing_user, &room_id, left_count).await, true),
			};

			Some((room_id, left, loaded))
		})
		.collect::<Vec<_>>();

	let (joined, invited, knocked, left) = join4(joined, invited, knocked, left).await;

	let mut memberships: Vec<_> = joined.into_iter().chain(invited).chain(knocked).collect();
	let mut changed = left.len() != connection.left_rooms.len();
	let mut left_rooms = HashMap::with_capacity(left.len());
	for (room_id, left, loaded) in left {
		if left.removed || connection.rooms.contains_key(&room_id) {
			memberships.push(Membership {
				room_id: room_id.clone(),
				membership: left.membership.clone(),
				left_count: Some(left.left_count),
			});
		}

		changed |= loaded;
		left_rooms.insert(room_id, left);
	}

	// the whole map is replaced, so rooms the user has since rejoined drop out
	(memberships, changed.then_some(left_rooms))
}

async fn load_left_room(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	left_count: u64,
) -> LeftRoom {
	let state_cache = &services.rooms.state_cache;
	let leave_pdu = state_cache
		.left_state(user_id, room_id)
		.await
		.ok()
		.flatten();

	let membership = left_membership(leave_pdu.as_ref());
	let removed = leave_pdu.is_some_and(|pdu| pdu.sender() != user_id)
		&& state_cache.once_joined(user_id, room_id).await;

	LeftRoom { left_count, membership, removed }
}

/// Returns the membership the user's leave event set, `leave` or `ban`. Without
/// a leave event the user was removed by a server admin, which counts as a
/// leave.
fn left_membership(leave_pdu: Option<&PduEvent>) -> MembershipState {
	leave_pdu
		.and_then(|pdu| pdu.get_content::<RoomMemberEventContent>().ok())
		.map_or(MembershipState::Leave, |content| content.membership)
}

/// Loads what the lists sort and filter `membership`'s room by.
///
/// A room is ordered by its latest activity, or for a room the user is not
/// joined to, by when their membership changed. Whether the room is encrypted
/// and its type are only loaded if a list filters on them, as both need the
/// room's state.
async fn candidate(
	services: &Services,
	context: SyncContext<'_>,
	Membership { room_id, membership, left_count }: Membership,
	direct_rooms: &HashSet<OwnedRoomId>,
	(load_encryption, load_room_type): (bool, bool),
) -> Candidate {
	let is_dm = direct_rooms.contains(&room_id);
	let SyncContext { syncing_user, .. } = context;
	let state_cache = &services.rooms.state_cache;

	let activity = match (left_count, &membership) {
		| (Some(left_count), _) => Ok(left_count),
		| (None, MembershipState::Join) => services.rooms.timeline.room_activity(&room_id).await,
		| (None, MembershipState::Invite) =>
			state_cache.get_invite_count(&room_id, syncing_user).await,
		| (None, MembershipState::Knock) =>
			state_cache.get_knock_count(&room_id, syncing_user).await,
		| (None, _) => state_cache.get_left_count(&room_id, syncing_user).await,
	}
	.unwrap_or(0);

	let is_encrypted = if load_encryption {
		is_encrypted(services, syncing_user, &room_id, &membership).await
	} else {
		false
	};

	let room_type = if load_room_type {
		room_type(services, syncing_user, &room_id, &membership).await
	} else {
		None
	};

	Candidate {
		room_id,
		membership,
		activity,
		is_dm,
		is_encrypted,
		room_type,
	}
}

/// Returns the rooms the user's `m.direct` account data marks as DMs.
async fn direct_rooms(services: &Services, user_id: &UserId) -> HashSet<OwnedRoomId> {
	// invalid data counts as no DMs rather than failing every sync
	match services
		.account_data
		.get_global::<DirectEvent>(user_id, GlobalAccountDataEventType::Direct)
		.await
	{
		| Ok(event) => event.content.0.into_values().flatten().collect(),
		| Err(error) => {
			if !error.is_not_found() {
				warn!(%user_id, %error, "Failed to read m.direct for sliding sync");
			}

			HashSet::new()
		},
	}
}

/// Returns whether the room has an `m.room.encryption` state event.
///
/// Rooms the user is only invited to or knocking on have no state locally,
/// so their stripped state answers for them instead.
async fn is_encrypted(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	membership: &MembershipState,
) -> bool {
	if !matches!(membership, MembershipState::Invite | MembershipState::Knock) {
		return services
			.rooms
			.state_accessor
			.is_encrypted_room(room_id)
			.await;
	}

	stripped_state_event(services, user_id, room_id, membership, &StateEventType::RoomEncryption)
		.await
		.is_some()
}

/// Returns the room's type from its `m.room.create` event, or `None` if it
/// could not be loaded.
async fn room_type(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	membership: &MembershipState,
) -> Option<RoomTypeFilter> {
	if matches!(membership, MembershipState::Invite | MembershipState::Knock) {
		// the stripped state of an invite or knock is not required to include
		// `m.room.create`, so a room without it is treated as having no type
		// rather than being hidden from every list filtering on types
		let content: Option<RoomCreateEventContent> = stripped_state_event(
			services,
			user_id,
			room_id,
			membership,
			&StateEventType::RoomCreate,
		)
		.await
		.and_then(|event| event.get_field("content").ok().flatten());

		return Some(RoomTypeFilter::from(content.and_then(|content| content.room_type)));
	}

	match services.rooms.state_accessor.get_room_type(room_id).await {
		| Ok(room_type) => Some(RoomTypeFilter::from(Some(room_type))),
		| Err(error) if error.is_not_found() => Some(RoomTypeFilter::Default),
		| Err(error) => {
			warn!(%room_id, %error, "Failed to fetch room type for a sliding sync list filter");
			None
		},
	}
}

async fn stripped_state_event(
	services: &Services,
	user_id: &UserId,
	room_id: &RoomId,
	membership: &MembershipState,
	event_type: &StateEventType,
) -> Option<Raw<AnyStrippedStateEvent>> {
	let state_cache = &services.rooms.state_cache;
	let state = if *membership == MembershipState::Knock {
		state_cache.knock_state(user_id, room_id).await
	} else {
		state_cache.invite_state(user_id, room_id).await
	};

	state.ok()?.into_iter().find(|event| {
		event
			.get_field::<StateEventType>("type")
			.ok()
			.flatten()
			.as_ref()
			== Some(event_type)
	})
}

/// Returns the user's membership in a subscribed room, or `None` if the user
/// may not see the room.
///
/// `listed` is the membership found while building the lists. Rooms the user
/// left are visible even if they never joined, e.g. after rejecting an invite,
/// but not rooms they were banned from without ever joining.
async fn subscription_membership(
	services: &Services,
	context: SyncContext<'_>,
	room_id: &RoomId,
	listed: Option<MembershipState>,
) -> Option<MembershipState> {
	let SyncContext { syncing_user, .. } = context;
	let metadata = &services.rooms.metadata;

	let is_disabled = metadata.is_disabled(room_id);
	let is_banned = metadata.is_banned(room_id);
	pin_mut!(is_disabled, is_banned);
	if is_disabled.or(is_banned).await {
		return None;
	}

	// an invite may be to a room this server has no state for
	if listed.is_some() {
		return listed;
	}

	if !metadata.exists(room_id).await {
		return None;
	}

	let state_cache = &services.rooms.state_cache;
	if !state_cache.is_left(syncing_user, room_id).await {
		return None;
	}

	let leave_pdu = state_cache
		.left_state(syncing_user, room_id)
		.await
		.ok()
		.flatten();

	match left_membership(leave_pdu.as_ref()) {
		| MembershipState::Ban if !state_cache.once_joined(syncing_user, room_id).await => None,
		| membership => Some(membership),
	}
}

fn matches_filters(candidate: &Candidate, filters: Option<&v5::request::ListFilters>) -> bool {
	// TODO MSC4186: ruma's `ListFilters` does not model `spaces`, `tags` or
	// `not_tags`, nor the rename of `is_invite` to `is_invited`.
	let Some(filters) = filters else {
		return true;
	};

	let is_invite = candidate.membership == MembershipState::Invite;
	let bool_filters = [
		(candidate.is_dm, filters.is_dm),
		(candidate.is_encrypted, filters.is_encrypted),
		(is_invite, filters.is_invite),
	];

	if !bool_filters
		.into_iter()
		.all(|(value, filter)| filter.is_none_or(|expected| value == expected))
	{
		return false;
	}

	if filters.room_types.is_empty() && filters.not_room_types.is_empty() {
		return true;
	}

	// a room whose type could not be loaded is left out of type filtered lists
	let Some(room_type) = &candidate.room_type else {
		return false;
	};

	if filters.not_room_types.contains(room_type) {
		return false;
	}

	filters.room_types.is_empty() || filters.room_types.contains(room_type)
}

/// Returns the disjoint index ranges a list's inclusive `[start, end]` ranges
/// cover.
///
/// Under [MSC4186], a list without ranges holds every matching room.
///
/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
#[allow(clippy::single_range_in_vec_init, reason = "these are lists of index ranges")]
fn list_indices(len: usize, ranges: &[(UInt, UInt)]) -> Vec<Range<usize>> {
	if ranges.is_empty() {
		return vec![0..len];
	}

	let mut ranges: Vec<_> = ranges
		.iter()
		.map(|&(start, end)| {
			let start = usize_from_ruma(start).min(len);
			start..usize_from_ruma(end).saturating_add(1).clamp(start, len)
		})
		.filter(|range| !range.is_empty())
		.collect();

	ranges.sort_unstable_by_key(|range| range.start);

	let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
	for range in ranges {
		match merged.last_mut() {
			| Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
			| _ => merged.push(range),
		}
	}

	merged
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init, reason = "these are lists of index ranges")]
mod tests {
	use ruma::uint;

	use super::*;

	fn candidate(room_id: &str, membership: MembershipState, activity: u64) -> Candidate {
		Candidate {
			room_id: room_id.try_into().unwrap(),
			membership,
			activity,
			is_dm: false,
			is_encrypted: false,
			room_type: Some(RoomTypeFilter::Default),
		}
	}

	fn custom_room_type() -> RoomTypeFilter { RoomTypeFilter::from(Some("com.example")) }

	#[test]
	fn ranges_are_inclusive_and_clamped() {
		assert_eq!(list_indices(10, &[(uint!(0), uint!(4))]), [0..5]);
		assert_eq!(list_indices(3, &[(uint!(0), uint!(20))]), [0..3]);
		assert!(list_indices(3, &[(uint!(5), uint!(20))]).is_empty());
		assert!(list_indices(3, &[(uint!(2), uint!(1))]).is_empty());
	}

	#[test]
	fn overlapping_ranges_are_merged() {
		let ranges = [(uint!(5), uint!(9)), (uint!(0), uint!(4)), (uint!(2), uint!(6))];

		assert_eq!(list_indices(20, &ranges), [0..10]);
		assert_eq!(list_indices(20, &[(uint!(0), uint!(1)), (uint!(5), uint!(6))]), [0..2, 5..7]);
	}

	#[test]
	fn a_list_without_ranges_holds_every_room() {
		assert_eq!(list_indices(4, &[]), [0..4]);
	}

	#[test]
	fn absent_bool_filter_matches_either_value() {
		let mut dm = candidate("!a:example.com", MembershipState::Join, 0);
		dm.is_dm = true;
		let other = candidate("!b:example.com", MembershipState::Join, 0);
		let filters = v5::request::ListFilters::default();

		assert!(matches_filters(&dm, Some(&filters)));
		assert!(matches_filters(&other, Some(&filters)));
	}

	#[test]
	fn bool_filter_matches_only_its_own_value() {
		let mut dm = candidate("!a:example.com", MembershipState::Join, 0);
		dm.is_dm = true;
		let other = candidate("!b:example.com", MembershipState::Join, 0);
		let mut filters = v5::request::ListFilters::default();

		filters.is_dm = Some(true);
		assert!(matches_filters(&dm, Some(&filters)));
		assert!(!matches_filters(&other, Some(&filters)));

		filters.is_dm = Some(false);
		assert!(!matches_filters(&dm, Some(&filters)));
		assert!(matches_filters(&other, Some(&filters)));
	}

	#[test]
	fn is_invite_separates_invites_from_other_memberships() {
		let invite = candidate("!a:example.com", MembershipState::Invite, 0);
		let left = candidate("!b:example.com", MembershipState::Leave, 0);
		let mut filters = v5::request::ListFilters::default();

		filters.is_invite = Some(true);
		assert!(matches_filters(&invite, Some(&filters)));
		assert!(!matches_filters(&left, Some(&filters)));

		filters.is_invite = Some(false);
		assert!(!matches_filters(&invite, Some(&filters)));
		assert!(matches_filters(&left, Some(&filters)));
	}

	#[test]
	fn an_unknown_room_type_is_excluded_only_from_type_filtered_lists() {
		let mut unknown = candidate("!a:example.com", MembershipState::Join, 0);
		unknown.room_type = None;
		let mut filters = v5::request::ListFilters::default();

		assert!(matches_filters(&unknown, Some(&filters)));

		filters.not_room_types = vec![RoomTypeFilter::Space];
		assert!(!matches_filters(&unknown, Some(&filters)));
	}

	#[test]
	fn positive_room_type_matches_and_mismatches() {
		let mut custom = candidate("!a:example.com", MembershipState::Join, 0);
		custom.room_type = Some(custom_room_type());
		let untyped = candidate("!b:example.com", MembershipState::Join, 0);
		let mut filters = v5::request::ListFilters::default();

		filters.room_types = vec![custom_room_type()];
		assert!(matches_filters(&custom, Some(&filters)));
		assert!(!matches_filters(&untyped, Some(&filters)));
	}

	#[test]
	fn negative_room_type_overrides_positive() {
		let mut custom = candidate("!a:example.com", MembershipState::Join, 0);
		custom.room_type = Some(custom_room_type());
		let untyped = candidate("!b:example.com", MembershipState::Join, 0);
		let mut filters = v5::request::ListFilters::default();

		filters.room_types = vec![custom_room_type()];
		filters.not_room_types = vec![custom_room_type()];
		assert!(!matches_filters(&custom, Some(&filters)));

		filters.room_types = Vec::new();
		filters.not_room_types = vec![RoomTypeFilter::Space];
		assert!(matches_filters(&untyped, Some(&filters)));
	}

	#[test]
	fn a_leave_without_an_event_is_a_leave() {
		assert_eq!(left_membership(None), MembershipState::Leave);
	}
}
