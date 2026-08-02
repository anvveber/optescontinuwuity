pub mod livekit {
	pub mod get_token {
		use ruma::{
			OwnedUserId,
			api::{federation::authentication::ServerSignatures, request, response},
			metadata,
		};
		use serde::{Deserialize, Serialize};

		metadata! {
			method: POST,
			rate_limited: false,
			authentication: ServerSignatures,
			history: {
				1.0 => "/_matrix/federation/v1/rtc/livekit/get_token",
			}
		}

		#[derive(Clone, Debug, Deserialize, Serialize)]
		pub struct Member {
			pub id: String,
			pub claimed_device_id: String,
		}

		#[request]
		pub struct Request {
			pub user_id: OwnedUserId,
			pub room_id: String,
			pub slot_id: String,
			pub member: Member,
		}

		#[response]
		pub struct Response {
			pub url: String,
			pub jwt: String,
		}

		impl Request {
			#[must_use]
			pub fn new(
				user_id: OwnedUserId,
				room_id: String,
				slot_id: String,
				member: Member,
			) -> Self {
				Self { user_id, room_id, slot_id, member }
			}
		}

		impl Response {
			#[must_use]
			pub fn new(url: String, jwt: String) -> Self { Self { url, jwt } }
		}
	}
}
