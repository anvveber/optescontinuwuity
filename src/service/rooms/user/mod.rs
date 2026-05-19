use std::{collections::BTreeMap, sync::Arc};

use conduwuit::{
	Result, implement,
	matrix::pdu::{PduCount, RawPduId},
	utils::stream::TryIgnore,
};
use database::{Database, Deserialized, Json, Map};
use futures::{Stream, StreamExt, pin_mut};
use ruma::{
	OwnedEventId, OwnedRoomId, OwnedUserId, RoomId, UserId,
	api::client::sync::sync_events::UnreadNotificationsCount,
	assign,
	events::receipt::ReceiptThread,
	push::{Action, HighlightTweakValue, Tweak},
};

use crate::{Dep, rooms, rooms::short::ShortStateHash};

pub struct Service {
	db: Data,
	services: Services,
}

struct Data {
	db: Arc<Database>,
	userroomid_notificationcount: Arc<Map>,
	userroomid_highlightcount: Arc<Map>,
	roomuserid_lastnotificationread: Arc<Map>,
	roomuserid_mainnotificationread: Arc<Map>,
	roomuserideventid_threadnotificationread: Arc<Map>,
	roomsynctoken_shortstatehash: Arc<Map>,
	usernotificationid_notification: Arc<Map>,
}

struct Services {
	short: Dep<rooms::short::Service>,
}

impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			db: Data {
				db: args.db.clone(),
				userroomid_notificationcount: args.db["userroomid_notificationcount"].clone(),
				userroomid_highlightcount: args.db["userroomid_highlightcount"].clone(),
				roomuserid_lastnotificationread: args.db["roomuserid_lastnotificationread"]
					.clone(),
				roomuserid_mainnotificationread: args.db["roomuserid_mainnotificationread"]
					.clone(),
				roomuserideventid_threadnotificationread: args.db
					["roomuserideventid_threadnotificationread"]
					.clone(),
				roomsynctoken_shortstatehash: args.db["roomsynctoken_shortstatehash"].clone(),
				usernotificationid_notification: args.db["usernotificationid_notification"]
					.clone(),
			},

			services: Services {
				short: args.depend::<rooms::short::Service>("rooms::short"),
			},
		}))
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NotificationRecord {
	pub actions: Vec<Action>,
	pub pdu_id: Vec<u8>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub room_id: Option<OwnedRoomId>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pdu_count: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thread_root: Option<OwnedEventId>,
}

#[implement(Service)]
pub async fn set_notification_read(
	&self,
	user_id: &UserId,
	room_id: &RoomId,
	thread: ReceiptThread,
	count: u64,
) {
	self.update_notification_read_marker(user_id, room_id, &thread, count)
		.await;
	self.recompute_notification_counts(user_id, room_id).await;
}

impl Service {
	async fn update_notification_read_marker(
		&self,
		user_id: &UserId,
		room_id: &RoomId,
		thread: &ReceiptThread,
		count: u64,
	) {
		match thread {
			| ReceiptThread::Unthreaded => {
				let key = (room_id, user_id);
				let current = self
					.db
					.roomuserid_lastnotificationread
					.qry(&key)
					.await
					.deserialized()
					.unwrap_or(0);

				if count > current {
					self.db.roomuserid_lastnotificationread.put(key, count);
				}
			},
			| ReceiptThread::Main => {
				let key = (room_id, user_id);
				let current = self
					.db
					.roomuserid_mainnotificationread
					.qry(&key)
					.await
					.deserialized()
					.unwrap_or(0);

				if count > current {
					self.db.roomuserid_mainnotificationread.put(key, count);
				}
			},
			| ReceiptThread::Thread(root_event_id) => {
				let key = (room_id, user_id, root_event_id);
				let current = self
					.db
					.roomuserideventid_threadnotificationread
					.qry(&key)
					.await
					.deserialized()
					.unwrap_or(0);

				if count > current {
					self.db
						.roomuserideventid_threadnotificationread
						.put(key, count);
				}
			},
			| ReceiptThread::_Custom(_) | _ => {},
		}
	}

	async fn recompute_notification_counts(&self, user_id: &UserId, room_id: &RoomId) {
		let counts = self.notification_counts_scan(user_id, room_id).await;
		let userroom_id = (user_id, room_id);
		self.db
			.userroomid_highlightcount
			.put(userroom_id, counts.total_highlight_count);
		self.db
			.userroomid_notificationcount
			.put(userroom_id, counts.total_notification_count);
	}

