//! MSC4391 in-room bot commands: describing the admin command tree, and
//! turning structured invocations into argv.

use std::any::TypeId;

use clap::{ArgAction, Command, CommandFactory, builder::ValueParser};
use conduwuit::{Err, Result, err};
use ruma::{
	OwnedEventId, OwnedRoomAliasId, OwnedRoomId, OwnedRoomOrAliasId, OwnedServerName, OwnedUserId,
};
use serde_json::Value;
use service::admin::{
	CommandDescription, DefaultValue, Invocation, LiteralType, Parameter, Primitive, Schema, Text,
};

use crate::admin::AdminCommand;

/// Each leaf admin command's description content.
#[must_use]
pub(crate) fn descriptions() -> Vec<CommandDescription> {
	let mut described = Vec::new();
	walk(&AdminCommand::command(), &mut Vec::new(), &mut described);
	described
}

fn walk(command: &Command, path: &mut Vec<String>, described: &mut Vec<CommandDescription>) {
	for sub in command.get_subcommands() {
		if sub.is_hide_set() {
			continue;
		}
		path.push(sub.get_name().to_owned());
		if sub.has_subcommands() {
			walk(sub, path, described);
		} else {
			described.push(describe(sub, path));
		}
		path.pop();
	}
}

fn describe(command: &Command, path: &[String]) -> CommandDescription {
	let parent = &path[..path.len().saturating_sub(1)];
	CommandDescription {
		command: path.join(" "),
		parameters: command
			.get_arguments()
			.filter(|arg| !arg.is_hide_set())
			.filter_map(parameter)
			.collect(),
		description: command.get_about().map(Text::new),
		aliases: command
			.get_visible_aliases()
			.map(|alias| [parent, &[alias.to_owned()]].concat().join(" "))
			.collect(),
	}
}

fn primitive(parser: &ValueParser) -> Schema {
	let id = parser.type_id();
	let is = |type_id: TypeId| id == type_id;
	let primitive = |kind| Schema::Primitive { kind };

	if is(TypeId::of::<OwnedRoomOrAliasId>()) {
		return Schema::Union {
			variants: vec![primitive(Primitive::RoomId), primitive(Primitive::RoomAlias)],
		};
	}
	let kind = if is(TypeId::of::<bool>()) {
		Primitive::Boolean
	} else if [
		TypeId::of::<u8>(),
		TypeId::of::<u16>(),
		TypeId::of::<u32>(),
		TypeId::of::<u64>(),
		TypeId::of::<usize>(),
		TypeId::of::<i8>(),
		TypeId::of::<i16>(),
		TypeId::of::<i32>(),
		TypeId::of::<i64>(),
		TypeId::of::<isize>(),
	]
	.into_iter()
	.any(is)
	{
		Primitive::Integer
	} else if is(TypeId::of::<OwnedUserId>()) {
		Primitive::UserId
	} else if is(TypeId::of::<OwnedRoomId>()) {
		Primitive::RoomId
	} else if is(TypeId::of::<OwnedRoomAliasId>()) {
		Primitive::RoomAlias
	} else if is(TypeId::of::<OwnedEventId>()) {
		Primitive::EventId
	} else if is(TypeId::of::<OwnedServerName>()) {
		Primitive::ServerName
	} else {
		Primitive::String
	};
	primitive(kind)
}

