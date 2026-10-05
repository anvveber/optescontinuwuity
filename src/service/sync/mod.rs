mod connection;
mod room_config;

use std::{
	collections::HashMap,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
	time::{Duration, Instant},
};

use conduwuit::{Err, Result, SyncMutex, trace, utils::with_lock::WithLock};
use futures::StreamExt;
use ruma::{DeviceId, OwnedDeviceId, OwnedUserId, RoomId, UserId, events::TimelineEventType};
use tokio::sync::{Mutex, watch};

use self::connection::Connection;
pub use self::{
	connection::{
		ConnectionKey, ConnectionState, ConnectionUpdate, ExtensionScope, LeftRoom, Pos,
		RoomState,
	},
	room_config::{RequiredState, RoomConfig},
};
use crate::{Dep, rooms};

const CONNECTION_TTL: Duration = Duration::from_hours(7 * 24);

const MAX_CONNECTIONS_PER_DEVICE: usize = 8;

const SWEEP_INTERVAL: Duration = Duration::from_hours(1);

type DeviceConnections = HashMap<Option<String>, Entry>;

pub struct Service {
	services: Services,
	wakers: Wakers,
	connections: SyncMutex<HashMap<(OwnedUserId, OwnedDeviceId), DeviceConnections>>,
	last_sweep: SyncMutex<Instant>,
	/// Connection IDs and response sequence numbers.
	next_seq: AtomicU64,
}

struct Services {
	state_cache: Dep<rooms::state_cache::Service>,
}

/// What changed for a user, so a long-poll only ends for changes it reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeKind {
	Timeline,
	Typing,
	Receipts,
	AccountData,
	ToDevice,
	Keys,
	DeviceLists,
}

impl WakeKind {
	pub const ALL: [Self; 7] = [
		Self::Timeline,
		Self::Typing,
		Self::Receipts,
		Self::AccountData,
		Self::ToDevice,
		Self::Keys,
		Self::DeviceLists,
	];

	const fn index(self) -> usize {
		match self {
			| Self::Timeline => 0,
			| Self::Typing => 1,
			| Self::Receipts => 2,
			| Self::AccountData => 3,
			| Self::ToDevice => 4,
			| Self::Keys => 5,
			| Self::DeviceLists => 6,
		}
	}

	/// What a timeline event changes: a membership or encryption event also
	/// changes who shares an encrypted room.
	#[must_use]
	pub fn of_event(kind: &TimelineEventType) -> &'static [Self] {
		match kind {
			| TimelineEventType::RoomMember | TimelineEventType::RoomEncryption =>
				&[Self::Timeline, Self::DeviceLists],
			| _ => &[Self::Timeline],
		}
	}
}

type Generations = [u64; WakeKind::ALL.len()];

#[derive(Default)]
struct Wakers(Mutex<HashMap<OwnedUserId, watch::Sender<Generations>>>);

impl Wakers {
	async fn subscribe(&self, user: &UserId, interest: &[WakeKind]) -> WakeSubscription {
		let mut receiver = self
			.0
			.lock()
			.await
			.entry(user.to_owned())
			.or_insert_with(|| watch::channel(Generations::default()).0)
			.subscribe();

		let seen = *receiver.borrow_and_update();

		WakeSubscription {
			receiver,
			interest: interest.to_vec(),
			seen,
		}
	}

	async fn wake(&self, user: &UserId, kinds: &[WakeKind]) {
		let mut wakers = self.0.lock().await;
		wake_waker(&mut wakers, user, kinds);
	}

	async fn wake_all<I>(&self, users: I, kinds: &[WakeKind])
	where
		I: IntoIterator<Item = OwnedUserId> + Send,
	{
		let mut wakers = self.0.lock().await;
		for user in users {
			wake_waker(&mut wakers, &user, kinds);
		}
	}
}

/// Bumps the generation of each kind observed by anyone long-polling for this
/// user, reaping the entry once its last subscriber is gone.
fn wake_waker(
	wakers: &mut HashMap<OwnedUserId, watch::Sender<Generations>>,
	user: &UserId,
	kinds: &[WakeKind],
) {
	let Some(waker) = wakers.get(user) else {
		return;
	};

	if waker.receiver_count() == 0 {
		wakers.remove(user);
		return;
	}

	waker.send_modify(|generations| {
		for kind in kinds {
			let generation = &mut generations[kind.index()];
			*generation = generation.wrapping_add(1);
		}
	});
}

/// A long-poll's view of its user's wakes. A wake that lands between building
/// a response and waiting is still seen, since kinds are compared against the
/// generations last seen rather than against a notification.
pub struct WakeSubscription {
	receiver: watch::Receiver<Generations>,
	interest: Vec<WakeKind>,
	seen: Generations,
}

