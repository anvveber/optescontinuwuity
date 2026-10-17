use conduwuit::{
	extract_variant,
	utils::{IterStream, ReadyExt, stream::WidebandExt},
};
use conduwuit_service::{Services, account_data::AnyRawAccountDataEvent, sync::ExtensionScope};
use futures::StreamExt;
use ruma::{api::client::sync::sync_events::v5, assign};

use super::{SyncContext, scope};
use crate::client::sync::v5::lists::RoomSelection;

/// Builds the [MSC4548] account data extension: global account data changed
/// since the last response, and room account data for the rooms in scope.
///
/// A room entering the scope gets all of its account data, and a room
/// re-entering it gets what changed since it left.
///
/// [MSC4548]: https://github.com/matrix-org/matrix-spec-proposals/pull/4548
pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
) -> (v5::response::AccountData, Option<ExtensionScope>) {
	let SyncContext {
		syncing_user,
		request,
		connection,
		next_batch,
		..
	} = context;
	let config = &request.extensions.account_data;
	let previous = &connection.account_data;
	let enabled = config.enabled == Some(true);

	if !enabled {
		return (
			v5::response::AccountData::default(),
			// forgetting the scope makes enabling it again start over
			previous.enabled.then(ExtensionScope::default),
		);
	}

	let scope = scope(selection, config.lists.as_deref(), config.rooms.as_deref());
	let global_since = if previous.enabled { context.since() } else { 0 };

	let global = services
		.account_data
		.changes_since(None, syncing_user, Some(global_since), Some(next_batch))
		.ready_filter_map(|event| extract_variant!(event, AnyRawAccountDataEvent::Global))
		.collect()
		.await;

	let rooms = scope
		.iter()
		.stream()
		.wide_filter_map(async |room_id| {
			let since = previous.since(room_id, context.since());
			let events: Vec<_> = services
				.account_data
				.changes_since(Some(room_id), syncing_user, Some(since), Some(next_batch))
				.ready_filter_map(|event| extract_variant!(event, AnyRawAccountDataEvent::Room))
				.collect()
				.await;

			(!events.is_empty()).then(|| (room_id.clone(), events))
		})
		.collect()
		.await;

	let account_data = assign!(v5::response::AccountData::default(), { global, rooms });

	(account_data, previous.advance(scope, context.since()))
}
