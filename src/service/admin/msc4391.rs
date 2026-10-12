//! MSC4391 in-room bot commands for the admin room.

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use conduwuit::{Event, Result, debug, err, info, pdu::PartialPdu};
use futures::FutureExt;
use ruma::{
	OwnedEventId, UserId,
	events::{Mentions, StateEventType},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, value::to_raw_value};
use sha2::{Digest, Sha256};

use super::{CommandInput, InvocationSource, Service};

const COMMAND_DESCRIPTION_EVENT: &str = "org.matrix.msc4391.command_description";

/// Prototype of the command describer, supplied by the reloadable admin
/// module: each admin command's description event content.
pub type Describer = fn() -> Vec<CommandDescription>;

/// Content of an `org.matrix.msc4391.command_description` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDescription {
	/// Space-separated command name, e.g. `users reset-password`.
	pub command: String,
	pub parameters: Vec<Parameter>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<Text>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	/// Other full names the command answers to.
	pub aliases: Vec<String>,
}

/// One argument of a command: its key, type, and whether it may be omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
	pub key: String,
	pub schema: Schema,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	/// Absent from the wire when false, since MSC4391 defaults to required.
	pub optional: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<Text>,
	#[serde(rename = "fi.mau.default_value", default, skip_serializing_if = "Option::is_none")]
	/// What the command uses when the argument is omitted; informational.
	pub default_value: Option<DefaultValue>,
}

/// Extensible-events text, plain body only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
	#[serde(rename = "m.text")]
	pub parts: Vec<TextPart>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPart {
	pub body: String,
}

impl Text {
	#[must_use]
	pub fn new(body: &impl ToString) -> Self {
		Self {
			parts: vec![TextPart { body: body.to_string() }],
		}
	}
}

/// The type of a parameter, as an MSC4391 `schema_type`-tagged object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schema_type", rename_all = "snake_case")]
pub enum Schema {
	Primitive {
		#[serde(rename = "type")]
		kind: Primitive,
	},
	/// A room ID or alias, or a fixed set of choices.
	Union {
		variants: Vec<Self>,
	},
	Literal {
		literal_type: LiteralType,
		value: String,
	},
	Array {
		items: Box<Self>,
	},
}

/// Primitive types MSC4391 names. Matrix identifiers are sent as references
/// (`room_id`, `event_id`) or plain strings; see `scalar` in the admin module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
	Boolean,
	Integer,
	String,
	UserId,
	RoomId,
	RoomAlias,
	EventId,
	ServerName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralType {
	String,
}

/// The `fi.mau.default_value` extension: an integer where the argument is one,
/// otherwise the default as clap spells it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DefaultValue {
	Integer(i64),
	String(String),
}

/// Content of an `org.matrix.msc4391.command` invocation. The arguments are
/// untrusted and are checked against the command's schemas.
#[derive(Debug, Deserialize)]
pub struct Invocation {
	pub command: String,
	/// Argument values by parameter key; absent or `null` means none.
	#[serde(default)]
	pub arguments: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
struct CommandContent {
	body: Option<String>,
	#[serde(rename = "org.matrix.msc4391.command")]
	command: Option<Value>,
	#[serde(rename = "m.mentions")]
	mentions: Option<Mentions>,
}

/// Padded base64 of SHA-256(command + sender), as MSC4391 requires.
fn state_key(command: &str, sender: &UserId) -> String {
	let mut hasher = Sha256::new();
	hasher.update(command.as_bytes());
	hasher.update(sender.as_bytes());
	STANDARD.encode(hasher.finalize())
}

impl Service {
	/// Syncs the admin room's command description state with the admin
	/// command tree, sending only what changed.
	pub async fn publish_command_descriptions(&self) -> Result {
		let Some(describe) = *self.describe.read() else {
			return Ok(());
		};
		let Ok(room_id) = self.get_admin_room().await else {
			return Ok(());
		};
		let server_user = &self.services.globals.server_user;
		let event_type = StateEventType::from(COMMAND_DESCRIPTION_EVENT);

		let wanted: BTreeMap<String, CommandDescription> = describe()
			.into_iter()
			.map(|content| (state_key(&content.command, server_user), content))
			.collect();
		let state_lock = self.services.state.mutex.lock(&room_id).await;
		let mut sent = 0_usize;

		for (key, content) in &wanted {
			let current: Option<CommandDescription> = self
				.services
				.state_accessor
				.room_state_get_content(&room_id, &event_type, key.as_str())
				.await
				.ok();
			if current.as_ref() == Some(content) {
				continue;
			}

			self.services
				.timeline
				.build_and_append_pdu(
					PartialPdu {
						event_type: COMMAND_DESCRIPTION_EVENT.into(),
						content: to_raw_value(content)?,
						state_key: Some(key.clone().into()),
						..PartialPdu::default()
					},
					server_user,
					Some(&room_id),
					&state_lock,
				)
				.boxed()
				.await?;
			sent = sent.saturating_add(1);
		}

		if sent > 0 {
			info!(count = sent, "Updated admin command descriptions");
		} else {
			debug!("Admin command descriptions are up to date");
		}
		Ok(())
	}

	/// Queues the admin command in a room message, either `!admin …` text or
	/// an MSC4391 invocation addressed to the server user.
	pub async fn handle_room_message<E>(&self, event: &E, sent_locally: bool) -> Result
	where
		E: Event + Send + Sync,
	{
		let content: CommandContent = event.get_content()?;
		let reply_id: Option<OwnedEventId> = Some(event.event_id().into());

		if let Some(invocation) = content.command
			&& self
				.is_structured_admin_command(event, content.mentions.as_ref())
				.await
		{
			return self
				.channel
				.0
				.send(CommandInput {
					command: content.body.unwrap_or_default(),
					structured: Some(invocation),
					reply_id,
					source: InvocationSource::AdminRoom,
					sender: Some(event.sender().to_owned()),
				})
				.map_err(|e| err!("Failed to enqueue admin command: {e:?}"));
		}

		let Some(body) = content.body else {
			return Ok(());
		};
		if let Some(source) = self.is_admin_command(event, &body, sent_locally).await {
			self.command_with_sender(body, reply_id, source, event.sender().to_owned())?;
		}
		Ok(())
	}

	async fn is_structured_admin_command<E>(&self, event: &E, mentions: Option<&Mentions>) -> bool
	where
		E: Event + Send + Sync,
	{
		let server_user = &self.services.globals.server_user;
		if !mentions.is_some_and(|mentions| mentions.user_ids.contains(server_user)) {
			return false;
		}
		if event.sender() == server_user
			&& self.services.server.config.emergency_password.is_none()
		{
			return false;
		}
		let Some(room_id) = event.room_id() else {
			return false;
		};
		self.is_admin_room(room_id).await && self.user_is_admin(event.sender()).await
	}
}