	fn is_highlight(actions: &[Action]) -> bool {
		actions.iter().any(|action| {
			matches!(action, Action::SetTweak(Tweak::Highlight(HighlightTweakValue::Yes)))
		})
	}

	fn record_count(record: &NotificationRecord) -> Option<u64> {
		if let Some(count) = record.pdu_count {
			return Some(count);
		}

		match RawPduId::from(record.pdu_id.as_slice()).pdu_count() {
			| PduCount::Normal(count) => Some(count),
			| PduCount::Backfilled(_) => None,
		}
	}

	async fn thread_notification_reads(
		&self,
		user_id: &UserId,
		room_id: &RoomId,
	) -> BTreeMap<OwnedEventId, u64> {
		type Key = (OwnedRoomId, OwnedUserId, OwnedEventId);

		let prefix = (room_id, user_id);
		self.db
			.roomuserideventid_threadnotificationread
			.stream_prefix(&prefix)
			.ignore_err()
			.map(|((_, _, event_id), count): (Key, u64)| (event_id, count))
			.collect()
			.await
	}

	async fn notification_counts_scan(
		&self,
		user_id: &UserId,
		room_id: &RoomId,
	) -> NotificationCountsScan {
		let unthreaded_read = self.last_notification_read(user_id, room_id).await;
		let main_read = self.last_main_notification_read(user_id, room_id).await;
		let thread_reads = self.thread_notification_reads(user_id, room_id).await;
		let shortroomid = self.services.short.get_shortroomid(room_id).await.ok();

		let items = self.notifications_rev(user_id, None);
		pin_mut!(items);

		let mut counts = NotificationCountsScan::default();
		while let Some((_, record)) = items.next().await {
			if !record.matches_room(room_id, shortroomid) {
				continue;
			}

			let Some(count) = Self::record_count(&record) else {
				continue;
			};

			let read_up_to = match record.thread_root.as_ref() {
				| Some(root_event_id) =>
					unthreaded_read.max(*thread_reads.get(root_event_id).unwrap_or(&0)),
				| None => unthreaded_read.max(main_read),
			};

			if count <= read_up_to {
				continue;
			}

			let is_highlight = Self::is_highlight(&record.actions);
			counts.total_notification_count = counts.total_notification_count.saturating_add(1);
			if is_highlight {
				counts.total_highlight_count = counts.total_highlight_count.saturating_add(1);
			}

			match record.thread_root.clone() {
				| Some(root_event_id) => {
					let entry = counts.thread_counts.entry(root_event_id).or_default();
					entry.notification_count = entry.notification_count.saturating_add(1);
					if is_highlight {
						entry.highlight_count = entry.highlight_count.saturating_add(1);
					}
				},
				| None => {
					counts.main_notification_count =
						counts.main_notification_count.saturating_add(1);
					if is_highlight {
						counts.main_highlight_count =
							counts.main_highlight_count.saturating_add(1);
					}
				},
			}
		}

		counts
	}
}

#[implement(Service)]
pub async fn notification_count(&self, user_id: &UserId, room_id: &RoomId) -> u64 {
	let key = (user_id, room_id);
	self.db
		.userroomid_notificationcount
		.qry(&key)
		.await
		.deserialized()
		.unwrap_or(0)
}

#[implement(Service)]
pub async fn highlight_count(&self, user_id: &UserId, room_id: &RoomId) -> u64 {
	let key = (user_id, room_id);
	self.db
		.userroomid_highlightcount
		.qry(&key)
		.await
		.deserialized()
		.unwrap_or(0)
}

#[implement(Service)]
pub async fn last_notification_read(&self, user_id: &UserId, room_id: &RoomId) -> u64 {
	let key = (room_id, user_id);
	self.db
		.roomuserid_lastnotificationread
		.qry(&key)
		.await
		.deserialized()
		.unwrap_or(0)
}

#[implement(Service)]
pub async fn last_main_notification_read(&self, user_id: &UserId, room_id: &RoomId) -> u64 {
	let key = (room_id, user_id);
	self.db
		.roomuserid_mainnotificationread
		.qry(&key)
		.await
		.deserialized()
		.unwrap_or(0)
}

