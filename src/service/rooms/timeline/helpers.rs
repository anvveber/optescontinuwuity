//! Helpers for submitting events with the right checks performed

pub(crate) use conduwuit::{
	Err, Event, Result, err,
	matrix::pdu::{PartialPdu, UnsignedContext},
	trace,
};
use ruma::{
	OwnedEventId, RoomId, UserId,
	events::{
		AnyTimelineEvent, MessageLikeEventType, StateEventType,
		room::{
			canonical_alias::RoomCanonicalAliasEventContent,
			history_visibility::{HistoryVisibility, RoomHistoryVisibilityEventContent},
			join_rules::{JoinRule, RoomJoinRulesEventContent},
			member::{MembershipState, RoomMemberEventContent},
			server_acl::RoomServerAclEventContent,
		},
	},
	serde::Raw,
};
use serde_json::{from_str, value::RawValue};

use crate::rooms::state::RoomMutexGuard;

impl super::Service {
	pub async fn send_timeline_event(
		&self,
		sender: &UserId,
		room_id: &RoomId,
		state_lock: &RoomMutexGuard,
		mut partial_pdu: PartialPdu,
	) -> Result<OwnedEventId> {
		match &partial_pdu.state_key {
			| Some(state_key) => {
				let modified_content = self
					.assert_allowed_to_send_state_event(
						room_id,
						&partial_pdu.event_type.to_string().into(),
						state_key,
						&partial_pdu.content,
					)
					.await?;

				if let Some(content) = modified_content {
					partial_pdu.content = content;
				}
			},
			| None => {
				self.allowed_to_send_message_event(
					room_id,
					&partial_pdu.event_type.to_string().into(),
				)
				.await?;
			},
		}

		if !self.services.config.allow_sticky_events {
			partial_pdu.sticky = None;
		}

		let event_id = self
			.build_and_append_pdu(partial_pdu, sender, Some(room_id), state_lock)
			.await?;

		Ok(event_id)
	}

	async fn allowed_to_send_message_event(
		&self,
		room_id: &RoomId,
		event_type: &MessageLikeEventType,
	) -> Result {
		if *event_type == MessageLikeEventType::CallInvite
			&& self.services.directory.is_public_room(room_id).await
		{
			return Err!(Request(Forbidden("Room call invites are not allowed in public rooms")));
		}

		// Forbid m.room.encrypted if encryption is disabled
		if MessageLikeEventType::RoomEncrypted == *event_type
			&& !self.services.config.allow_encryption
		{
			return Err!(Request(Forbidden("Encryption has been disabled")));
		}

		Ok(())
	}

	async fn assert_allowed_to_send_state_event(
		&self,
		room_id: &RoomId,
		event_type: &StateEventType,
		state_key: &str,
		json: &RawValue,
	) -> Result<Option<Box<RawValue>>> {
		match event_type {
			| StateEventType::RoomCreate => {
				return Err!(Request(BadJson(debug_warn!(
					%room_id,
					"You cannot update m.room.create after a room has been created."
				))));
			},
			| StateEventType::RoomServerAcl =>
				self.assert_allowed_to_send_room_server_acl_event(room_id, json)?,
			| StateEventType::RoomEncryption =>
			// Forbid m.room.encryption if encryption is disabled
				if !self.services.config.allow_encryption {
					return Err!(Request(Forbidden(
						"Encryption is disabled on this homeserver."
					)));
				},
			| StateEventType::RoomJoinRules =>
				self.assert_allowed_to_send_room_join_rules_event(room_id, json)
					.await?,
			| StateEventType::RoomHistoryVisibility =>
				self.assert_allowed_to_send_room_history_visibility_event(room_id, json)
					.await?,
			| StateEventType::RoomCanonicalAlias =>
				self.assert_allowed_to_send_room_canonical_alias_event(room_id, json)
					.await?,
			| StateEventType::RoomMember => {
				return self
					.assert_allowed_to_send_room_member_event(room_id, state_key, json)
					.await;
			},
			| _ => (),
		}

		Ok(None)
	}

