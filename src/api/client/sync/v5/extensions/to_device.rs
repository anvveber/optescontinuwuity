use conduwuit::{Result, utils::math::usize_from_ruma};
use conduwuit_service::{Services, sync::ConnectionState};
use futures::StreamExt;
use ruma::{DeviceId, UserId, api::client::sync::sync_events::v5, assign};

use super::SyncContext;

const DEFAULT_LIMIT: usize = 100;

/// Clients which do not send a `since` acknowledge messages with `pos`, but
/// only if that response delivered everything up to it.
fn since(request: &v5::Request, connection: &ConnectionState) -> u64 {
	request
		.extensions
		.to_device
		.since
		.as_deref()
		.and_then(|since| since.parse().ok())
		.or_else(|| {
			connection
				.to_device_through_pos
				.then_some(connection.count)
				.flatten()
		})
		.unwrap_or(0)
}

pub(in super::super) async fn acknowledge(
	services: &Services,
	syncing_user: &UserId,
	syncing_device: &DeviceId,
	request: &v5::Request,
	connection: &ConnectionState,
) {
	if request.extensions.to_device.enabled != Some(true) {
		return;
	}

	let since = since(request, connection);
	if since != 0 {
		services
			.users
			.remove_to_device_events(syncing_user, syncing_device, since)
			.await;
	}
}

pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
) -> Result<Option<v5::response::ToDevice>> {
	let SyncContext {
		syncing_user,
		syncing_device,
		request,
		connection,
		next_batch,
	} = context;
	let config = &request.extensions.to_device;

	if config.enabled != Some(true) {
		return Ok(None);
	}

	let limit = config
		.limit
		.map_or(DEFAULT_LIMIT, usize_from_ruma)
		.clamp(1, DEFAULT_LIMIT);

	let mut messages: Vec<_> = services
		.users
		.get_to_device_events(
			syncing_user,
			syncing_device,
			Some(since(request, connection)),
			Some(next_batch),
		)
		.take(limit.saturating_add(1))
		.collect()
		.await;

	// with more messages waiting, the batch ends at the last one returned
	let token = if messages.len() > limit {
		messages.truncate(limit);
		messages.last().map_or(next_batch, |(count, _)| *count)
	} else {
		next_batch
	};

	Ok(Some(assign!(v5::response::ToDevice::default(), {
		next_batch: token.to_string(),
		events: messages.into_iter().map(|(_, message)| message).collect(),
	})))
}
