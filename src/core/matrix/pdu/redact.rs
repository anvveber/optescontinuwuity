use ruma::{RoomVersionId, canonical_json::redact_content_in_place};
use serde::Deserialize;
use serde_json::{json, value::to_raw_value};

use crate::{Err, Result, err};

impl super::Pdu {
	pub fn redact(
		&mut self,
		room_version_id: &RoomVersionId,
		redacted_because: Option<&ruma::EventId>,
	) -> Result {
		let Some(rules) = room_version_id.rules() else {
			return Err!("Cannot redact event for unknown room version {room_version_id}");
		};

		self.unsigned = None;

		// a redacted sticky event is just a normal event (MSC4354)
		self.sticky = None;

		let mut content = serde_json::from_str(self.content.get()).map_err(|e| {
			err!(Request(BadJson("Failed to deserialize content into type: {e}")))
		})?;

		redact_content_in_place(&mut content, &rules.redaction, self.kind.to_string());

		if let Some(event_id) = redacted_because {
			self.unsigned = to_raw_value(&json!({
				"org.continuwuity.redacted_by": event_id,
			}))
			.expect("Failed to serialize unsigned")
			.into();
		}

		self.content = to_raw_value(&content).expect("Failed to serialize content");

		Ok(())
	}

	/// Records the redaction without discarding the content.
	pub fn mark_redacted(&mut self, reason: &JsonValue) {
		let redacted_because = json!({
			"redacted_because": reason,
		});

		self.unsigned = to_raw_value(&redacted_because)
			.expect("Failed to serialize unsigned")
			.into();
	}

	#[must_use]
	pub fn recorded_redaction(&self) -> Option<JsonValue> {
		let unsigned = self.unsigned.as_deref()?;
		if !unsigned.get().contains("redacted_because") {
			return None;
		}

		serde_json::from_str::<ExtractRedactedBecause>(unsigned.get())
			.ok()?
			.redacted_because
	}

	pub fn apply_recorded_redaction(&mut self, room_version_id: &RoomVersionId) -> Result {
		let Some(reason) = self.recorded_redaction() else {
			return Ok(());
		};

		self.redact(room_version_id, reason)
	}
}

#[derive(Deserialize)]
struct ExtractRedactedBecause {
	redacted_because: Option<JsonValue>,
}