fn parameter(arg: &clap::Arg) -> Option<Parameter> {
	let many = arg
		.get_num_args()
		.is_some_and(|range| range.max_values() > 1);
	let schema = match arg.get_action() {
		| ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version =>
			return None,
		| ArgAction::SetTrue | ArgAction::SetFalse =>
			Schema::Primitive { kind: Primitive::Boolean },
		| ArgAction::Count => Schema::Primitive { kind: Primitive::Integer },
		| action => {
			let choices = arg.get_possible_values();
			let item = if choices.is_empty() {
				primitive(arg.get_value_parser())
			} else {
				Schema::Union {
					variants: choices
						.iter()
						.filter(|choice| !choice.is_hide_set())
						.map(|choice| Schema::Literal {
							literal_type: LiteralType::String,
							value: choice.get_name().to_owned(),
						})
						.collect(),
				}
			};
			if matches!(action, ArgAction::Append) || many {
				Schema::Array { items: Box::new(item) }
			} else {
				item
			}
		},
	};

	let default_value = match (arg.get_default_values().first(), &schema) {
		| (Some(default), Schema::Primitive { kind }) if *kind != Primitive::Boolean => {
			let default = default.to_string_lossy();
			Some(match kind {
				| Primitive::Integer => default.parse().map_or_else(
					|_| DefaultValue::String(default.into_owned()),
					DefaultValue::Integer,
				),
				| _ => DefaultValue::String(default.into_owned()),
			})
		},
		| _ => None,
	};

	Some(Parameter {
		key: arg.get_id().as_str().to_owned(),
		schema,
		optional: !arg.is_required_set(),
		description: arg
			.get_long_help()
			.or_else(|| arg.get_help())
			.map(Text::new),
		default_value,
	})
}

/// The argv `AdminCommand::try_parse_from` expects for an
/// `org.matrix.msc4391.command` invocation.
pub(crate) fn argv(invocation: &Value) -> Result<Vec<String>> {
	let root = AdminCommand::command();
	let Invocation { command: name, arguments } = serde_json::from_value(invocation.clone())
		.map_err(|error| err!("The command is malformed: {error}"))?;
	let arguments = arguments.unwrap_or_default();

	let mut argv = vec!["admin".to_owned()];
	let mut command = &root;
	for word in name.split_whitespace() {
		command = command
			.find_subcommand(word)
			.ok_or_else(|| err!("There is no `{name}` command."))?;
		argv.push(command.get_name().to_owned());
	}
	if command.has_subcommands() {
		return Err!("`{name}` needs a subcommand.");
	}

	for key in arguments.keys() {
		if !command
			.get_arguments()
			.filter(|arg| parameter(arg).is_some())
			.any(|arg| arg.get_id() == key)
		{
			return Err!("`{name}` has no `{key}` argument.");
		}
	}

	let mut positionals = Vec::new();
	let mut skipped = None;
	for arg in command.get_arguments() {
		let key = arg.get_id().as_str();
		let value = match arguments.get(key) {
			| None | Some(Value::Null) => {
				if arg.is_positional() {
					skipped.get_or_insert(key);
				}
				continue;
			},
			| Some(value) => value,
		};
		let schema = parameter(arg)
			.expect("described arguments have schemas")
			.schema;
		let (item_schema, is_array) = match &schema {
			| Schema::Array { items } => (&**items, true),
			| schema => (schema, false),
		};
		let values = match value {
			| Value::Array(items) if is_array => items
				.iter()
				.map(|value| scalar(item_schema, value))
				.collect::<Result<Vec<_>>>()?,
			| Value::Array(_) => return Err!("`{key}` does not accept an array."),
			| _ if is_array => return Err!("`{key}` must be an array."),
			| value => vec![scalar(item_schema, value)?],
		};

		if arg.is_positional() {
			if let Some(skipped) = skipped {
				return Err!("`{key}` can only be given together with `{skipped}`.");
			}
			positionals.extend(values);
			continue;
		}

		let flag = arg
			.get_long()
			.map(|long| format!("--{long}"))
			.or_else(|| arg.get_short().map(|short| format!("-{short}")))
			.ok_or_else(|| err!("`{key}` cannot be passed by name."))?;
		match arg.get_action() {
			| ArgAction::SetTrue => match value {
				| Value::Bool(true) => argv.push(flag),
				| Value::Bool(false) => {},
				| _ => return Err!("`{key}` must be a boolean."),
			},
			| ArgAction::SetFalse => match value {
				| Value::Bool(false) => argv.push(flag),
				| Value::Bool(true) => {},
				| _ => return Err!("`{key}` must be a boolean."),
			},
			| ArgAction::Count => {
				let count = value
					.as_u64()
					.ok_or_else(|| err!("`{key}` must be a count."))?;
				argv.extend((0..count).map(|_| flag.clone()));
			},
			| _ => argv.extend(values.into_iter().map(|value| format!("{flag}={value}"))),
		}
	}

	if !positionals.is_empty() {
		argv.push("--".to_owned());
		argv.extend(positionals);
	}
	Ok(argv)
}

