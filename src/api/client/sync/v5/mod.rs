mod extensions;
mod lists;
mod room;

use std::{cmp, time::Duration};

use axum::extract::State;
use conduwuit::{Err, Result, debug, info, trace};
use conduwuit_service::{
	Services,
	sync::{ConnectionKey, ConnectionState, ConnectionUpdate, WakeKind},
};
use ruma::{
	DeviceId, UserId, api::client::sync::sync_events::v5, assign, presence::PresenceState, uint,
};
use tokio::time::{Instant, timeout_at};

use self::{
	extensions::build_extensions,
	lists::select_rooms,
	room::{add_sticky_senders, build_rooms},
};
use crate::{Ruma, client_ip::ClientIp};

const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// A response that takes this long to build is reported at `info`.
const SLOW_BUILD: Duration = Duration::from_millis(100);

/// MSC4186 allows at most this many lists and room subscriptions; the same
/// bound keeps ranges and `required_state` from growing the work unbounded.
const MAX_ENTRIES: usize = 100;

/// References to common data needed to calculate the sync response.
#[derive(Clone, Copy)]
struct SyncContext<'a> {
	/// The ID of the user requesting this sync.
	syncing_user: &'a UserId,
	/// The ID of the device requesting this sync, which will belong to
	/// `syncing_user`.
	syncing_device: &'a DeviceId,
	request: &'a v5::Request,
	connection: &'a ConnectionState,
	/// The global count as of when we started building the response, which
	/// bounds every read.
	next_batch: u64,
}

impl SyncContext<'_> {
	fn since(&self) -> u64 { self.connection.count.unwrap_or(0) }
}

/// `POST /_matrix/client/unstable/org.matrix.simplified_msc3575/sync`
/// ([MSC4186])
///
/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
pub(crate) async fn sync_events_v5_route(
	State(ref services): State<crate::State>,
	ClientIp(client_ip): ClientIp, // NOTE: Required for updating device metadata
	body: Ruma<v5::Request>,
) -> Result<v5::Response> {
	let syncing_user = body.identity.expect_sender_user()?;
	let syncing_device = body.identity.expect_sender_device()?;

	services
		.users
		.update_device_last_seen(syncing_user, Some(syncing_device), client_ip)
		.await;

	let mut request = body.body;
	validate(&request, services.globals.current_count()?)?;

	if services.config.allow_local_presence && request.set_presence != PresenceState::Offline {
		services
			.presence
			.ping_presence(syncing_user, &request.set_presence)
			.await?;
	}

	let key = ConnectionKey {
		user_id: syncing_user.to_owned(),
		device_id: syncing_device.to_owned(),
		conn_id: request.conn_id.clone(),
	};

	let pos = request.pos.take();
	let connection = services.sync.begin_request(&key, pos.as_deref())?;

	extensions::acknowledge_to_device(
		services,
		syncing_user,
		syncing_device,
		&request,
		&connection,
	)
	.await;

	// a request without a position must be answered straight away
	let timeout = connection
		.count
		.and(request.timeout)
		.map_or(Duration::ZERO, |timeout| cmp::min(timeout, MAX_TIMEOUT));
	let deadline = Instant::now()
		.checked_add(timeout)
		.expect("timeout is bounded by MAX_TIMEOUT");

	// subscribe first so no update is missed
	let mut wake = services
		.sync
		.subscribe_to_wake(syncing_user, &wake_interest(&request))
		.await;

	loop {
		let context = SyncContext {
			syncing_user,
			syncing_device,
			request: &request,
			connection: &connection,
			next_batch: services.globals.current_count()?,
		};

		let (mut response, update) = build_response(services, context).await?;

		// a changed list count is news even without room data, and clients
		// replenish their one-time keys as soon as the counts move, and send
		// encryption requests between syncs of a connection without rooms
		let respond = !is_empty(&response)
			|| update.list_counts.is_some()
			|| match timeout_at(deadline, wake.woken()).await {
				| Ok(Some(woken)) => {
					debug!(conn_id = ?request.conn_id, ?woken, "sliding sync woken");
					!has_rooms(&request) || woken.contains(&WakeKind::Keys)
				},
				| Ok(None) | Err(_) => true,
			};

		if respond {
			response.pos = services
				.sync
				.finish_request(&key, &connection, context.next_batch, update)
				.to_string();

			trace!(
				rooms = response.rooms.len(),
				pos = %response.pos,
				"responding to sliding sync request"
			);

			return Ok(response);
		}
	}
}

