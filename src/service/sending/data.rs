use std::{borrow::Cow, fmt::Debug, sync::Arc};

use conduwuit::{
	Error, Result, at, utils,
	utils::{ReadyExt, stream::TryIgnore},
};
use database::{Batch, Database, Deserialized, Map};
use futures::{Stream, StreamExt};
use ruma::{ServerName, UserId};

use super::{Destination, SendingEvent};
use crate::{Dep, globals};

pub(super) type OutgoingItem = (Key, SendingEvent, Destination);
pub(super) type SendingItem = (Key, SendingEvent);
pub(super) type QueueItem = (Key, SendingEvent);
pub(super) type Key = Vec<u8>;

pub struct Data {
	servercurrentevent_data: Arc<Map>,
	servernameevent_data: Arc<Map>,
	servername_educount: Arc<Map>,
	pub(super) todeviceid_events: Arc<Map>,
	pub(super) db: Arc<Database>,
	services: Services,
}

struct Services {
	globals: Dep<globals::Service>,
}

impl Data {
	pub(super) fn new(args: &crate::Args<'_>) -> Self {
		let db = &args.db;
		Self {
			servercurrentevent_data: db["servercurrentevent_data"].clone(),
			servernameevent_data: db["servernameevent_data"].clone(),
			servername_educount: db["servername_educount"].clone(),
			todeviceid_events: db["todeviceid_events"].clone(),
			db: args.db.clone(),
			services: Services {
				globals: args.depend::<globals::Service>("globals"),
			},
		}
	}

	pub(super) fn delete_active_request(&self, key: &[u8]) {
		self.servercurrentevent_data.remove(key);
	}

	pub(super) async fn is_queued(&self, key: &[u8]) -> bool {
		self.servernameevent_data.get(key).await.is_ok()
	}

	pub(super) async fn delete_all_active_requests_for(&self, destination: &Destination) {
		let prefix = destination.get_prefix();
		let mut batch = self.servercurrentevent_data.batch();
		self.servercurrentevent_data
			.raw_keys_prefix(&prefix)
			.ignore_err()
			.ready_for_each(|key| batch.remove(&self.servercurrentevent_data, key))
			.await;
		batch.commit();
	}

	pub(super) async fn delete_all_requests_for(&self, destination: &Destination) {
		let prefix = destination.get_prefix();
		self.servercurrentevent_data
			.raw_keys_prefix(&prefix)
			.ignore_err()
			.ready_for_each(|key| self.servercurrentevent_data.remove(key))
			.await;

		self.servernameevent_data
			.raw_keys_prefix(&prefix)
			.ignore_err()
			.ready_for_each(|key| self.servernameevent_data.remove(key))
			.await;
	}

	pub(super) fn mark_as_active<'a, I>(&self, events: I)
	where
		I: Iterator<Item = &'a QueueItem>,
	{
		let mut batch = self.servercurrentevent_data.batch();
		events
			.filter(|(key, _)| !key.is_empty())
			.for_each(|(key, val)| {
				let val = serialize_event(val);

				batch.insert(&self.servercurrentevent_data, key, val);
				batch.remove(&self.servernameevent_data, key);
			});
		batch.commit();
	}

	#[inline]
	pub fn active_requests(&self) -> impl Stream<Item = OutgoingItem> + Send + '_ {
		self.servercurrentevent_data
			.raw_stream()
			.ignore_err()
			.map(|(key, val)| {
				let (dest, event) =
					parse_servercurrentevent(key, val).expect("invalid servercurrentevent");

				(key.to_vec(), event, dest)
			})
	}

	#[inline]
	pub fn active_requests_for(
		&self,
		destination: &Destination,
	) -> impl Stream<Item = SendingItem> + Send + '_ + use<'_> {
		let prefix = destination.get_prefix();
		self.servercurrentevent_data
			.raw_stream_from(&prefix)
			.ignore_err()
			.ready_take_while(move |(key, _)| key.starts_with(&prefix))
			.map(|(key, val)| {
				let (_, event) =
					parse_servercurrentevent(key, val).expect("invalid servercurrentevent");

				(key.to_vec(), event)
			})
	}

	pub(super) fn queue_requests<'a, I>(&self, requests: I) -> Vec<Vec<u8>>
	where
		I: Iterator<Item = (&'a SendingEvent, &'a Destination)> + Clone + Debug + Send,
	{
		let keys: Vec<_> = requests
			.clone()
			.map(|(event, dest)| {
				let mut key = dest.get_prefix();
				if let SendingEvent::Pdu(value) = event {
					key.extend(value.as_ref());
				} else {
					let count = self.services.globals.next_count().unwrap();
					key.extend(&count.to_be_bytes());
				}

				key
			})
			.collect();

		self.servernameevent_data.insert_batch(
			keys.iter()
				.zip(requests.map(at!(0)))
				.map(|(key, event)| (key, serialize_event(event))),
		);

		keys
	}

	pub(super) fn queue_to_device(
		&self,
		batch: &mut Batch,
		dest: &Destination,
		event: &SendingEvent,
		count: u64,
	) -> Key {
		let mut key = dest.get_prefix();
		key.extend_from_slice(&count.to_be_bytes());
		batch.insert(&self.servernameevent_data, &key, serialize_event(event));
		key
	}

	pub fn queued_requests(
		&self,
		destination: &Destination,
	) -> impl Stream<Item = QueueItem> + Send + '_ + use<'_> {
		let prefix = destination.get_prefix();
		self.servernameevent_data
			.raw_stream_from(&prefix)
			.ignore_err()
			.ready_take_while(move |(key, _)| key.starts_with(&prefix))
			.map(|(key, val)| {
				let (_, event) =
					parse_servercurrentevent(key, val).expect("invalid servercurrentevent");

				(key.to_vec(), event)
			})
	}

	pub(super) fn set_latest_educount(&self, server_name: &ServerName, last_count: u64) {
		self.servername_educount.raw_put(server_name, last_count);
	}

	pub(super) fn queued_appservices(&self) -> impl Stream<Item = Destination> + Send + '_ {
		self.servernameevent_data
			.raw_stream_from(b"+")
			.ignore_err()
			.ready_take_while(|(key, _)| key.starts_with(b"+"))
			.map(|(key, val)| {
				parse_servercurrentevent(key, val)
					.expect("invalid queued appservice event")
					.0
			})
	}

	pub async fn get_latest_educount(&self, server_name: &ServerName) -> u64 {
		self.servername_educount
			.get(server_name)
			.await
			.deserialized()
			.unwrap_or(0)
	}
}

