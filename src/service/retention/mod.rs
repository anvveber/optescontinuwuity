//! # Room retention (MSC1763 `m.room.retention`)
//!
//! With `allow_room_retention = true`, a room whose state has
//! `m.room.retention` with `max_lifetime` (ms) forgets its messages:
//!
//! - an event older than `max_lifetime` is hidden from clients at once;
//! - a background job (every `retention_purge_interval_secs`) deletes it from
//!   the database: the event itself, its search tokens, relation and thread
//!   entries, reference marks and short-ID mappings.
//!
//! After a purge that removed something, the deleted entries are also erased
//! from the database files: the memory buffers are written out (which retires
//! the write-ahead log that holds every write) and every table is compacted,
//! which drops deleted entries from older files. Client transaction IDs that
//! point to purged events are deleted first. With retention on, RocksDB keeps
//! no archive of retired write-ahead logs and rewrites its file index
//! (MANIFEST) after every change, since it records the first and last key of every file
//! (see `db_opts.rs`). Purged event IDs stay where kept events name them
//! (`prev_events`): the room's history needs those links.
//! What this cannot reach: disk blocks of deleted files (the filesystem frees
//! them but does not overwrite them), backups and disk snapshots.
//!
//! Never removed: state events (names, membership, power levels, the policy
//! itself), the room's forward extremities and its newest event, which the
//! room needs to continue its history.
//!
//! Rooms without the state event keep everything. Media is not affected
//! (encrypted media can't be tied to a room by the server).
//!
//! See `optes/retention-design.md`.

