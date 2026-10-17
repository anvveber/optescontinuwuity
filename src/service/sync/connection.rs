use std::{
	collections::{BTreeMap, HashMap, HashSet},
	fmt,
	str::FromStr,
	sync::Arc,
};

use ruma::{OwnedDeviceId, OwnedRoomId, OwnedUserId, events::room::member::MembershipState};

use super::RoomConfig;

/// How many unacknowledged responses a connection keeps. A client that cancels
/// and retries a request may go on with any of the responses built for it, so
/// more than the latest one is kept. Acknowledged state is dropped straight
/// away.
const MAX_PENDING: usize = 8;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConnectionKey {
	pub user_id: OwnedUserId,
	pub device_id: OwnedDeviceId,
	pub conn_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionState {
	/// Keeps responses of a restarted connection off its successor.
	pub id: u64,
	pub count: Option<u64>,
	pub rooms: HashMap<OwnedRoomId, RoomState>,
	pub left_rooms: HashMap<OwnedRoomId, LeftRoom>,
	pub list_counts: BTreeMap<String, usize>,
	/// Whether the acknowledged response delivered every to-device message up
	/// to its `pos`.
	pub to_device_through_pos: bool,
	pub typing: ExtensionScope,
}

#[derive(Clone, Debug)]
pub struct RoomState {
	pub sent_at: u64,
	pub membership: MembershipState,
	pub is_dm: bool,
	/// The combined config when the room was last selected.
	pub config: Arc<RoomConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeftRoom {
	pub left_count: u64,
	/// `leave` or `ban`.
	pub membership: MembershipState,
	/// Kicked or banned after joining, which keeps the room listed.
	pub removed: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionScope {
	pub enabled: bool,
	pub rooms: HashSet<OwnedRoomId>,
	/// When rooms that left the scope were last covered.
	pub left_at: HashMap<OwnedRoomId, u64>,
}

/// Changes to a connection's state, applied once the client acknowledges the
/// response they were built with.
#[derive(Debug, Default)]
pub struct ConnectionUpdate {
	pub rooms: HashMap<OwnedRoomId, RoomState>,
	pub left_rooms: Option<HashMap<OwnedRoomId, LeftRoom>>,
	pub list_counts: Option<BTreeMap<String, usize>>,
	pub to_device_through_pos: bool,
	pub typing: Option<ExtensionScope>,
}

/// A sliding sync `pos`, sent as `{count}_{seq}`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pos {
	/// The global count the response was built up to.
	pub count: u64,
	/// Unique to each response, which keeps a retried request from committing
	/// a response the client never received, even when the stream count has
	/// not moved.
	pub(super) seq: u64,
}

pub(super) struct Connection {
	pos: Option<Pos>,
	state: Arc<ConnectionState>,
	pending: BTreeMap<u64, (Pos, ConnectionUpdate)>,
}

impl ExtensionScope {
	/// Returns the count to send `room_id`'s changes from: `count` if the room
	/// is in scope, the count it left the scope at if it was in scope before,
	/// or zero if everything must be sent.
	#[must_use]
	pub fn since(&self, room_id: &OwnedRoomId, count: u64) -> u64 {
		if !self.enabled {
			return 0;
		}

		if self.rooms.contains(room_id) {
			return count;
		}

		self.left_at.get(room_id).copied().unwrap_or(0)
	}

	/// Returns the scope after this response, covering `rooms` and remembering
	/// when the rooms that left it were last covered. Returns `None` if the
	/// scope did not change.
	#[must_use]
	pub fn advance(&self, rooms: HashSet<OwnedRoomId>, count: u64) -> Option<Self> {
		if self.enabled && self.rooms == rooms {
			return None;
		}

		let mut left_at = self.left_at.clone();
		for room_id in self.rooms.difference(&rooms) {
			left_at.insert(room_id.clone(), count);
		}
		for room_id in &rooms {
			left_at.remove(room_id);
		}

		Some(Self { enabled: true, rooms, left_at })
	}
}

impl ConnectionState {
	fn apply(&mut self, update: ConnectionUpdate) {
		self.rooms.extend(update.rooms);
		if let Some(left_rooms) = update.left_rooms {
			self.left_rooms = left_rooms;
		}

		if let Some(list_counts) = update.list_counts {
			self.list_counts = list_counts;
		}

		self.to_device_through_pos = update.to_device_through_pos;
		if let Some(typing) = update.typing {
			self.typing = typing;
		}
	}
}

impl Connection {
	pub(super) fn new(id: u64) -> Self {
		Self {
			pos: None,
			state: Arc::new(ConnectionState { id, ..Default::default() }),
			pending: BTreeMap::new(),
		}
	}

	/// Commits the response `pos` was sent with and drops the responses built
	/// before it, as the client has moved past them.
	///
	/// Returns false if `pos` is neither the current position nor a pending
	/// response, in which case the client must start over.
	pub(super) fn acknowledge(&mut self, pos: Pos) -> bool {
		if self.pos == Some(pos) {
			return true;
		}

		let Some((pending_pos, _)) = self.pending.get(&pos.seq) else {
			return false;
		};

		if *pending_pos != pos {
			return false;
		}

		let newer = self.pending.split_off(&pos.seq.saturating_add(1));
		let (_, update) = self
			.pending
			.remove(&pos.seq)
			.expect("pending response was just found");

		self.pending = newer;
		let state = Arc::make_mut(&mut self.state);
		state.apply(update);
		state.count = Some(pos.count);
		self.pos = Some(pos);
		true
	}

	pub(super) fn state(&self) -> Arc<ConnectionState> { Arc::clone(&self.state) }

	pub(super) fn id(&self) -> u64 { self.state.id }

	/// `pos.seq` must be unique across connections.
	pub(super) fn stage(&mut self, pos: Pos, update: ConnectionUpdate) {
		self.pending.insert(pos.seq, (pos, update));
		while self.pending.len() > MAX_PENDING {
			self.pending.pop_first();
		}
	}
}

impl fmt::Display for Pos {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}_{}", self.count, self.seq)
	}
}