impl WakeSubscription {
	/// Waits for a wake of a kind this subscription is interested in and
	/// returns the kinds it saw, or `None` once the user's waker is gone.
	pub async fn woken(&mut self) -> Option<Vec<WakeKind>> {
		loop {
			let current = *self.receiver.borrow_and_update();
			let woken: Vec<_> = self
				.interest
				.iter()
				.copied()
				.filter(|kind| current[kind.index()] != self.seen[kind.index()])
				.collect();

			self.seen = current;
			if !woken.is_empty() {
				return Some(woken);
			}

			self.receiver.changed().await.ok()?;
		}
	}
}

impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			services: Services {
				state_cache: args.depend::<rooms::state_cache::Service>("rooms::state_cache"),
			},
			wakers: Wakers::default(),
			connections: SyncMutex::new(HashMap::new()),
			last_sweep: SyncMutex::new(Instant::now()),
			next_seq: AtomicU64::new(0),
		}))
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	pub async fn subscribe_to_wake(
		&self,
		user: &UserId,
		interest: &[WakeKind],
	) -> WakeSubscription {
		self.wakers.subscribe(user, interest).await
	}

	/// Wake the target user's sync loop. Call this when something
	/// that gets included in a legacy sync response changes.
	///
	/// Be careful where you call this function! In particular, don't call
	/// it in any function that's called by `append_pdu`. `append_pdu` will call
	/// it _after_ it's done appending a PDU, and calling it earlier can cause
	/// hard-to-diagnose race conditions.
	pub async fn wake(&self, user: &UserId, kinds: &[WakeKind]) {
		trace!(?user, ?kinds, "Waking user's sync loops");

		self.wakers.wake(user, kinds).await;
	}

	/// Wake all of our users who are joined to the specified room.
	pub async fn wake_all_joined(&self, room: &RoomId, kinds: &[WakeKind]) {
		trace!(?room, ?kinds, "Waking all joined users' sync loops");
		let users: Vec<_> = self
			.services
			.state_cache
			.active_local_users_in_room(room)
			.collect()
			.await;

		self.wakers.wake_all(users, kinds).await;
	}

	/// Commits the response `pos` acknowledges. Without a `pos` the connection
	/// starts over.
	pub fn begin_request(
		&self,
		key: &ConnectionKey,
		pos: Option<&str>,
	) -> Result<Arc<ConnectionState>> {
		let Ok(pos) = pos.map(str::parse::<Pos>).transpose() else {
			let removed = self.connections.with_lock(|connections| {
				connections
					.get_mut(&device_key(key))
					.and_then(|device| device.remove(&key.conn_id))
			});

			drop(removed);
			return Err!(Request(UnknownPos("Unrecognised sync position.")));
		};

		let connection = match pos {
			| None => self.start(key),
			| Some(pos) => {
				let resumed = self.resume(key);
				let acknowledged = resumed.as_ref().is_some_and(|connection| {
					connection.with_lock(|connection| connection.acknowledge(pos))
				});

				let (Some(connection), true) = (&resumed, acknowledged) else {
					// leave a connection another request just started alone
					if let Some(resumed) = &resumed {
						drop(self.remove(key, resumed));
					}

					return Err!(Request(UnknownPos(
						"Connection data unknown to server; restarting sync stream."
					)));
				};

				Arc::clone(connection)
			},
		};

		Ok(connection.with_lock(|connection| connection.state()))
	}

	/// `connection` is the state the response was built from.
	pub fn finish_request(
		&self,
		key: &ConnectionKey,
		connection: &ConnectionState,
		count: u64,
		update: ConnectionUpdate,
	) -> Pos {
		let pos = Pos {
			count,
			seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
		};

		let current = self.connections.with_lock(|connections| {
			connections
				.get(&device_key(key))
				.and_then(|device| device.get(&key.conn_id))
				.map(|entry| Arc::clone(&entry.connection))
		});

		// `with_lock` takes an `FnMut`, so the update is moved out of an option
		let mut update = Some(update);
		if let Some(current) = current {
			current.with_lock(|current| {
				// a connection restarted meanwhile never accepts this position
				if current.id() == connection.id
					&& let Some(update) = update.take()
				{
					current.stage(pos, update);
				}
			});
		}

		pos
	}

	pub fn forget_device(&self, user_id: &UserId, device_id: &DeviceId) {
		let key = (user_id.to_owned(), device_id.to_owned());
		let removed = self
			.connections
			.with_lock(|connections| connections.remove(&key));

		drop(removed);
	}

	fn start(&self, key: &ConnectionKey) -> Arc<SyncMutex<Connection>> {
		let expired = self.sweep_expired();
		let id = self.next_seq.fetch_add(1, Ordering::Relaxed);
		let connection = Arc::new(SyncMutex::new(Connection::new(id)));

		let replaced = self.connections.with_lock(|connections| {
			let device = connections.entry(device_key(key)).or_default();
			let mut replaced: Vec<_> = device.remove(&key.conn_id).into_iter().collect();
			replaced.extend(evict_least_recently_used(device));

			device.insert(key.conn_id.clone(), Entry {
				connection: Arc::clone(&connection),
				last_used: Instant::now(),
			});

			replaced
		});

		// connection state can be large, so it is freed outside the lock
		drop((expired, replaced));
		connection
	}

	fn resume(&self, key: &ConnectionKey) -> Option<Arc<SyncMutex<Connection>>> {
		let (resumed, expired) = self.connections.with_lock(|connections| {
			let Some(device) = connections.get_mut(&device_key(key)) else {
				return (None, None);
			};

			match device.get_mut(&key.conn_id) {
				| Some(entry) if entry.last_used.elapsed() <= CONNECTION_TTL => {
					entry.last_used = Instant::now();
					(Some(Arc::clone(&entry.connection)), None)
				},
				| Some(_) => (None, device.remove(&key.conn_id)),
				| None => (None, None),
			}
		});

		drop(expired);
		resumed
	}

	/// Removes the connection under `key` if it is still `connection`.
	fn remove(
		&self,
		key: &ConnectionKey,
		connection: &Arc<SyncMutex<Connection>>,
	) -> Option<Entry> {
		self.connections.with_lock(|connections| {
			let device = connections.get_mut(&device_key(key))?;
			if device
				.get(&key.conn_id)
				.is_none_or(|entry| !Arc::ptr_eq(&entry.connection, connection))
			{
				return None;
			}

			let removed = device.remove(&key.conn_id);
			if device.is_empty() {
				connections.remove(&device_key(key));
			}

			removed
		})
	}

	/// Drops idle connections at most once per [`SWEEP_INTERVAL`].
	fn sweep_expired(&self) -> Vec<Entry> {
		let due = self.last_sweep.with_lock(|last_sweep| {
			let due = last_sweep.elapsed() >= SWEEP_INTERVAL;
			if due {
				*last_sweep = Instant::now();
			}

			due
		});

		if !due {
			return Vec::new();
		}

		self.connections.with_lock(|connections| {
			let mut expired = Vec::new();
			connections.retain(|_, device| {
				expired.extend(
					device
						.extract_if(|_, entry| entry.last_used.elapsed() > CONNECTION_TTL)
						.map(|(_, entry)| entry),
				);

				!device.is_empty()
			});

			expired
		})
	}
}