/// The changes that can alter a response to this request, so that a connection
/// asking only for encryption data sleeps through room activity.
fn wake_interest(request: &v5::Request) -> Vec<WakeKind> {
	let extensions = &request.extensions;
	let enabled = |enabled: Option<bool>| enabled == Some(true);
	let rooms = has_rooms(request);

	[
		(rooms, WakeKind::Timeline),
		(rooms || enabled(extensions.account_data.enabled), WakeKind::AccountData),
		(rooms && enabled(extensions.typing.enabled), WakeKind::Typing),
		(rooms && enabled(extensions.receipts.enabled), WakeKind::Receipts),
		(enabled(extensions.to_device.enabled), WakeKind::ToDevice),
		(enabled(extensions.e2ee.enabled), WakeKind::Keys),
		(enabled(extensions.e2ee.enabled), WakeKind::DeviceLists),
	]
	.into_iter()
	.filter_map(|(interested, kind)| interested.then_some(kind))
	.collect()
}

fn has_rooms(request: &v5::Request) -> bool {
	!request.lists.is_empty() || !request.room_subscriptions.is_empty()
}

fn validate(request: &v5::Request, current_count: u64) -> Result {
	let too_many = request.lists.len() > MAX_ENTRIES
		|| request.room_subscriptions.len() > MAX_ENTRIES
		|| request.lists.values().any(|list| {
			list.ranges.len() > MAX_ENTRIES
				|| list.room_details.required_state.len() > MAX_ENTRIES
		})
		|| request
			.room_subscriptions
			.values()
			.any(|subscription| subscription.required_state.len() > MAX_ENTRIES);

	if too_many {
		return Err!(Request(InvalidParam(
			"Too many lists, subscriptions, ranges or required_state entries."
		)));
	}

	let extensions = &request.extensions;
	if extensions.to_device.limit == Some(uint!(0))
		|| extensions.sticky_events.limit == Some(uint!(0))
	{
		return Err!(Request(BadJson("Extension limits must be positive.")));
	}

	// a position past the stream would acknowledge messages not yet delivered
	let invalid = |since: Option<&str>| {
		since.is_some_and(|since| {
			!since
				.parse::<u64>()
				.is_ok_and(|since| since <= current_count)
		})
	};
	if invalid(extensions.to_device.since.as_deref())
		|| invalid(extensions.sticky_events.since.as_deref())
	{
		return Err!(Request(InvalidParam("Invalid extension `since`.")));
	}

	Ok(())
}

async fn build_response(
	services: &Services,
	context: SyncContext<'_>,
) -> Result<(v5::Response, ConnectionUpdate)> {
	let started = Instant::now();
	let mut selection = select_rooms(services, context).await?;
	let selected = started.elapsed();
	let counts_changed = *selection.counts() != context.connection.list_counts;
	let rooms = build_rooms(services, context, &selection).await;
	let built_rooms = started.elapsed();
	let (extensions, scopes) = build_extensions(services, context, &selection, &rooms).await?;
	log_build(context, selection.rooms.len(), [selected, built_rooms, started.elapsed()]);

	let mut room_results = rooms.rooms;
	add_sticky_senders(services, &selection, &mut room_results, &extensions.sticky_events).await;

	let response = assign!(v5::Response::new(String::new()), {
		txn_id: context.request.txn_id.clone(),
		lists: selection.lists(),
		rooms: room_results,
		extensions,
	});

	let update = ConnectionUpdate {
		rooms: rooms.updates,
		left_rooms: selection.left_rooms.take(),
		to_device_through_pos: response
			.extensions
			.to_device
			.as_ref()
			.is_some_and(|to_device| to_device.next_batch == context.next_batch.to_string()),
		list_counts: counts_changed.then(|| selection.counts().clone()),
		account_data: scopes.account_data,
		receipts: scopes.receipts,
		typing: scopes.typing,
	};

	Ok((response, update))
}

/// `phases` are the elapsed times after selecting rooms, building them and
/// building the extensions.
fn log_build(context: SyncContext<'_>, rooms: usize, phases: [Duration; 3]) {
	let [selected, built_rooms, total] = phases;
	let conn_id = context.request.conn_id.as_deref();
	let rooms_took = built_rooms.saturating_sub(selected);
	let extensions_took = total.saturating_sub(built_rooms);

	if total >= SLOW_BUILD {
		info!(
			?conn_id,
			rooms,
			?selected,
			?rooms_took,
			?extensions_took,
			?total,
			"slow sliding sync response build"
		);
	} else {
		debug!(
			?conn_id,
			rooms,
			?selected,
			?rooms_took,
			?extensions_took,
			?total,
			"built sliding sync response"
		);
	}
}

