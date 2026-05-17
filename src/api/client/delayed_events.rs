use std::time::{Duration, SystemTime};

use axum::extract::State;
use conduwuit::{Err, Result};
use ruma::api::client::delayed_events;
use service::rooms::delayed_events::ScheduledDelayedEvent;

use crate::{Ruma, client_ip::ClientIp};

pub(crate) async fn send_delayed_event_route(
	State(services): State<crate::State>,
	ClientIp(client_ip): ClientIp,
	Ruma {
		body:
			delayed_events::send_delayed_event::unstable::Request {
				txn_id,
				room_id,
				event_type,
				content,
				delay: delay_ms,
				state_key,
				..
			},
		identity,
		..
	}: Ruma<delayed_events::send_delayed_event::unstable::Request>,
) -> Result<delayed_events::send_delayed_event::unstable::Response> {
	let sender_user = identity.expect_sender_user()?;
	let sender_device = identity.sender_device();

	if services.users.is_suspended(sender_user).await? {
		return Err!(Request(UserSuspended("You cannot perform this action while suspended.")));
	}

	services
		.users
		.update_device_last_seen(sender_user, sender_device, client_ip)
		.await;

	if delay_ms > Duration::from_hours(24 * 365 * 10000) {
		// Even if no maximum delay has been configured,
		// don't allow non-sensical values. This saves us from needing to worry
		// about arithmetic overflows later.
		return Err!(Request(Forbidden("Requested delay duration is too long.")));
	}

	let running_since = SystemTime::now();

	let scheduled_event = ScheduledDelayedEvent {
		event_type,
		state_key,
		content,
		user_id: sender_user.to_owned(),
		room_id,
		running_since,
		delay_ms,
	};

	let delay_id = services
		.rooms
		.delayed_events
		.queue_delayed_event(txn_id, sender_device, scheduled_event)
		.await?;

	Ok(delayed_events::send_delayed_event::unstable::Response::new(delay_id))
}

pub(crate) async fn update_delayed_event_event_route(
	State(services): State<crate::State>,
	Ruma {
		body: delayed_events::update_delayed_event::unstable_v2::Request { delay_id, action, .. },
		..
	}: Ruma<delayed_events::update_delayed_event::unstable_v2::Request>,
) -> Result<delayed_events::update_delayed_event::unstable_v2::Response> {
	services
		.rooms
		.delayed_events
		.update_delayed_event(delay_id, action)
		.await?;

	Ok(delayed_events::update_delayed_event::unstable_v2::Response::new())
}

pub(crate) async fn get_delayed_event_route(
	State(services): State<crate::State>,
	Ruma {
		body: delayed_events::get_delayed_event::unstable::Request { delay_id, .. },
		..
	}: Ruma<delayed_events::get_delayed_event::unstable::Request>,
) -> Result<delayed_events::get_delayed_event::unstable::Response> {
	let data = services
		.rooms
		.delayed_events
		.get_delayed_event(delay_id)
		.await?;

	Ok(data.into())
}

pub(crate) async fn get_all_delayed_events_route(
	State(services): State<crate::State>,
	body: Ruma<delayed_events::get_all_delayed_events::unstable::Request>,
) -> Result<delayed_events::get_all_delayed_events::unstable::Response> {
	let sender_user = body.identity.expect_sender_user()?;

	let mut data = services
		.rooms
		.delayed_events
		.get_user_scheduled_delayed_events(sender_user, None)
		.await;

	data.sort_by_key(|event| event.running_since.checked_add(event.delay));

	Ok(delayed_events::get_all_delayed_events::unstable::Response::new(data))
}