fn scalar(schema: &Schema, value: &Value) -> Result<String> {
	match schema {
		| Schema::Union { variants } => variants
			.iter()
			.find_map(|schema| scalar(schema, value).ok())
			.ok_or_else(|| err!("An argument has a value of an unsupported type.")),
		| Schema::Literal { value: literal, .. } if value.as_str() == Some(literal) =>
			Ok(literal.clone()),
		| Schema::Primitive { kind } => match kind {
			| Primitive::Boolean => value
				.as_bool()
				.map(|value| value.to_string())
				.ok_or_else(|| err!("An argument must be a boolean.")),
			| Primitive::Integer => value
				.as_i64()
				.or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
				.map(|value| value.to_string())
				.ok_or_else(|| err!("An argument must be an integer.")),
			| Primitive::RoomId => reference(value, "room_id", &["room_id", "id"]),
			| Primitive::EventId => reference(value, "event_id", &["event_id"]),
			| _ => value
				.as_str()
				.map(ToOwned::to_owned)
				.ok_or_else(|| err!("An argument must be a string.")),
		},
		| _ => Err!("An argument has an invalid schema."),
	}
}

fn reference(value: &Value, kind: &str, fields: &[&str]) -> Result<String> {
	let reference = value
		.as_object()
		.ok_or_else(|| err!("A {kind} reference must be an object."))?;
	if reference.get("type").and_then(Value::as_str) != Some(kind) {
		return Err!("A reference must have type `{kind}`.");
	}
	fields
		.iter()
		.find_map(|field| reference.get(*field).and_then(Value::as_str))
		.map(ToOwned::to_owned)
		.ok_or_else(|| err!("A {kind} reference has no id."))
}

#[cfg(test)]
mod tests {
	use std::collections::HashSet;

	use clap::Parser;
	use serde_json::json;
	use service::admin::{CommandDescription, Primitive, Schema};

	use super::{argv, descriptions};
	use crate::admin::AdminCommand;

