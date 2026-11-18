use std::{collections::BTreeMap, sync::Arc};

use conduwuit::{SyncMutex, result::LogErr, warn};
use conduwuit_core::{
	Event, Result, err,
	matrix::{
		event::RelationTypeEqual,
		pdu::{PduCount, PduEvent, PduId, RawPduId},
	},
	utils::{
		ReadyExt,
		stream::{TryIgnore, WidebandExt},
	},
};
use conduwuit_database::{Deserialized, Map};
use futures::{Stream, StreamExt};
use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, EventId, OwnedEventId, OwnedUserId, RoomId, UInt,
	UserId,
	api::{Direction, client::threads::get_threads::v1::IncludeThreads},
	events::{
		AnySyncMessageLikeEvent, AnySyncTimelineEvent,
		relation::{BundledThread, RelationType},
	},
	serde::Raw,
	uint,
};
use serde_json::json;

use crate::{Dep, release, rooms, rooms::short::ShortRoomId};

pub struct Service {
	db: Data,
	services: Services,
	/// Delayed release: thread replies (root event ID, reply event ID) not
	/// yet counted in their root's summary. Applied when the reply is
	/// released (`release_pending`).
	pending: SyncMutex<Vec<(OwnedEventId, OwnedEventId)>>,
}

struct Services {
	short: Dep<rooms::short::Service>,
	pdu_metadata: Dep<rooms::pdu_metadata::Service>,
	timeline: Dep<rooms::timeline::Service>,
	release: Dep<release::Service>,
}

pub(super) struct Data {
	threadid_userids: Arc<Map>,
}

impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			db: Data {
				threadid_userids: args.db["threadid_userids"].clone(),
			},
			services: Services {
				short: args.depend::<rooms::short::Service>("rooms::short"),
				pdu_metadata: args.depend::<rooms::pdu_metadata::Service>("rooms::pdu_metadata"),
				timeline: args.depend::<rooms::timeline::Service>("rooms::timeline"),
				release: args.depend::<release::Service>("release"),
			},
			pending: SyncMutex::new(Vec::new()),
		}))
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	/// Serializes an event for the `latest_event` of a thread's aggregation,
	/// stripping `unsigned` as we don't want to send e.g. txnid to clients.
	fn latest_event<E>(event: &E) -> Raw<AnySyncMessageLikeEvent>
	where
		E: Event,
	{
		let mut latest_pdu = event.clone().into_pdu();
		latest_pdu.unsigned = None;
		latest_pdu
			.into_format::<Raw<AnySyncTimelineEvent>>()
			.cast_unchecked()
	}

	fn set_thread(unsigned: &mut CanonicalJsonObject, thread: BundledThread) {
		let content = serde_json::to_value(thread).expect("to_value always works");
		unsigned.insert(
			"m.relations".to_owned(),
			json!({ "m.thread": content })
				.try_into()
				.expect("thread is valid json"),
		);
	}

	/// Remove `event` from the aggregation of the thread matching
	/// `root_event_id`
	pub async fn remove_from_thread<E>(&self, root_event_id: &EventId, event: &E) -> Result
	where
		E: Event + Send + Sync,
	{
		self.refresh_summary(root_event_id, event.sender(), &event.room_id_or_hash(), false)
			.await
	}

	/// Rebuilds the thread summary in the root's `unsigned` from the thread's
	/// replies. Delayed release: only released replies count, so the summary
	/// never shows a reply before clients may see the reply itself;
	/// `held_too` counts every reply (at shutdown).
	async fn refresh_summary(
		&self,
		root_event_id: &EventId,
		user_id: &UserId,
		room_id: &RoomId,
		held_too: bool,
	) -> Result {
		let Ok(root_id) = self.services.timeline.get_pdu_id(root_event_id).await else {
			return Ok(());
		};

		let mut root_pdu_json = self
			.services
			.timeline
			.get_pdu_json_from_id(&root_id)
			.await
			.map_err(|e| err!(Database("Thread root pdu not found: {e:?}")))?;

		let thread: Vec<_> = self
			.services
			.pdu_metadata
			.get_relations(
				user_id,
				room_id,
				root_event_id,
				PduCount::max(),
				usize::MAX,
				0,
				Direction::Backward,
			)
			.await
			.into_iter()
			.filter(|(count, pdu)| {
				RelationType::Thread.relation_type_equal(pdu)
					&& (held_too || self.services.release.is_released(*count))
			})
			.collect();

		match thread.first() {
			| Some((_, latest)) => {
				// See add_to_thread: no summary on a backfilled root.
				if !matches!(root_id.pdu_count(), PduCount::Normal(_)) {
					return Ok(());
				}
				let CanonicalJsonValue::Object(unsigned) = root_pdu_json
					.entry("unsigned".to_owned())
					.or_insert_with(|| CanonicalJsonValue::Object(BTreeMap::default()))
				else {
					return Ok(());
				};
				Self::set_thread(
					unsigned,
					BundledThread::new(
						Self::latest_event(latest),
						thread.len().try_into().unwrap_or(UInt::MAX),
						true,
					),
				);
			},
			| None => {
				// No events are left in the thread, or the root is backfilled
				let Some(CanonicalJsonValue::Object(unsigned)) = root_pdu_json.get_mut("unsigned")
				else {
					return Ok(());
				};
				if unsigned.remove("m.relations").is_none() {
					return Ok(());
				}
			},
		}

		self.services
			.timeline
			.replace_pdu(&root_id, &root_pdu_json)
			.await
	}

	/// Delayed release: applies the held thread replies that are released
	/// now (all of them with `all`, at shutdown: after a restart everything
	/// is released). Called by the release service right after each tick.
	pub async fn release_pending(&self, all: bool) {
		let taken = std::mem::take(&mut *self.pending.lock());
		let mut later = Vec::new();
		for (root_event_id, reply_event_id) in taken {
			let Ok(count) = self.services.timeline.get_pdu_count(&reply_event_id).await else {
				continue; // gone (purged)
			};
			if !all && !self.services.release.is_released(count) {
				later.push((root_event_id, reply_event_id));
				continue;
			}
			let Ok(reply) = self.services.timeline.get_pdu(&reply_event_id).await else {
				continue;
			};
			if let Err(e) = self.apply_reply(&root_event_id, &reply, all).await {
				warn!(
					"Failed to add released event {reply_event_id} to thread {root_event_id}: \
					 {e:?}"
				);
			}
		}
		self.pending.lock().extend(later);
	}

	/// The summary and participants for a released reply.
	async fn apply_reply(&self, root_event_id: &EventId, reply: &PduEvent, all: bool) -> Result {
		let root_id = self.services.timeline.get_pdu_id(root_event_id).await?;
		self.refresh_summary(root_event_id, reply.sender(), &reply.room_id_or_hash(), all)
			.await?;
		self.add_participant(&root_id, reply.sender()).await
	}

	async fn add_participant(&self, root_id: &RawPduId, user_id: &UserId) -> Result {
		let mut users = Vec::new();
		match self.get_participants(root_id).await {
			| Ok(userids) => {
				users.extend_from_slice(&userids);
			},
			| _ => {
				let root_pdu = self.services.timeline.get_pdu_from_id(root_id).await?;
				users.push(root_pdu.sender().to_owned());
			},
		}
		users.push(user_id.to_owned());

		self.update_participants(root_id, &users)
	}

	pub async fn add_to_thread<E>(&self, root_event_id: &EventId, event: &E) -> Result
	where
		E: Event + Send + Sync,
	{
		let root_id = self
			.services
			.timeline
			.get_pdu_id(root_event_id)
			.await
			.map_err(|e| {
				err!(Request(InvalidParam("Invalid event_id in thread message: {e:?}")))
			})?;

		// Delayed release: the root's summary and the thread's participants
		// change only when the reply is released (release_pending). Changed
		// now, they would show the reply to everyone who reads the root.
		if self.services.release.enabled() {
			self.pending
				.lock()
				.push((root_event_id.to_owned(), event.event_id().to_owned()));
			return Ok(());
		}

		let root_pdu = self
			.services
			.timeline
			.get_pdu_from_id(&root_id)
			.await
			.map_err(|e| err!(Request(InvalidParam("Thread root not found: {e:?}"))))?;

		let mut root_pdu_json = self
			.services
			.timeline
			.get_pdu_json_from_id(&root_id)
			.await
			.map_err(|e| err!(Request(InvalidParam("Thread root pdu not found: {e:?}"))))?;

		// get_relations doesn't support backfilled targets, so redactions
		// couldn't update the aggregation of a backfilled root
		//
		// TODO: Support backfilled threads. This needs
		// `src/service/rooms/pdu_metadata/mod.rs`'s
		// (TODO: Relations with backfilled pdus) to be implemented,
		// and then this check can be removed.
		if matches!(root_id.pdu_count(), PduCount::Normal(_))
			&& let CanonicalJsonValue::Object(unsigned) = root_pdu_json
				.entry("unsigned".to_owned())
				.or_insert_with(|| CanonicalJsonValue::Object(BTreeMap::default()))
		{
			let latest_event = Self::latest_event(event);
			let thread = match unsigned
				.get("m.relations")
				.and_then(|r| r.as_object())
				.and_then(|r| r.get("m.thread"))
				.and_then(|relations| {
					serde_json::from_value::<BundledThread>(relations.clone().into()).ok()
				}) {
				// Thread already existed
				| Some(mut thread) => {
					thread.count = thread.count.saturating_add(uint!(1));
					thread.latest_event = latest_event;
					thread
				},
				// New thread
				| None => BundledThread::new(latest_event, uint!(1), true),
			};

			Self::set_thread(unsigned, thread);

			self.services
				.timeline
				.replace_pdu(&root_id, &root_pdu_json)
				.await?;
		}

		let mut users = Vec::new();
		match self.get_participants(&root_id).await {
			| Ok(userids) => {
				users.extend_from_slice(&userids);
			},
			| _ => {
				users.push(root_pdu.sender().to_owned());
			},
		}
		users.push(event.sender().to_owned());

		self.update_participants(&root_id, &users)
	}

	pub async fn threads_until<'a>(
		&'a self,
		user_id: &'a UserId,
		room_id: &'a RoomId,
		shorteventid: PduCount,
		_inc: &'a IncludeThreads,
	) -> Result<impl Stream<Item = (PduCount, PduEvent)> + Send + 'a> {
		let shortroomid: ShortRoomId = self.services.short.get_shortroomid(room_id).await?;

		let current: RawPduId = PduId {
			shortroomid,
			shorteventid: shorteventid.saturating_sub(1),
		}
		.into();

		let stream = self
			.db
			.threadid_userids
			.rev_raw_keys_from(&current)
			.ignore_err()
			.map(RawPduId::from)
			.ready_take_while(move |pdu_id| pdu_id.shortroomid() == shortroomid.to_be_bytes())
			.wide_filter_map(move |pdu_id| async move {
				let mut pdu = self.services.timeline.get_pdu_from_id(&pdu_id).await.ok()?;

				let pdu_id: PduId = pdu_id.into();
				let ctx = self
					.services
					.timeline
					.get_unsigned_context(&pdu, Some(user_id))
					.await;
				pdu.set_unsigned(Some(ctx)).log_err().ok();

				Some((pdu_id.shorteventid, pdu))
			});

		Ok(stream)
	}

	pub(super) fn update_participants(
		&self,
		root_id: &RawPduId,
		participants: &[OwnedUserId],
	) -> Result {
		let users = participants
			.iter()
			.map(|user| user.as_bytes())
			.collect::<Vec<_>>()
			.join(&[0xFF][..]);

		self.db.threadid_userids.insert(root_id, &users);

		Ok(())
	}

	pub(super) async fn get_participants(&self, root_id: &RawPduId) -> Result<Vec<OwnedUserId>> {
		self.db.threadid_userids.get(root_id).await.deserialized()
	}
}
