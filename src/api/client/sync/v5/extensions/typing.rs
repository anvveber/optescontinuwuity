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

use super::{SyncContext, scope};
use crate::client::sync::v5::lists::RoomSelection;

/// Builds the [MSC4508] typing extension: who is typing in each joined room in
/// scope whose typing state changed since the last response.
///
/// [MSC4508]: https://github.com/matrix-org/matrix-spec-proposals/pull/4508
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
			// forgetting the scope makes enabling it again start over
			previous.enabled.then(ExtensionScope::default),
		);
	}

	// rooms the user is not joined to are left out, so a room joined later
	// enters the scope then and gets its full state
	let scope: HashSet<_> = scope(selection, config.lists.as_deref(), config.rooms.as_deref())
		.into_iter()
		.filter(|room_id| selection.is_joined(room_id))
		.collect();
	let typing = &services.rooms.typing;

	let rooms = scope
		.iter()
		.stream()
		.wide_filter_map(async |room_id| {
			// a room re-entering the scope gets its current state
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

			// a room entering the scope with nobody typing has nothing to send,
			// as the client already assumes nobody is typing
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

	(typing, previous.advance(scope, context.since()))
}