	fn find<'a>(described: &'a [CommandDescription], name: &str) -> &'a CommandDescription {
		described
			.iter()
			.find(|description| description.command == name)
			.unwrap()
	}

	#[test]
	fn every_leaf_command_is_described_once_with_unique_parameters() {
		let described = descriptions();
		let names: HashSet<&str> = described
			.iter()
			.map(|description| description.command.as_str())
			.collect();
		assert_eq!(names.len(), described.len());
		assert!(names.contains("users create"));

		for description in &described {
			let keys: HashSet<&str> = description
				.parameters
				.iter()
				.map(|parameter| parameter.key.as_str())
				.collect();
			assert_eq!(
				keys.len(),
				description.parameters.len(),
				"{} repeats a parameter key",
				description.command
			);
		}
	}

	#[test]
	fn arguments_are_typed_from_the_clap_definitions() {
		let described = descriptions();
		let reset = find(&described, "users reset-password");
		let parameter = |key: &str| {
			reset
				.parameters
				.iter()
				.find(|parameter| parameter.key == key)
				.unwrap()
		};

		assert_eq!(parameter("logout").schema, Schema::Primitive { kind: Primitive::Boolean });
		assert!(parameter("logout").optional);
		assert_eq!(parameter("username").schema, Schema::Primitive { kind: Primitive::String });
		assert!(!parameter("username").optional);
		assert!(parameter("password").optional);
	}

	#[test]
	fn descriptions_serialize_to_the_msc4391_wire_format() {
		let described = descriptions();
		let reset = find(&described, "users reset-password");
		let content = serde_json::to_value(reset).unwrap();

		assert_eq!(content["command"], "users reset-password");
		let logout = content["parameters"]
			.as_array()
			.unwrap()
			.iter()
			.find(|parameter| parameter["key"] == "logout")
			.unwrap();
		assert_eq!(logout["schema"], json!({ "schema_type": "primitive", "type": "boolean" }));
		assert_eq!(logout["optional"], true);
		assert!(logout["description"]["m.text"][0]["body"].is_string());

		let username = content["parameters"]
			.as_array()
			.unwrap()
			.iter()
			.find(|parameter| parameter["key"] == "username")
			.unwrap();
		assert!(username.get("optional").is_none());
		assert_eq!(serde_json::from_value::<CommandDescription>(content).unwrap(), *reset);
	}

	#[test]
	fn a_structured_invocation_parses_as_the_typed_command() {
		let argv = argv(&json!({
			"command": "users reset-password",
			"arguments": { "logout": true, "username": "alice", "password": "-secret" },
		}))
		.unwrap();

		assert_eq!(argv, [
			"admin",
			"users",
			"reset-password",
			"--logout",
			"--",
			"alice",
			"-secret"
		]);
		AdminCommand::try_parse_from(&argv).unwrap();
	}

	#[test]
	fn a_room_reference_passes_its_id() {
		let argv = argv(&json!({
			"command": "rooms moderation ban-room",
			"arguments": { "room": { "type": "room_id", "room_id": "!room:example.org", "via": ["example.org"] } },
		}))
		.unwrap();

		assert_eq!(argv.last().map(String::as_str), Some("!room:example.org"));
		AdminCommand::try_parse_from(&argv).unwrap();
	}

	#[test]
	fn unknown_commands_and_arguments_are_refused() {
		argv(&json!({ "command": "users nope" })).unwrap_err();
		argv(&json!({ "command": "users" })).unwrap_err();
		argv(&json!({ "command": "users create", "arguments": { "nope": 1 } })).unwrap_err();
		argv(&json!({ "arguments": {} })).unwrap_err();
		argv(&json!({ "command": "users create", "arguments": [] })).unwrap_err();
	}

	#[test]
	fn structured_arguments_match_their_schemas() {
		argv(&json!({
			"command": "users reset-password",
			"arguments": { "logout": "true", "username": "alice" },
		}))
		.unwrap_err();
		argv(&json!({
			"command": "rooms moderation ban-room",
			"arguments": { "room": "!room:example.org" },
		}))
		.unwrap_err();
	}

	fn sample(schema: &Schema) -> serde_json::Value {
		match schema {
			| Schema::Array { items } => json!([sample(items)]),
			| Schema::Union { variants } => sample(&variants[0]),
			| Schema::Literal { value, .. } => json!(value),
			| Schema::Primitive { kind } => match kind {
				| Primitive::Boolean => json!(true),
				| Primitive::Integer => json!(1),
				| Primitive::UserId => json!("@alice:example.org"),
				| Primitive::RoomId =>
					json!({ "type": "room_id", "room_id": "!room:example.org" }),
				| Primitive::RoomAlias => json!("#room:example.org"),
				| Primitive::EventId =>
					json!({ "type": "event_id", "id": "!room:example.org", "event_id": "$event" }),
				| Primitive::ServerName => json!("example.org"),
				| Primitive::String => json!("mxc://example.org/media"),
			},
		}
	}

	/// Commands whose requirements are "one of these", which MSC4391 cannot
	/// express: every argument is described as optional and clap reports
	/// what is missing.
	const ONE_OF: [&str; 2] = ["token issue", "media delete-url-preview"];

	#[test]
	fn every_described_command_accepts_its_required_arguments() {
		let mut failures = Vec::new();
		for description in descriptions() {
			let name = &description.command;
			if ONE_OF.contains(&name.as_str()) {
				continue;
			}
			let arguments: serde_json::Map<String, serde_json::Value> = description
				.parameters
				.iter()
				.filter(|parameter| !parameter.optional)
				.map(|parameter| (parameter.key.clone(), sample(&parameter.schema)))
				.collect();
			let invocation = json!({ "command": name, "arguments": arguments });
			match argv(&invocation) {
				| Ok(argv) =>
					if let Err(error) = AdminCommand::try_parse_from(&argv) {
						failures.push(format!("{name}: {argv:?}: {error}"));
					},
				| Err(error) => failures.push(format!("{name}: {error}")),
			}
		}
		assert!(failures.is_empty(), "{}", failures.join("\n"));
	}
}
