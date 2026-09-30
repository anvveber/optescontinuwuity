use conduwuit::error;
use database::Json;
use ruma::{OwnedDeviceId, OwnedUserId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Destination, EduBuf, Msg, SendingEvent, Service};

#[derive(Deserialize, Serialize)]
pub(super) struct ToDeviceDelivery {
	pub count: u64,
	pub delete_after_delivery: bool,
	pub event: ToDeviceMessage,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct ToDeviceMessage {
	pub to_user_id: OwnedUserId,
	pub to_device_id: OwnedDeviceId,
	#[serde(rename = "type")]
	pub event_type: String,
	pub sender: OwnedUserId,
	pub content: Value,
}

impl Service {
	pub(crate) async fn send_to_device(&self, event: ToDeviceMessage) {
		let registrations = self.services.appservice.read().await;
		let count = self.services.globals.next_count().unwrap();
		let mut batch = self.db.todeviceid_events.batch();
		batch.put(
			&self.db.todeviceid_events,
			(&event.to_user_id, &event.to_device_id, count),
			Json(
				json!({"type": event.event_type, "sender": event.sender, "content": event.content}),
			),
		);
		let mut delivery = ToDeviceDelivery {
			count,
			delete_after_delivery: false,
			event,
		};

		let mut requests = Vec::new();
		for info in registrations.values().filter(|info| {
			self.services
				.globals
				.user_is_local(&delivery.event.to_user_id)
				&& info.is_user_match(&delivery.event.to_user_id)
				&& info
					.registration
					.url
					.as_deref()
					.is_some_and(|url| !url.is_empty() && url != "null")
		}) {
			let dest = Destination::Appservice(info.registration.id.clone());
			delivery.delete_after_delivery =
				info.is_exclusive_user_match(&delivery.event.to_user_id);
			let mut buf = EduBuf::new();
			serde_json::to_writer(&mut buf, &delivery)
				.expect("appservice to-device delivery can be serialized");
			let event = SendingEvent::ToDevice(buf);
			let queue_id = self.db.queue_to_device(&mut batch, &dest, &event, count);
			requests.push(Msg { dest, event, queue_id });
		}
		batch.commit();
		for request in requests {
			if let Err(error) = self.dispatch(request) {
				error!(%error, "Failed to dispatch queued appservice to-device delivery");
			}
		}
	}
}
