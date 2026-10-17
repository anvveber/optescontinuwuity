mod e2ee;
mod to_device;
mod typing;

use std::collections::HashSet;

use conduwuit::Result;
use conduwuit_service::{Services, sync::ExtensionScope};
use futures::future::try_join;
use ruma::{
	OwnedRoomId,
	api::client::sync::sync_events::v5::{self, request::ExtensionRoomConfig},
	assign,
};

pub(super) use self::to_device::acknowledge as acknowledge_to_device;
use super::{SyncContext, lists::RoomSelection};

pub(super) struct ScopeUpdates {
	pub(super) typing: Option<ExtensionScope>,
}

/// Builds every enabled extension, along with the room scopes the connection
/// should remember once the client acknowledges the response.
pub(super) async fn build_extensions(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
) -> Result<(v5::response::Extensions, ScopeUpdates)> {
	let (typing, typing_scope) = typing::collect(services, context, selection).await;

	let (e2ee, to_device) =
		try_join(e2ee::collect(services, context), to_device::collect(services, context)).await?;

	let extensions = assign!(v5::response::Extensions::default(), {
		typing,
		e2ee,
		to_device,
	});

	let scopes = ScopeUpdates { typing: typing_scope };

	Ok((extensions, scopes))
}

/// Returns the rooms an extension covers, as selected by its [MSC4508] `lists`
/// and `rooms` fields.
///
/// `lists` names the lists whose in-range rooms are covered, and `rooms` names
/// the room subscriptions that are covered. Leaving either out covers every
/// list or every subscription respectively.
///
/// [MSC4508]: https://github.com/matrix-org/matrix-spec-proposals/pull/4508
fn scope(
	selection: &RoomSelection,
	lists: Option<&[String]>,
	rooms: Option<&[ExtensionRoomConfig]>,
) -> HashSet<OwnedRoomId> {
	let in_range: Vec<&Vec<OwnedRoomId>> = match lists {
		| None => selection.in_range.values().collect(),
		| Some(lists) => lists
			.iter()
			.filter_map(|list_id| selection.in_range.get(list_id))
			.collect(),
	};

	let is_covered = |room_id: &OwnedRoomId| {
		rooms.is_none_or(|rooms| {
			rooms.iter().any(|room| match room {
				| ExtensionRoomConfig::AllSubscribed => true,
				| ExtensionRoomConfig::Room(covered) => covered == room_id,
				| _ => false,
			})
		})
	};

	let subscriptions = selection
		.subscriptions
		.iter()
		.filter(|room_id| is_covered(room_id));

	in_range
		.into_iter()
		.flatten()
		.chain(subscriptions)
		.cloned()
		.collect()
}

#[cfg(test)]
mod tests {
	use ruma::owned_room_id;

	use super::*;

	fn selection() -> RoomSelection {
		let mut selection = RoomSelection::default();
		selection
			.in_range
			.insert("first".to_owned(), vec![owned_room_id!("!list-a:example.com")]);
		selection
			.in_range
			.insert("second".to_owned(), vec![owned_room_id!("!list-b:example.com")]);
		selection.subscriptions.extend([
			owned_room_id!("!subscription-a:example.com"),
			owned_room_id!("!subscription-b:example.com"),
		]);
		selection
	}

	#[test]
	fn scope_uses_the_selected_lists_and_subscriptions() {
		assert_eq!(
			scope(
				&selection(),
				Some(&["first".to_owned()]),
				Some(&[
					ExtensionRoomConfig::Room(owned_room_id!("!list-b:example.com")),
					ExtensionRoomConfig::Room(owned_room_id!("!subscription-b:example.com")),
				]),
			),
			HashSet::from([
				owned_room_id!("!list-a:example.com"),
				owned_room_id!("!subscription-b:example.com"),
			])
		);
	}

	#[test]
	fn scope_defaults_to_all_lists_and_subscriptions() {
		assert_eq!(scope(&selection(), None, None).len(), 4);
	}

	#[test]
	fn empty_scope_selectors_match_nothing() {
		assert!(scope(&selection(), Some(&[]), Some(&[])).is_empty());
	}
}
