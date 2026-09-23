use std::collections::HashSet;

use conduwuit::{
	utils::{IterStream, stream::WidebandExt},
	warn,
};
use conduwuit_service::{Services, sync::ExtensionScope};
use futures::StreamExt;
use ruma::{
	api::client::sync::sync_events::v5,
	assign,
	events::typing::{SyncTypingEvent, TypingEventContent},
	serde::Raw,
};

use super::{SyncContext, scope, scope_update};
use crate::client::sync::v5::lists::RoomSelection;

pub(super) async fn collect(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
) -> (v5::response::Typing, Option<ExtensionScope>) {
	let SyncContext { syncing_user, request, connection, .. } = context;
	let config = &request.extensions.typing;
	let previous = &connection.typing;
	let enabled = config.enabled == Some(true);

	if !enabled {
		return (
			v5::response::Typing::default(),
			scope_update(context, previous, false, HashSet::new()),
		);
	}

	// a room joined later enters the scope then, getting its full state
	let scope: HashSet<_> = scope(selection, config.lists.as_deref(), config.rooms.as_deref())
		.into_iter()
		.filter(|room_id| selection.is_joined(room_id))
		.collect();
	let typing = &services.rooms.typing;

	let rooms = scope
		.iter()
		.stream()
		.wide_filter_map(async |room_id| {
			// MSC4508: a room re-entering the scope gets its current state
			let since = if previous.enabled && previous.rooms.contains(room_id) {
				context.since()
			} else {
				0
			};
			let last_update = typing
				.last_typing_update(room_id)
				.await
				.inspect_err(
					|error| warn!(%room_id, %error, "Failed to check for typing updates"),
				)
				.ok()?;

			if last_update <= since {
				return None;
			}

			let user_ids = typing
				.typing_users_for_user(room_id, syncing_user)
				.await
				.inspect_err(|error| warn!(%room_id, %error, "Failed to get typing users"))
				.ok()?;

			// entering the scope, nobody typing is what the client assumes
			if since == 0 && user_ids.is_empty() {
				return None;
			}

			let event = Raw::new(&SyncTypingEvent::new(TypingEventContent::new(user_ids)))
				.expect("typing event should be valid");

			Some((room_id.clone(), event))
		})
		.collect()
		.await;

	let typing = assign!(v5::response::Typing::default(), { rooms });

	(typing, scope_update(context, previous, true, scope))
}
