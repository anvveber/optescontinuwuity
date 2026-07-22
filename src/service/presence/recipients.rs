use std::collections::HashSet;

use conduwuit::{Error, Result};
use ruma::{
	OwnedUserId,
	api::federation::{
		query::get_presence_recipients, transactions::edu::PresenceRecipientListUpdates,
	},
};
use serde::{Deserialize, Serialize};

/// Represents a user's presence recipients list.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PresenceRecipients {
	pub stream_id: i64,
	pub recipient_users: HashSet<OwnedUserId>,
}

impl From<get_presence_recipients::msc4495::Response> for PresenceRecipients {
	fn from(response: get_presence_recipients::msc4495::Response) -> Self {
		Self {
			stream_id: response.stream_id.into(),
			recipient_users: response.recipients.into_iter().collect(),
		}
	}
}

impl Default for PresenceRecipients {
	fn default() -> Self {
		Self {
			stream_id: i32::MIN.into(),
			recipient_users: HashSet::new(),
		}
	}
}

impl PresenceRecipients {
	#[must_use]
	pub fn new(stream_id: i64, recipient_users: HashSet<OwnedUserId>) -> Self {
		Self { stream_id, recipient_users }
	}

	pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
		serde_json::from_slice(bytes)
			.map_err(|_| Error::bad_database("Invalid presence data in database"))
	}

	/// Get the changes between this PresenceRecipients and a new recipients set
	#[must_use]
	pub fn delta_to(
		&self,
		new_recipients: &HashSet<OwnedUserId>,
	) -> PresenceRecipientListUpdates {
		PresenceRecipientListUpdates::new(
			new_recipients
				.difference(&self.recipient_users)
				.cloned()
				.collect(),
			self.recipient_users
				.difference(new_recipients)
				.cloned()
				.collect(),
		)
	}

	/// Apply a [`PresenceRecipientListUpdates`] delta to this presence list,
	/// constructing a new recipients entry.
	pub fn apply_delta(&mut self, new_stream_id: i64, delta: PresenceRecipientListUpdates) {
		self.recipient_users.extend(delta.add);
		self.recipient_users.retain(|x| !delta.delete.contains(x));
		self.stream_id = new_stream_id;
	}
}
