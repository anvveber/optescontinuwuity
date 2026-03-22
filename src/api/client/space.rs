use std::{collections::VecDeque, str::FromStr};

use axum::extract::State;
use conduwuit::{
	Err, Result,
	utils::{future::TryExtExt, stream::IterStream},
};
use conduwuit_service::{
	Services,
	rooms::spaces::{
		PaginationToken, SummaryAccessibility, get_parent_children_via, summary_to_chunk,
	},
};
use futures::{StreamExt, future::OptionFuture};
use ruma::{
	OwnedRoomId, OwnedServerName, RoomId, UInt, UserId, api::client::space::get_hierarchy,
};

use crate::Ruma;

/// # `GET /_matrix/client/v1/rooms/{room_id}/hierarchy`
///
/// Paginates over the space tree in a depth-first manner to locate child rooms
/// of a given space.
pub(crate) async fn get_hierarchy_route(
	State(services): State<crate::State>,
	body: Ruma<get_hierarchy::v1::Request>,
) -> Result<get_hierarchy::v1::Response> {
	let limit = body
		.limit
		.unwrap_or_else(|| UInt::from(10_u32))
		.min(UInt::from(100_u32));

	let max_depth = body
		.max_depth
		.unwrap_or_else(|| UInt::from(3_u32))
		.min(UInt::from(10_u32));

	let key = body
		.from
		.as_ref()
		.and_then(|s| PaginationToken::from_str(s).ok());

	// Should prevent unexpected behaviour in (bad) clients
	if let Some(ref token) = key {
		if token.suggested_only != body.suggested_only || token.max_depth != max_depth {
			return Err!(Request(InvalidParam(
				"suggested_only and max_depth cannot change on paginated requests"
			)));
		}
	}

	get_client_hierarchy(
		&services,
		body.sender_user(),
		&body.room_id,
		limit.try_into().unwrap_or(10),
		max_depth.try_into().unwrap_or(usize::MAX),
		body.suggested_only,
		key.as_ref()
			.into_iter()
			.flat_map(|t| t.short_room_ids.iter()),
	)
	.await
}

async fn get_client_hierarchy<'a, ShortRoomIds>(
	services: &Services,
	sender_user: &UserId,
	room_id: &RoomId,
	limit: usize,
	max_depth: usize,
	suggested_only: bool,
	short_room_ids: ShortRoomIds,
) -> Result<get_hierarchy::v1::Response>
where
	ShortRoomIds: Iterator<Item = &'a u64> + Clone + Send + Sync + 'a,
{
	type Via = Vec<OwnedServerName>;
	type Entry = (Vec<OwnedRoomId>, Via);
	type Rooms = VecDeque<Entry>;

	let short_room_ids = short_room_ids.collect::<Vec<_>>();

	let mut queue: Rooms = [(
		vec![room_id.to_owned()],
		room_id
			.server_name()
			.map(ToOwned::to_owned)
			.into_iter()
			.collect(),
	)]
	.into();

	let mut rooms = Vec::with_capacity(limit);
	let mut start_populating = short_room_ids.is_empty();
	while let Some((path, via)) = queue.pop_front() {
		let current_room = path
			.last()
			.expect("the path should have at least one room ID");
		let summary = services
			.rooms
			.spaces
			.get_summary_and_children_client(current_room, suggested_only, sender_user, &via)
			.await?;

		match (summary, current_room == room_id) {
			| (None | Some(SummaryAccessibility::Inaccessible), false) => {
				// Just ignore other unavailable rooms
			},
			| (None, true) => {
				return Err!(Request(Forbidden("The requested room was not found")));
			},
			| (Some(SummaryAccessibility::Inaccessible), true) => {
				return Err!(Request(Forbidden("The requested room is inaccessible")));
			},
			| (Some(SummaryAccessibility::Accessible(summary)), _) => {
				if path.len() > max_depth.saturating_add(1) {
					continue;
				}

				if !start_populating {
					if path.len() > short_room_ids.len() {
						// Not interested - we've gone too deep and we haven't hit the last room
						// that was sent yet, so we can skip this tree of rooms
						continue;
					}

					if path.len() == short_room_ids.len() {
						let is_last_sent_room = path
							.iter()
							.stream()
							.all(async |parent_room| {
								let parent_short_room_id = services
									.rooms
									.short
									.get_shortroomid(parent_room)
									.await
									.expect("rooms should have a short room ID");
								short_room_ids.contains(&&parent_short_room_id)
							})
							.await;
						if is_last_sent_room {
							start_populating = true;
						}
					}
				}

				let children: Vec<Entry> = get_parent_children_via(&summary, suggested_only)
					.filter(|(room, _)| !path.contains(room))
					.map(|(key, val)| {
						let mut path = path.clone();
						path.push(key);
						(path, val.collect())
					})
					.collect();

				if start_populating {
					rooms.push(summary_to_chunk(summary));
				}

				if !start_populating && queue.is_empty() && children.is_empty() {
					break;
				}

				if rooms.len() >= limit {
					break;
				}

				queue.extend(children);
			},
		}
	}

	let next_batch: OptionFuture<_> = queue
		.pop_front()
		.map(|(path, _)| async move {
			let next_short_room_ids: Vec<_> = path
				.iter()
				.stream()
				.filter_map(|room_id| services.rooms.short.get_shortroomid(room_id).ok())
				.collect()
				.await;

			(next_short_room_ids.iter().ne(short_room_ids) && !next_short_room_ids.is_empty())
				.then_some(PaginationToken {
					short_room_ids: next_short_room_ids,
					limit: limit.try_into().ok()?,
					max_depth: max_depth.try_into().ok()?,
					suggested_only,
				})
				.as_ref()
				.map(PaginationToken::to_string)
		})
		.into();

	Ok(get_hierarchy::v1::Response {
		next_batch: next_batch.await.flatten(),
		rooms,
	})
}