fn serialize_event(event: &SendingEvent) -> Cow<'_, [u8]> {
	match event {
		| SendingEvent::Edu(value) => Cow::Borrowed(value),
		| SendingEvent::ToDevice(value) => {
			// NUL distinguishes deliveries from legacy JSON EDUs.
			let mut encoded = Vec::with_capacity(value.len().saturating_add(1));
			encoded.push(0);
			encoded.extend_from_slice(value);
			Cow::Owned(encoded)
		},
		| SendingEvent::Pdu(_) | SendingEvent::Flush => Cow::Borrowed(&[]),
	}
}

fn parse_servercurrentevent(key: &[u8], value: &[u8]) -> Result<(Destination, SendingEvent)> {
	// Appservices start with a plus
	Ok::<_, Error>(if key.starts_with(b"+") {
		let mut parts = key[1..].splitn(2, |&b| b == 0xFF);

		let server = parts.next().expect("splitn always returns one element");
		let event = parts
			.next()
			.ok_or_else(|| Error::bad_database("Invalid bytes in servercurrentpdus."))?;

		let server = utils::string_from_bytes(server).map_err(|_| {
			Error::bad_database("Invalid server bytes in server_currenttransaction")
		})?;

		(
			Destination::Appservice(server),
			if let Some(value) = value.strip_prefix(&[0]) {
				SendingEvent::ToDevice(value.into())
			} else if value.is_empty() {
				SendingEvent::Pdu(event.into())
			} else {
				SendingEvent::Edu(value.into())
			},
		)
	} else if key.starts_with(b"$") {
		let mut parts = key[1..].splitn(3, |&b| b == 0xFF);

		let user = parts.next().expect("splitn always returns one element");
		let user_string = utils::str_from_bytes(user)
			.map_err(|_| Error::bad_database("Invalid user string in servercurrentevent"))?;
		let user_id = UserId::parse(user_string)
			.map_err(|_| Error::bad_database("Invalid user id in servercurrentevent"))?;

		let pushkey = parts
			.next()
			.ok_or_else(|| Error::bad_database("Invalid bytes in servercurrentpdus."))?;
		let pushkey_string = utils::string_from_bytes(pushkey)
			.map_err(|_| Error::bad_database("Invalid pushkey in servercurrentevent"))?;

		let event = parts
			.next()
			.ok_or_else(|| Error::bad_database("Invalid bytes in servercurrentpdus."))?;

		(
			Destination::Push(user_id, pushkey_string),
			if value.is_empty() {
				SendingEvent::Pdu(event.into())
			} else {
				// I'm pretty sure this should never be called
				SendingEvent::Edu(value.into())
			},
		)
	} else {
		let mut parts = key.splitn(2, |&b| b == 0xFF);

		let server = parts.next().expect("splitn always returns one element");
		let event = parts
			.next()
			.ok_or_else(|| Error::bad_database("Invalid bytes in servercurrentpdus."))?;

		let server = utils::string_from_bytes(server).map_err(|_| {
			Error::bad_database("Invalid server bytes in server_currenttransaction")
		})?;

		(
			Destination::Federation(ServerName::parse(&server).map_err(|_| {
				Error::bad_database("Invalid server string in server_currenttransaction")
			})?),
			if value.is_empty() {
				SendingEvent::Pdu(event.into())
			} else {
				SendingEvent::Edu(value.into())
			},
		)
	})
}

#[cfg(test)]
mod tests {
	use super::{Destination, SendingEvent, parse_servercurrentevent, serialize_event};

	#[test]
	fn msc4203_queue_encoding_preserves_legacy_events() {
		let key = b"+bridge\xFF\0\0\0\0\0\0\0\x01";
		let edu = SendingEvent::Edu(br#"{"type":"m.typing","content":{}}"#.as_slice().into());
		let encoded = serialize_event(&edu);
		assert_eq!(encoded.as_ref(), br#"{"type":"m.typing","content":{}}"#);
		assert_eq!(
			parse_servercurrentevent(key, &encoded).unwrap(),
			(Destination::Appservice("bridge".into()), edu)
		);

		let delivery = SendingEvent::ToDevice(br#"{"count":1,"event":{}}"#.as_slice().into());
		let encoded = serialize_event(&delivery);
		assert_eq!(encoded[0], 0);
		assert_eq!(
			parse_servercurrentevent(key, &encoded).unwrap(),
			(Destination::Appservice("bridge".into()), delivery)
		);
	}
}
