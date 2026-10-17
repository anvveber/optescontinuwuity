mod extensions;
mod lists;
mod room;

use std::{cmp, time::Duration};

use axum::extract::State;
use conduwuit::{Err, Result, debug, trace};
use conduwuit_service::{
	Services,
	sync::{ConnectionKey, ConnectionState, ConnectionUpdate},
};
use ruma::{
	DeviceId, UserId, api::client::sync::sync_events::v5, assign, presence::PresenceState, uint,
};
use tokio::time::{Instant, timeout_at};

use self::{extensions::build_extensions, lists::select_rooms, room::build_rooms};
use crate::{Ruma, client_ip::ClientIp};

/// The longest a request waits for changes. [MSC4186] leaves this to the
/// server, so this is the same cap `/v3/sync` uses.
///
/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// [MSC4186] allows at most this many lists and room subscriptions. The same
/// bound keeps ranges and `required_state` from growing the work unbounded.
///
/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
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

	// clients send pending encryption requests, such as key backups, between
	// syncs, so a connection without rooms returns on any activity
	let extensions_only = request.lists.is_empty() && request.room_subscriptions.is_empty();

	// subscribe first so no update is missed
	let mut wake = services.sync.subscribe_to_wake(syncing_user).await;

	loop {
		let context = SyncContext {
			syncing_user,
			syncing_device,
			request: &request,
			connection: &connection,
			next_batch: services.globals.current_count()?,
		};

		let (mut response, update) = build_response(services, context).await?;

		// a changed list count must be sent, even without any room data
		let respond = !is_empty(&response)
			|| update.list_counts.is_some()
			|| !matches!(timeout_at(deadline, wake.changed()).await, Ok(Ok(())))
			|| extensions_only;

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
	if extensions.to_device.limit == Some(uint!(0)) {
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
	if invalid(extensions.to_device.since.as_deref()) {
		return Err!(Request(InvalidParam("Invalid extension `since`.")));
	}

	Ok(())
}

/// Builds a response from the connection's last acknowledged state, along with
/// the update to commit once the client acknowledges it.
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
	let (extensions, scopes) = build_extensions(services, context, &selection).await?;
	if cfg!(debug_assertions) {
		log_build(context, selection.rooms.len(), [selected, built_rooms, started.elapsed()]);
	}

	let response = assign!(v5::Response::new(String::new()), {
		txn_id: context.request.txn_id.clone(),
		lists: selection.lists(),
		rooms: rooms.rooms,
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
		typing: scopes.typing,
	};

	Ok((response, update))
}

/// Logs how long each phase of building a response took. `phases` are the
/// elapsed times after selecting rooms, building them and building the
/// extensions.
fn log_build(context: SyncContext<'_>, rooms: usize, phases: [Duration; 3]) {
	let [selected, built_rooms, total] = phases;

	debug!(
		conn_id = ?context.request.conn_id.as_deref(),
		rooms,
		?selected,
		rooms_took = ?built_rooms.saturating_sub(selected),
		extensions_took = ?total.saturating_sub(built_rooms),
		?total,
		"built sliding sync response"
	);
}

fn is_empty(response: &v5::Response) -> bool {
	let extensions = &response.extensions;

	response.rooms.is_empty()
		&& extensions.typing.is_empty()
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
