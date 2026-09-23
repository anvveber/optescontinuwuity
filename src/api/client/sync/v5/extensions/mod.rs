mod account_data;
mod e2ee;
mod receipts;
mod sticky;
mod to_device;
mod typing;

use std::collections::HashSet;

use conduwuit::Result;
use conduwuit_service::{Services, sync::ExtensionScope};
use futures::future::{join3, try_join3};
use ruma::{
	OwnedRoomId,
	api::client::sync::sync_events::v5::{self, request::ExtensionRoomConfig},
	assign,
};

pub(super) use self::to_device::acknowledge as acknowledge_to_device;
use super::{SyncContext, lists::RoomSelection, room::BuiltRooms};

pub(super) struct ScopeUpdates {
	pub(super) account_data: Option<ExtensionScope>,
	pub(super) receipts: Option<ExtensionScope>,
	pub(super) typing: Option<ExtensionScope>,
}

pub(super) async fn build_extensions(
	services: &Services,
	context: SyncContext<'_>,
	selection: &RoomSelection,
	rooms: &BuiltRooms,
) -> Result<(v5::response::Extensions, ScopeUpdates)> {
	let ((account_data, account_data_scope), (receipts, receipts_scope), (typing, typing_scope)) =
		join3(
			account_data::collect(services, context, selection),
			receipts::collect(services, context, selection, rooms),
			typing::collect(services, context, selection),
		)
		.await;

	let (e2ee, to_device, sticky_events) = try_join3(
		e2ee::collect(services, context),
		to_device::collect(services, context),
		sticky::collect(services, context, selection, rooms),
	)
	.await?;

	let extensions = assign!(v5::response::Extensions::default(), {
		account_data,
		receipts,
		typing,
		e2ee,
		to_device,
		sticky_events,
	});

	let scopes = ScopeUpdates {
		account_data: account_data_scope,
		receipts: receipts_scope,
		typing: typing_scope,
	};

	Ok((extensions, scopes))
}

/// MSC4508: rooms in range of the named lists and the named subscriptions,
/// all of either by default.
fn scope(
	selection: &RoomSelection,
	lists: Option<&[String]>,
	rooms: Option<&[ExtensionRoomConfig]>,
) -> HashSet<OwnedRoomId> {
	let mut scope = HashSet::new();

	match lists {
		| None => scope.extend(selection.in_range.values().flatten().cloned()),
		| Some(lists) => scope.extend(
			lists
				.iter()
				.filter_map(|list_id| selection.in_range.get(list_id))
				.flatten()
				.cloned(),
		),
	}

	match rooms {
		| None => scope.extend(selection.subscriptions.iter().cloned()),
		| Some(rooms) =>
			for room in rooms {
				match room {
					| ExtensionRoomConfig::AllSubscribed =>
						scope.extend(selection.subscriptions.iter().cloned()),
					| ExtensionRoomConfig::Room(room_id)
						if selection.subscriptions.contains(room_id) =>
					{
						scope.insert(room_id.clone());
					},
					| _ => {},
				}
			},
	}

	scope
}

/// A disabled extension forgets its scope, so enabling it again starts over.
fn scope_update(
	context: SyncContext<'_>,
	previous: &ExtensionScope,
	enabled: bool,
	scope: HashSet<OwnedRoomId>,
) -> Option<ExtensionScope> {
	if !enabled {
		return previous.enabled.then(ExtensionScope::default);
	}

	previous.advance(scope, context.since())
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