impl FromStr for Pos {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let (count, seq) = s.split_once('_').ok_or(())?;

		Ok(Self {
			count: count.parse().map_err(|_| ())?,
			seq: seq.parse().map_err(|_| ())?,
		})
	}
}

#[cfg(test)]
mod tests {
	use ruma::owned_room_id;

	use super::*;

	struct Seq(u64);

	impl Seq {
		fn stage(
			&mut self,
			connection: &mut Connection,
			count: u64,
			update: ConnectionUpdate,
		) -> Pos {
			let pos = Pos { count, seq: self.0 };
			self.0 = self.0.saturating_add(1);
			connection.stage(pos, update);
			pos
		}
	}

	fn sent(room_id: &OwnedRoomId, sent_at: u64) -> ConnectionUpdate {
		ConnectionUpdate {
			rooms: HashMap::from([(room_id.clone(), RoomState {
				sent_at,
				membership: MembershipState::Join,
				is_dm: false,
				config: Arc::default(),
			})]),
			..Default::default()
		}
	}

	#[test]
	fn pos_round_trips() {
		let pos = Pos { count: 42, seq: 7 };

		assert_eq!(pos.to_string().parse(), Ok(pos));
		"42".parse::<Pos>().unwrap_err();
		"a_b".parse::<Pos>().unwrap_err();
	}

	#[test]
	fn a_response_is_committed_only_once_acknowledged() {
		let room_id = owned_room_id!("!room:example.com");
		let mut connection = Connection::new(0);
		let mut seq = Seq(0);
		let pos = seq.stage(&mut connection, 10, sent(&room_id, 10));

		assert!(connection.state().rooms.is_empty());
		assert!(connection.acknowledge(pos));
		assert_eq!(connection.state().count, Some(10));
		assert_eq!(connection.state().rooms[&room_id].sent_at, 10);
	}

	#[test]
	fn retrying_a_position_does_not_commit_the_unreceived_response() {
		let room_id = owned_room_id!("!room:example.com");
		let mut connection = Connection::new(0);
		let mut seq = Seq(0);
		let first = seq.stage(&mut connection, 10, ConnectionUpdate::default());
		assert!(connection.acknowledge(first));

		// the response to this request is lost, and the client retries `first`
		let lost = seq.stage(&mut connection, 10, sent(&room_id, 10));
		assert_eq!(lost.count, first.count);
		assert!(connection.acknowledge(first));

		assert!(connection.state().rooms.is_empty());
	}

	#[test]
	fn acknowledging_drops_older_responses_only() {
		let mut connection = Connection::new(0);
		let mut seq = Seq(0);
		let older = seq.stage(&mut connection, 1, ConnectionUpdate::default());
		let acked = seq.stage(&mut connection, 2, ConnectionUpdate::default());
		let newer = seq.stage(&mut connection, 3, ConnectionUpdate::default());

		assert!(connection.acknowledge(acked));
		assert!(!connection.acknowledge(older));
		assert!(connection.acknowledge(newer));
	}

	#[test]
	fn unknown_or_forged_positions_are_rejected() {
		let mut connection = Connection::new(0);
		let mut seq = Seq(0);
		let pos = seq.stage(&mut connection, 5, ConnectionUpdate::default());

		assert!(!connection.acknowledge(Pos { count: 6, seq: pos.seq }));
		assert!(!connection.acknowledge(Pos { count: 5, seq: pos.seq.saturating_add(1) }));
	}

	#[test]
	fn pending_responses_are_bounded() {
		let mut connection = Connection::new(0);
		let mut seq = Seq(0);
		let first = seq.stage(&mut connection, 0, ConnectionUpdate::default());
		for _ in 0..MAX_PENDING {
			seq.stage(&mut connection, 1, ConnectionUpdate::default());
		}

		assert_eq!(connection.pending.len(), MAX_PENDING);
		assert!(!connection.acknowledge(first));
	}

	#[test]
	fn extension_scope_tracks_rooms_entering_and_leaving() {
		let stays = owned_room_id!("!stays:example.com");
		let leaves = owned_room_id!("!leaves:example.com");
		let fresh = owned_room_id!("!fresh:example.com");

		let disabled = ExtensionScope::default();
		assert_eq!(disabled.since(&stays, 10), 0);

		let scope = disabled
			.advance(HashSet::from([stays.clone(), leaves.clone()]), 10)
			.unwrap();
		assert_eq!(scope.since(&stays, 10), 10);
		assert_eq!(scope.since(&fresh, 10), 0);
		assert!(
			scope
				.advance(HashSet::from([stays.clone(), leaves.clone()]), 20)
				.is_none()
		);

		let scope = scope.advance(HashSet::from([stays]), 20).unwrap();
		assert_eq!(scope.since(&leaves, 30), 20);
	}
}