fn is_empty(response: &v5::Response) -> bool {
	let extensions = &response.extensions;

	response.rooms.is_empty()
		&& extensions.account_data.is_empty()
		&& extensions.receipts.is_empty()
		&& extensions.typing.is_empty()
		&& extensions.sticky_events.rooms.is_empty()
		&& extensions.e2ee.device_lists.is_empty()
		&& extensions
			.to_device
			.as_ref()
			.is_none_or(|to_device| to_device.events.is_empty())
}

#[cfg(test)]
mod tests {
	use ruma::{
		api::client::sync::sync_events::{DeviceLists, v5::response},
		owned_room_id, owned_user_id,
		serde::Raw,
	};

	use super::*;

	fn response() -> v5::Response { v5::Response::new(String::new()) }

	fn encryption_request() -> v5::Request {
		let mut request = v5::Request::new();
		request.extensions.to_device.enabled = Some(true);
		request.extensions.e2ee.enabled = Some(true);
		request
	}

	#[test]
	fn an_extension_position_must_have_been_issued() {
		let mut request = encryption_request();
		request.extensions.to_device.since = Some("10".to_owned());
		validate(&request, 10).unwrap();

		request.extensions.to_device.since = Some("11".to_owned());
		validate(&request, 10).unwrap_err();

		request.extensions.to_device.since = Some("not-a-position".to_owned());
		validate(&request, 10).unwrap_err();

		request.extensions.to_device.since = None;
		request.extensions.sticky_events.since = Some("11".to_owned());
		validate(&request, 10).unwrap_err();
	}

	#[test]
	fn an_extension_limit_must_be_positive() {
		let mut request = encryption_request();
		request.extensions.to_device.limit = Some(uint!(0));

		validate(&request, 10).unwrap_err();

		request.extensions.to_device.limit = Some(uint!(1));
		validate(&request, 10).unwrap();
	}

	#[test]
	fn an_encryption_only_connection_ignores_room_activity() {
		assert_eq!(wake_interest(&encryption_request()), [
			WakeKind::ToDevice,
			WakeKind::Keys,
			WakeKind::DeviceLists,
		]);
	}

	#[test]
	fn rooms_add_what_a_room_response_reports() {
		let mut request = encryption_request();
		request.extensions.typing.enabled = Some(true);
		request
			.room_subscriptions
			.insert(owned_room_id!("!a:example.com"), v5::request::RoomSubscription::default());

		let interest = wake_interest(&request);

		for kind in [
			WakeKind::Timeline,
			WakeKind::AccountData,
			WakeKind::Typing,
			WakeKind::ToDevice,
			WakeKind::Keys,
			WakeKind::DeviceLists,
		] {
			assert!(interest.contains(&kind), "{kind:?}");
		}
		assert!(!interest.contains(&WakeKind::Receipts));
	}

	#[test]
	fn a_fresh_response_is_empty() {
		assert!(is_empty(&response()));
	}

	#[test]
	fn a_room_makes_the_response_non_empty() {
		let mut response = response();
		response
			.rooms
			.insert(owned_room_id!("!a:example.com"), response::Room::default());

		assert!(!is_empty(&response));
	}

	#[test]
	fn extension_data_makes_the_response_non_empty() {
		let mut typing = response();
		typing.extensions.typing.rooms.insert(
			owned_room_id!("!a:example.com"),
			Raw::from_json_string("{}".to_owned()).unwrap(),
		);

		let mut device_lists = response();
		device_lists.extensions.e2ee.device_lists =
			assign!(DeviceLists::new(), { changed: vec![owned_user_id!("@a:example.com")] });

		assert!(!is_empty(&typing));
		assert!(!is_empty(&device_lists));
	}

	#[test]
	fn key_counts_and_an_empty_to_device_batch_do_not_count() {
		let mut response = response();
		response.extensions.to_device = Some(response::ToDevice::default());
		response.extensions.e2ee.device_unused_fallback_key_types = Some(Vec::new());

		assert!(is_empty(&response));
	}
}
