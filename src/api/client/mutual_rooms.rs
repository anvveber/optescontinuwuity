use axum::extract::State;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use conduwuit::{Err, Result, err};
use conduwuit_service::Services;
use futures::StreamExt;
use ruma::{RoomId, UInt, UserId, api::client::membership::mutual_rooms};

use crate::Ruma;

/// Maximum number of rooms returned per page.
const MAX_BATCH_SIZE: usize = 100;

/// # `GET /_matrix/client/unstable/uk.half-shot.msc2666/user/mutual_rooms`
///
/// Gets the rooms the sender shares with the specified user, with pagination.
///
/// An implementation of [MSC2666](https://github.com/matrix-org/matrix-spec-proposals/pull/2666)
#[tracing::instrument(skip_all, name = "mutual_rooms", level = "info")]
pub(crate) async fn get_mutual_rooms_unstable_route(
	State(services): State<crate::State>,
	body: Ruma<mutual_rooms::unstable::Request>,
) -> Result<mutual_rooms::unstable::Response> {
	let sender_user = body.identity.expect_sender_user()?;

	if sender_user == body.user_id {
		return Err!(Request(InvalidParam("You cannot request rooms in common with yourself.")));
	}

	let (_, joined, next_batch_token) =
		query_shared_rooms(&services, sender_user, &body.user_id, body.batch_token.as_deref())
			.await?;

	Ok(match next_batch_token {
		| Some(token) => mutual_rooms::unstable::Response::with_token(joined, token),
		| None => mutual_rooms::unstable::Response::new(joined),
	})
}

/// # `GET /_matrix/client/v1/mutual_rooms`
///
/// Gets the rooms the sender shares with the specified user, with pagination.
#[tracing::instrument(skip_all, name = "mutual_rooms", level = "info")]
pub(crate) async fn get_mutual_rooms_route(
	State(services): State<crate::State>,
	body: Ruma<mutual_rooms::v1::Request>,
) -> Result<mutual_rooms::v1::Response> {
	let sender_user = body.identity.expect_sender_user()?;

	if sender_user == body.user_id {
		return Err!(Request(InvalidParam("You cannot request rooms in common with yourself.")));
	}

	let (count, joined, next_batch) =
		query_shared_rooms(&services, sender_user, &body.user_id, body.from.as_deref()).await?;

	Ok(match next_batch {
		| Some(token) => mutual_rooms::v1::Response::with_batch_token(count, joined, token),
		| None => mutual_rooms::v1::Response::new(count, joined),
	})
}

/// Fetches one page of the rooms shared by two users, with MSC2666 pagination.
///
/// Returns the total shared-room count, the page of room IDs, and a
/// `next_batch` token when more rooms remain.
async fn query_shared_rooms(
	services: &Services,
	user_a: &UserId,
	user_b: &UserId,
	from: Option<&str>,
) -> Result<(UInt, Vec<ruma::OwnedRoomId>, Option<String>)> {
	let total: UInt = services
		.rooms
		.state_cache
		.get_shared_rooms(user_a, user_b)
		.count()
		.await
		.try_into()
		.expect("user should be in fewer than 9.1 quadrillion rooms");

	let cursor = match from {
		| Some(token) => Some(decode_batch_token(token)?),
		| None => None,
	};

	let mut stream = services
		.rooms
		.state_cache
		.get_shared_rooms(user_a, user_b)
		.boxed();
	let mut page: Vec<ruma::OwnedRoomId> = Vec::with_capacity(MAX_BATCH_SIZE + 1);

	// Resume from the cursor of a previous batch, if any
	if let Some(cursor) = cursor.as_ref() {
		while let Some(room) = stream.next().await {
			if &room > cursor {
				page.push(room);
				break;
			}
		}
	}

	// Collect up to one more room than the batch size to detect whether more remain
	while page.len() <= MAX_BATCH_SIZE {
		match stream.next().await {
			| Some(room) => page.push(room),
			| None => break,
		}
	}

	let has_more = page.len() > MAX_BATCH_SIZE;
	page.truncate(MAX_BATCH_SIZE);

	let next_batch = has_more.then(|| {
		encode_batch_token(
			page.last()
				.expect("page is non-empty when batched")
				.as_str(),
		)
	});

	Ok((total, page, next_batch))
}

/// Encodes a room ID as an opaque batch token.
fn encode_batch_token(room_id: &str) -> String { URL_SAFE_NO_PAD.encode(room_id) }

/// Decodes a batch token into a room ID. Rejects invalid tokens.
fn decode_batch_token(token: &str) -> Result<ruma::OwnedRoomId> {
	let bytes = URL_SAFE_NO_PAD
		.decode(token)
		.map_err(|_| err!(Request(InvalidParam("Invalid batch token."))))?;

	let room_id = std::str::from_utf8(&bytes)
		.ok()
		.and_then(|room_id| RoomId::parse(room_id).ok())
		.ok_or_else(|| err!(Request(InvalidParam("Invalid batch token."))))?;

	Ok(room_id)
}

#[cfg(test)]
mod tests {
	use ruma::room_id;

	use super::*;

	#[test]
	fn batch_token_round_trips() {
		let room_id = room_id!("!test:example.com");
		let token = encode_batch_token(room_id.as_str());
		assert_eq!(decode_batch_token(&token).expect("valid token"), room_id);
	}

	#[test]
	fn batch_token_is_url_safe_and_sized() {
		let token = encode_batch_token("!test:example.com");
		assert!(
			token
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
		);
		assert!(!token.is_empty());
		assert!(token.len() <= 255);
	}

	#[test]
	fn invalid_batch_tokens_are_rejected() {
		for token in ["", "not valid base64!!!", "aGVsbG8gd29ybGQ=", "ZW1wdHk="] {
			assert!(decode_batch_token(token).is_err(), "token `{token}` should be rejected");
		}
	}
}