use std::{collections::HashSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use conduwuit::{
	Event, Result, Server, debug, info,
	matrix::{PduCount, PduEvent, pdu::{PduId, RawPduId}},
	utils::{self, stream::TryIgnore},
	warn,
};
use database::{Database, Map, compact};
use futures::{StreamExt, future::ready};
use ruma::{EventId, OwnedEventId, RoomId, events::StateEventType};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use tokio::sync::Notify;

use crate::{Dep, rooms};

pub struct Service {
	interrupt: Notify,
	db: Maps,
	database: Arc<Database>,
	services: Services,
}

struct Maps {
	pduid_pdu: Arc<Map>,
	eventid_pduid: Arc<Map>,
	eventid_outlierpdu: Arc<Map>,
	eventid_shorteventid: Arc<Map>,
	shorteventid_eventid: Arc<Map>,
	shorteventid_shortstatehash: Arc<Map>,
	shorteventid_authchain: Arc<Map>,
	tofrom_relation: Arc<Map>,
	referencedevents: Arc<Map>,
	threadid_userids: Arc<Map>,
	userdevicetxnid_response: Arc<Map>,
}

struct Services {
	server: Arc<Server>,
	metadata: Dep<rooms::metadata::Service>,
	search: Dep<rooms::search::Service>,
	short: Dep<rooms::short::Service>,
	state: Dep<rooms::state::Service>,
	state_accessor: Dep<rooms::state_accessor::Service>,
	timeline: Dep<rooms::timeline::Service>,
}

/// Fields of an event's content that matter for cleanup.
#[derive(Deserialize, Default)]
struct Content {
	body: Option<String>,
	#[serde(rename = "m.relates_to")]
	relates_to: Option<RelatesTo>,
}

#[derive(Deserialize)]
struct RelatesTo {
	event_id: Option<OwnedEventId>,
	#[serde(rename = "m.in_reply_to")]
	in_reply_to: Option<InReplyTo>,
}

#[derive(Deserialize)]
struct InReplyTo {
	event_id: OwnedEventId,
}

const RETENTION_EVENT: &str = "m.room.retention";

#[async_trait]
impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		let db = &args.db;
		Ok(Arc::new(Self {
			interrupt: Notify::new(),
			db: Maps {
				pduid_pdu: db["pduid_pdu"].clone(),
				eventid_pduid: db["eventid_pduid"].clone(),
				eventid_outlierpdu: db["eventid_outlierpdu"].clone(),
				eventid_shorteventid: db["eventid_shorteventid"].clone(),
				shorteventid_eventid: db["shorteventid_eventid"].clone(),
				shorteventid_shortstatehash: db["shorteventid_shortstatehash"].clone(),
				shorteventid_authchain: db["shorteventid_authchain"].clone(),
				tofrom_relation: db["tofrom_relation"].clone(),
				referencedevents: db["referencedevents"].clone(),
				threadid_userids: db["threadid_userids"].clone(),
				userdevicetxnid_response: db["userdevicetxnid_response"].clone(),
			},
			database: Arc::clone(args.db),
			services: Services {
				server: args.server.clone(),
				metadata: args.depend::<rooms::metadata::Service>("rooms::metadata"),
				search: args.depend::<rooms::search::Service>("rooms::search"),
				short: args.depend::<rooms::short::Service>("rooms::short"),
				state: args.depend::<rooms::state::Service>("rooms::state"),
				state_accessor: args
					.depend::<rooms::state_accessor::Service>("rooms::state_accessor"),
				timeline: args.depend::<rooms::timeline::Service>("rooms::timeline"),
			},
		}))
	}

	#[tracing::instrument(skip_all, name = "retention", level = "debug")]
	async fn worker(self: Arc<Self>) -> Result<()> {
		if !self.enabled() {
			return Ok(());
		}

		let every =
			Duration::from_secs(self.services.server.config.retention_purge_interval_secs.max(1));
		loop {
			tokio::select! {
				() = self.interrupt.notified() => break,
				() = tokio::time::sleep(every) => {
					let purged = self.purge_all().await;
					if purged > 0 {
						info!(purged, "retention: purged expired events");
					}
				},
			}
		}

		Ok(())
	}

	fn interrupt(&self) { self.interrupt.notify_waiters(); }

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	#[inline]
	#[must_use]
	pub fn enabled(&self) -> bool { self.services.server.config.allow_room_retention }

	/// Unix ms before which non-state events in this room are expired; `None`
	/// if the room keeps everything.
	pub async fn cutoff(&self, room_id: &RoomId) -> Option<u64> {
		if !self.enabled() {
			return None;
		}

		let content: JsonValue = self
			.services
			.state_accessor
			.room_state_get_content(room_id, &StateEventType::from(RETENTION_EVENT), "")
			.await
			.ok()?;
		let max_lifetime = content.get("max_lifetime")?.as_u64()?;

		Some(utils::millis_since_unix_epoch().saturating_sub(max_lifetime))
	}

	/// Whether `pdu` is expired under `cutoff` (from [`Self::cutoff`]).
	#[must_use]
	pub fn is_expired<E: Event>(pdu: &E, cutoff: Option<u64>) -> bool {
		cutoff.is_some_and(|cutoff| {
			pdu.state_key().is_none() && u64::from(pdu.origin_server_ts().get()) < cutoff
		})
	}

	/// Whether the event with this ID is expired. Unknown events: false.
	pub async fn event_expired(&self, room_id: &RoomId, event_id: &EventId) -> bool {
		let Some(cutoff) = self.cutoff(room_id).await else {
			return false;
		};
		self.services
			.timeline
			.get_pdu(event_id)
			.await
			.is_ok_and(|pdu| Self::is_expired(&pdu, Some(cutoff)))
	}

	/// Purges expired events in every room with a retention policy, then
	/// erases them from the database files.
	pub async fn purge_all(&self) -> usize {
		let rooms: Vec<_> = self.services.metadata.iter_ids().collect().await;
		let mut purged = HashSet::new();
		for room_id in rooms {
			purged.extend(self.purge_room(&room_id).await);
		}
		if !purged.is_empty() {
			self.purge_txnids(&purged).await;
			let started = std::time::Instant::now();
			match self.erase_from_files().await {
				| Ok(()) => info!(
					elapsed_ms = started.elapsed().as_millis(),
					"retention: erased purged events from the database files"
				),
				| Err(e) => warn!("retention: erasing purged events from the database files failed: {e}"),
			}
		}
		purged.len()
	}

	/// Writes every memory buffer out and compacts every table, so deleted
	/// entries are gone from the files, not just marked. All tables, not only
	/// those of the purge: older deletions elsewhere (forward extremities,
	/// transaction IDs) can name purged events too.
	async fn erase_from_files(&self) -> Result {
		let database = self.database.clone();
		tokio::task::spawn_blocking(move || -> Result {
			let maps: Vec<Arc<Map>> = database.iter().map(|(_, map)| map.clone()).collect();
			// Memtables to files, so the write-ahead logs (which hold every
			// write, the purged events too) are retired.
			for map in &maps {
				map.flush_blocking()?;
			}
			let force = compact::Options { exhaustive: true, ..Default::default() };
			for map in &maps {
				map.compact_blocking(force.clone())?;
			}
			Ok(())
		})
		.await
		.map_err(|e| conduwuit::err!("retention: erase task failed: {e}"))?
	}

	/// Removes the client transaction IDs that point to purged events: they
	/// record which user and device sent an event, and Element's IDs carry
	/// the time of sending.
	async fn purge_txnids(&self, purged: &HashSet<OwnedEventId>) {
		let keys: Vec<Vec<u8>> = self
			.db
			.userdevicetxnid_response
			.raw_stream()
			.ignore_err()
			.filter(|(_, val)| {
				ready(std::str::from_utf8(val).is_ok_and(|v| purged.iter().any(|e| e.as_str() == v)))
			})
			.map(|(key, _)| key.to_vec())
			.collect()
			.await;
		for key in keys {
			self.db.userdevicetxnid_response.remove(&key);
		}
	}

	/// Purges expired events in one room. Returns the IDs of those removed.
	pub async fn purge_room(&self, room_id: &RoomId) -> Vec<OwnedEventId> {
		let Some(cutoff) = self.cutoff(room_id).await else {
			return Vec::new();
		};
		let Ok(shortroomid) = self.services.short.get_shortroomid(room_id).await else {
			return Vec::new();
		};

		let _lock = self.services.state.mutex.lock(room_id).await;

		let mut keep: HashSet<OwnedEventId> = self
			.services
			.state
			.get_forward_extremities(room_id)
			.collect()
			.await;
		if let Ok(latest) = self.services.timeline.latest_pdu_in_room(room_id).await {
			keep.insert(latest.event_id().to_owned());
		}

		let expired: Vec<(PduCount, PduEvent)> = self
			.services
			.timeline
			.pdus(room_id, None)
			.ignore_err()
			.take_while(|(_, pdu)| ready(u64::from(pdu.origin_server_ts().get()) < cutoff))
			.filter(|(_, pdu)| {
				ready(pdu.state_key().is_none() && !keep.contains(pdu.event_id()))
			})
			.collect()
			.await;

		let mut removed = Vec::with_capacity(expired.len());
		for (pdu_count, pdu) in expired {
			if let PduCount::Normal(shorteventid) = pdu_count {
				self.purge_event(room_id, shortroomid, shorteventid, &pdu).await;
				removed.push(pdu.event_id().to_owned());
			}
		}
		if !removed.is_empty() {
			debug!(%room_id, count = removed.len(), "retention purge");
		}
		removed
	}

	async fn purge_event(
		&self,
		room_id: &RoomId,
		shortroomid: u64,
		count: u64,
		pdu: &PduEvent,
	) {
		let event_id = pdu.event_id();
		let pdu_id: RawPduId = PduId { shortroomid, shorteventid: PduCount::Normal(count) }.into();
		let content: Content = pdu.get_content().unwrap_or_default();

		// Search index (only unencrypted bodies are indexed).
		if let Some(body) = &content.body {
			self.services
				.search
				.deindex_pdu(shortroomid, &pdu_id, body);
		}

		// Relations pointing at this event (key: to ++ from).
		let prefix = count.to_be_bytes();
		let keys: Vec<Vec<u8>> = self
			.db
			.tofrom_relation
			.raw_keys_prefix(&prefix)
			.ignore_err()
			.map(<[u8]>::to_vec)
			.collect()
			.await;
		for key in keys {
			self.db.tofrom_relation.remove(&key);
		}
		// This event's own relation to its target.
		if let Some(relates_to) = &content.relates_to {
			let targets = relates_to
				.event_id
				.iter()
				.chain(relates_to.in_reply_to.as_ref().map(|r| &r.event_id));
			for target in targets {
				if let Ok(PduCount::Normal(to)) = self.services.timeline.get_pdu_count(target).await {
					let mut key = to.to_be_bytes().to_vec();
					key.extend_from_slice(&count.to_be_bytes());
					self.db.tofrom_relation.remove(&key);
				}
			}
		}

		// Thread root entry, reference mark.
		self.db.threadid_userids.remove(&pdu_id);
		self.db.referencedevents.del((room_id, event_id));

		// Short-ID mappings and per-event state/auth-chain caches.
		if let Ok(short) = self.services.short.get_shorteventid(event_id).await {
			self.db.shorteventid_shortstatehash.del(short);
			self.db.shorteventid_authchain.del(short);
			self.db.shorteventid_eventid.del(short);
		}
		self.db.eventid_shorteventid.remove(event_id.as_bytes());

		// The event itself.
		self.db.pduid_pdu.remove(&pdu_id);
		self.db.eventid_pduid.remove(event_id.as_bytes());
		self.db.eventid_outlierpdu.remove(event_id.as_bytes());

		if self.services.timeline.get_pdu(event_id).await.is_ok() {
			warn!(%event_id, "retention: event still readable after purge");
		}
	}
}