#[implement(Service)]
pub async fn notification_read_count(
	&self,
	user_id: &UserId,
	room_id: &RoomId,
	thread_root: Option<&ruma::EventId>,
) -> u64 {
	let unthreaded_read = self.last_notification_read(user_id, room_id).await;

	match thread_root {
		| Some(thread_root) => {
			let key = (room_id, user_id, thread_root);
			let thread_read = self
				.db
				.roomuserideventid_threadnotificationread
				.qry(&key)
				.await
				.deserialized()
				.unwrap_or(0);

			unthreaded_read.max(thread_read)
		},
		| None => unthreaded_read.max(self.last_main_notification_read(user_id, room_id).await),
	}
}

#[implement(Service)]
pub async fn notification_counts_with_threads(
	&self,
	user_id: &UserId,
	room_id: &RoomId,
) -> (UnreadNotificationsCount, BTreeMap<OwnedEventId, UnreadNotificationsCount>) {
	let scan = self.notification_counts_scan(user_id, room_id).await;

	let unread_notifications = assign!(UnreadNotificationsCount::new(), {
		notification_count: Some(
			scan.main_notification_count
				.try_into()
				.expect("notification count can't go that high"),
		),
		highlight_count: Some(
			scan.main_highlight_count
				.try_into()
				.expect("highlight count can't go that high"),
		),
	});

	let unread_thread_notifications = scan
		.thread_counts
		.into_iter()
		.map(|(event_id, counts)| {
			let unread = assign!(UnreadNotificationsCount::new(), {
				notification_count: Some(
					counts
						.notification_count
						.try_into()
						.expect("notification count can't go that high"),
				),
				highlight_count: Some(
					counts
						.highlight_count
						.try_into()
						.expect("highlight count can't go that high"),
				),
			});

			(event_id, unread)
		})
		.collect();

	(unread_notifications, unread_thread_notifications)
}

#[implement(Service)]
pub async fn associate_token_shortstatehash(
	&self,
	room_id: &RoomId,
	token: u64,
	shortstatehash: ShortStateHash,
) {
	let shortroomid = self
		.services
		.short
		.get_shortroomid(room_id)
		.await
		.expect("room exists");

	let _cork = self.db.db.cork();
	let key: &[u64] = &[shortroomid, token];
	self.db
		.roomsynctoken_shortstatehash
		.put(key, shortstatehash);
}

#[implement(Service)]
pub async fn get_token_shortstatehash(
	&self,
	room_id: &RoomId,
	token: u64,
) -> Result<ShortStateHash> {
	let shortroomid = self.services.short.get_shortroomid(room_id).await?;

	let key: &[u64] = &[shortroomid, token];
	self.db
		.roomsynctoken_shortstatehash
		.qry(key)
		.await
		.deserialized()
}

#[implement(Service)]
pub fn notification_insert(
	&self,
	user_id: &UserId,
	count: u64,
	notification: &NotificationRecord,
) {
	self.db
		.usernotificationid_notification
		.put((user_id, count), Json(notification));
}

#[implement(Service)]
pub fn notifications_rev<'a>(
	&'a self,
	user_id: &'a UserId,
	from: Option<u64>,
) -> impl Stream<Item = (u64, NotificationRecord)> + Send + 'a {
	type Key = (OwnedUserId, u64);

	let from = from.unwrap_or(u64::MAX);
	let last_possible = (user_id, from);

	self.db
		.usernotificationid_notification
		.rev_stream_from(&last_possible)
		.ignore_err()
		.take_while(move |((stored_user, _), _): &(Key, NotificationRecord)| {
			std::future::ready(stored_user == user_id)
		})
		.map(|((_, count), notification): (Key, NotificationRecord)| (count, notification))
}

#[derive(Default)]
struct NotificationCountsScan {
	total_notification_count: u64,
	total_highlight_count: u64,
	main_notification_count: u64,
	main_highlight_count: u64,
	thread_counts: BTreeMap<OwnedEventId, ThreadNotificationCounts>,
}

#[derive(Default)]
struct ThreadNotificationCounts {
	notification_count: u64,
	highlight_count: u64,
}

impl NotificationRecord {
	fn matches_room(&self, room_id: &RoomId, shortroomid: Option<u64>) -> bool {
		if self
			.room_id
			.as_deref()
			.is_some_and(|stored_room_id| stored_room_id == room_id)
		{
			return true;
		}

		shortroomid.is_some_and(|shortroomid| {
			RawPduId::from(self.pdu_id.as_slice()).shortroomid() == shortroomid.to_be_bytes()
		})
	}
}