	fn assert_allowed_to_send_room_server_acl_event(
		&self,
		room_id: &RoomId,
		json: &RawValue,
	) -> Result {
		// prevents common ACL paw-guns as ACL management is difficult and prone
		// to irreversible mistakes

		let acl_content: RoomServerAclEventContent = from_str(json.get()).map_err(|e| {
			err!(Request(BadJson(debug_warn!("Room server ACL event is invalid: {e}"))))
		})?;

		let allow_has_wildcard = acl_content.allow.iter().any(|entry| entry == "*");
		let deny_has_wildcard = acl_content.deny.iter().any(|entry| entry == "*");
		let allow_has_server = acl_content
			.allow
			.iter()
			.any(|entry| entry == self.services.globals.server_name().as_str());

		if acl_content.allow.is_empty() {
			return Err!(Request(BadJson(debug_warn!(
				%room_id,
				"Sending an ACL event with an empty allow key will permanently \
				brick the room for non-conduwuit's as this equates to no servers \
				being allowed to participate in this room."
			))));
		}

		if allow_has_wildcard && deny_has_wildcard {
			return Err!(Request(BadJson(debug_warn!(
				%room_id,
				"Sending an ACL event with a deny and allow key value of \"*\" will \
				permanently brick the room for non-conduwuit's as this equates to \
				no servers being allowed to participate in this room."
			))));
		}

		if deny_has_wildcard
			&& !acl_content.is_allowed(self.services.globals.server_name())
			&& !allow_has_server
		{
			return Err!(Request(BadJson(debug_warn!(
				%room_id,
				"Sending an ACL event with a deny key value of \"*\" and without \
				your own server name in the allow key will result in you being \
				unable to participate in this room."
			))));
		}

		if !allow_has_wildcard
			&& !acl_content.is_allowed(self.services.globals.server_name())
			&& !allow_has_server
		{
			return Err!(Request(BadJson(debug_warn!(
				%room_id,
				"Sending an ACL event for an allow key without \"*\" and without \
				your own server name in the allow key will result in you being \
				unable to participate in this room."
			))));
		}
		Ok(())
	}

	async fn assert_allowed_to_send_room_join_rules_event(
		&self,
		room_id: &RoomId,
		json: &RawValue,
	) -> Result {
		// admin room is a sensitive room, it should not ever be made public
		if let Ok(admin_room_id) = self.services.admin.get_admin_room().await
			&& admin_room_id == room_id
		{
			let join_rule: RoomJoinRulesEventContent = from_str(json.get()).map_err(|e| {
				err!(Request(BadJson(debug_warn!("Room join rules event is invalid: {e}"))))
			})?;

			if join_rule.join_rule == JoinRule::Public {
				return Err!(Request(Forbidden(
					"Admin room is a sensitive room, it cannot be made public"
				)));
			}
		}

		Ok(())
	}

	async fn assert_allowed_to_send_room_history_visibility_event(
		&self,
		room_id: &RoomId,
		json: &RawValue,
	) -> Result {
		// admin room is a sensitive room, it should not ever be made world
		// readable

		if let Ok(admin_room_id) = self.services.admin.get_admin_room().await
			&& admin_room_id == room_id
		{
			let visibility_content: RoomHistoryVisibilityEventContent = from_str(json.get())
				.map_err(|e| {
					err!(Request(BadJson(debug_warn!(
						"Room history visibility event is invalid: {e}"
					))))
				})?;

			if visibility_content.history_visibility == HistoryVisibility::WorldReadable {
				return Err!(Request(Forbidden(
					"Admin room is a sensitive room, it cannot be made public"
				)));
			}
		}

		Ok(())
	}

	async fn assert_allowed_to_send_room_canonical_alias_event(
		&self,
		room_id: &RoomId,
		json: &RawValue,
	) -> Result {
		let canonical_alias_content: RoomCanonicalAliasEventContent = from_str(json.get())
			.map_err(|e| {
				err!(Request(BadJson(debug_warn!("Room canonical alias event is invalid: {e}"))))
			})?;

		let mut aliases = canonical_alias_content.alt_aliases.clone();

		if let Some(alias) = canonical_alias_content.alias {
			aliases.push(alias);
		}

		for alias in aliases {
			let (alias_room_id, _) = self
				.services
				.alias
				.resolve_alias(&alias)
				.await
				.map_err(|e| err!(Request(Unknown("Failed resolving alias \"{alias}\": {e}"))))?;

			if alias_room_id != room_id {
				return Err!(Request(BadAlias(
					"Room alias {alias} does not belong to room {room_id}"
				)));
			}
		}

		Ok(())
	}