struct Entry {
	connection: Arc<SyncMutex<Connection>>,
	last_used: Instant,
}

fn device_key(key: &ConnectionKey) -> (OwnedUserId, OwnedDeviceId) {
	(key.user_id.clone(), key.device_id.clone())
}

/// Makes room for one more connection on a device.
fn evict_least_recently_used(device: &mut HashMap<Option<String>, Entry>) -> Vec<Entry> {
	let excess = device
		.len()
		.saturating_add(1)
		.saturating_sub(MAX_CONNECTIONS_PER_DEVICE);

	if excess == 0 {
		return Vec::new();
	}

	let mut by_age: Vec<_> = device
		.iter()
		.map(|(conn_id, entry)| (entry.last_used, conn_id.clone()))
		.collect();

	by_age.sort_unstable_by_key(|(last_used, _)| *last_used);
	by_age
		.into_iter()
		.take(excess)
		.filter_map(|(_, conn_id)| device.remove(&conn_id))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	const SHORT: Duration = Duration::from_millis(50);

	fn alice() -> OwnedUserId { UserId::parse("@alice:example.com").unwrap() }

	#[tokio::test]
	async fn a_wake_after_subscribing_is_seen() {
		let wakers = Wakers::default();
		let user = alice();
		let mut subscription = wakers.subscribe(&user, &WakeKind::ALL).await;

		wakers.wake(&user, &[WakeKind::Typing]).await;

		assert_eq!(subscription.woken().await, Some(vec![WakeKind::Typing]));
	}

	#[tokio::test]
	async fn a_wake_while_the_response_is_built_is_not_lost() {
		let wakers = Wakers::default();
		let user = alice();
		let mut subscription = wakers.subscribe(&user, &[WakeKind::ToDevice]).await;

		wakers.wake(&user, &[WakeKind::ToDevice]).await;
		wakers.wake(&user, &[WakeKind::ToDevice]).await;

		assert_eq!(subscription.woken().await, Some(vec![WakeKind::ToDevice]));
		tokio::time::timeout(SHORT, subscription.woken())
			.await
			.unwrap_err();
	}

	#[tokio::test]
	async fn kinds_outside_the_interest_do_not_wake() {
		let wakers = Wakers::default();
		let user = alice();
		let mut subscription = wakers
			.subscribe(&user, &[WakeKind::ToDevice, WakeKind::Keys])
			.await;

		wakers
			.wake(&user, &[WakeKind::Typing, WakeKind::Timeline])
			.await;
		tokio::time::timeout(SHORT, subscription.woken())
			.await
			.unwrap_err();

		wakers.wake(&user, &[WakeKind::Keys]).await;
		assert_eq!(subscription.woken().await, Some(vec![WakeKind::Keys]));
	}

	#[tokio::test]
	async fn a_wake_reports_every_kind_it_bumped() {
		let wakers = Wakers::default();
		let user = alice();
		let mut subscription = wakers.subscribe(&user, &WakeKind::ALL).await;

		wakers
			.wake(&user, &[WakeKind::Timeline, WakeKind::DeviceLists])
			.await;

		assert_eq!(
			subscription.woken().await,
			Some(vec![WakeKind::Timeline, WakeKind::DeviceLists])
		);
	}

	#[tokio::test]
	async fn wake_notifies_all_subscribers() {
		let wakers = Wakers::default();
		let user = alice();
		let mut first = wakers.subscribe(&user, &WakeKind::ALL).await;
		let mut second = wakers.subscribe(&user, &WakeKind::ALL).await;

		wakers.wake(&user, &[WakeKind::Receipts]).await;

		assert!(first.woken().await.is_some());
		assert!(second.woken().await.is_some());
	}

	#[tokio::test]
	async fn waker_is_not_retained_without_subscribers() {
		let wakers = Wakers::default();
		let user = alice();

		wakers.wake(&user, &[WakeKind::Timeline]).await;
		assert!(wakers.0.lock().await.is_empty());

		drop(wakers.subscribe(&user, &WakeKind::ALL).await);
		assert_eq!(wakers.0.lock().await.len(), 1);

		wakers.wake(&user, &[WakeKind::Timeline]).await;
		assert!(wakers.0.lock().await.is_empty());
	}

	#[tokio::test]
	async fn subscriber_still_woken_after_a_sibling_disconnects() {
		let wakers = Wakers::default();
		let user = alice();
		let mut subscription = wakers.subscribe(&user, &WakeKind::ALL).await;
		drop(wakers.subscribe(&user, &WakeKind::ALL).await);

		wakers.wake(&user, &[WakeKind::Timeline]).await;

		assert!(subscription.woken().await.is_some());
	}

	#[test]
	fn every_kind_has_its_own_generation() {
		for (index, kind) in WakeKind::ALL.into_iter().enumerate() {
			assert_eq!(kind.index(), index);
		}
	}

	#[test]
	fn membership_and_encryption_events_also_change_device_lists() {
		assert_eq!(WakeKind::of_event(&TimelineEventType::RoomMember), [
			WakeKind::Timeline,
			WakeKind::DeviceLists
		]);
		assert_eq!(WakeKind::of_event(&TimelineEventType::RoomEncryption), [
			WakeKind::Timeline,
			WakeKind::DeviceLists
		]);
		assert_eq!(WakeKind::of_event(&TimelineEventType::RoomMessage), [WakeKind::Timeline]);
	}

	fn entry(age: Duration) -> Entry {
		Entry {
			connection: Arc::new(SyncMutex::new(Connection::new(0))),
			last_used: Instant::now().checked_sub(age).unwrap(),
		}
	}

	#[test]
	fn a_full_device_evicts_its_least_recently_used_connection() {
		let mut device: DeviceConnections = (0..MAX_CONNECTIONS_PER_DEVICE)
			.map(|i| {
				(Some(i.to_string()), entry(Duration::from_secs(u64::try_from(i).unwrap() + 1)))
			})
			.collect();

		let evicted = evict_least_recently_used(&mut device);

		assert_eq!(evicted.len(), 1);
		assert_eq!(device.len(), MAX_CONNECTIONS_PER_DEVICE - 1);
		assert!(!device.contains_key(&Some((MAX_CONNECTIONS_PER_DEVICE - 1).to_string())));
	}

	#[test]
	fn a_device_with_room_evicts_nothing() {
		let mut device: DeviceConnections = HashMap::from([(None, entry(Duration::ZERO))]);

		assert!(evict_least_recently_used(&mut device).is_empty());
		assert_eq!(device.len(), 1);
	}
}