	async fn assert_allowed_to_send_room_member_event(
		&self,
		room_id: &RoomId,
		state_key: &str,
		json: &RawValue,
	) -> Result<Option<Box<RawValue>>> {
		let mut membership_content: RoomMemberEventContent =
			from_str(json.get()).map_err(|e| {
				err!(Request(BadJson(debug_warn!("Room member event is invalid: {e}"))))
			})?;

		let Ok(state_key) = UserId::parse(state_key) else {
			return Err!(Request(BadJson(
				"Membership event has invalid or non-existent state key"
			)));
		};

		let Some(authorising_user) = membership_content.join_authorized_via_users_server else {
			return Ok(None);
		};

		// join_authorized_via_users_server must be thrown away, if user is
		// already a member of the room.
		if self
			.services
			.state_cache
			.is_joined(&state_key, room_id)
			.await
		{
			membership_content.join_authorized_via_users_server = None;
			return Ok(Some(
				RawValue::from_string(serde_json::to_string(&membership_content)?)
					.expect("json has to be valid we just built it"),
			));
		}

		if membership_content.membership != MembershipState::Join {
			return Err!(Request(BadJson(
				"join_authorised_via_users_server is only for member joins"
			)));
		}

		if !self.services.globals.user_is_local(&authorising_user) {
			return Err!(Request(InvalidParam(
				"Authorising user {authorising_user} does not belong to this homeserver"
			)));
		}

		if !self
			.services
			.state_cache
			.is_joined(&authorising_user, room_id)
			.await
		{
			return Err!(Request(InvalidParam(
				"Authorising user {authorising_user} is not in the room, they cannot authorise \
				 the join."
			)));
		}

		Ok(None)
	}

	/// Fetches contextual unsigned information, like prev content, membership,
	/// and redacted because.
	#[tracing::instrument(skip_all, fields(event_id=%event.event_id(), ?user_id))]
	pub async fn get_unsigned_context<'a, E>(
		&self,
		event: E,
		user_id: Option<&'a UserId>,
	) -> UnsignedContext<'a>
	where
		E: Event,
	{
		let ssh = self
			.services
			.state_accessor
			.pdu_shortstatehash(event.event_id())
			.await;
		let (membership, prev_content) = if let Some(sender_user) = user_id
			&& let Ok(ssh) = ssh
		{
			trace!("Fetching membership");
			let membership = Some(
				self.services
					.state_accessor
					.user_membership(ssh, sender_user)
					.await,
			);
			let prev_content = match event.get_unsigned_property::<OwnedEventId>("replaces_state")
			{
				| Ok(event_id) => {
					// Only include prev content if the sender is allowed to see
					// the event
					let can_see = self
						.services
						.state_accessor
						.user_can_see_event(sender_user, &event.room_id_or_hash(), &event_id)
						.await;
					if can_see {
						trace!(prev_event_id=%event_id, "Fetching prev content");
						self.get_pdu(&event_id).await.map(|p| p.content).ok()
					} else {
						trace!(
							prev_event_id=%event_id,
							"Not fetching prev content as sender cannot see the event"
						);
						None
					}
				},
				| _ => None,
			};

			(membership, prev_content)
		} else {
			trace!(?ssh, "No shortstatehash, cannot fetch prev content or membership");
			(None, None)
		};

		// N.B. Theoretically, the redaction event should be included in a
		// redacted capacity if the requesting user isn't allowed to see the
		// redaction event. However, the cost of looking up that information
		// is extraordinary compared to the value it would provide, so is
		// deliberately not done.
		let redacted_because = match event
			.get_unsigned_property::<OwnedEventId>("org.continuwuity.redacted_by")
		{
			| Ok(event_id) => self
				.get_pdu(&event_id)
				.await
				.map(|mut e| {
					// We don't include unsigned here since the other option is
					// to recursively generate unsigned data, which is
					// expensive.
					// Synapse seems to do that, but I don't see a use for it.
					e.unsigned = None;
					e
				})
				.map(Event::into_format::<Raw<AnyTimelineEvent>>)
				.ok()
				.inspect(|_| trace!(redacted_because_id=%event_id, "Fetched redacted_because")),
			| _ => None,
		};
		UnsignedContext {
			user_id,
			membership,
			prev_content,
			redacted_because,
		}
	}
}
