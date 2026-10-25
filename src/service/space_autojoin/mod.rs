//! # Space auto-join
//!
//! Makes spaces behave like Discord servers: joining a space joins its
//! channels.
//!
//! With `auto_join_space_children = true`:
//! - a local user who joins a space is joined to every child room they are
//!   allowed to join (rooms they can't join, e.g. invite-only ones, are
//!   skipped);
//! - when a child room is added to a space, every local member of the space is
//!   joined to it;
//! - a local user who leaves (or is removed from) a space leaves its child
//!   rooms.
//!
//! Sub-spaces (categories) work through the same rules: joining a space joins
//! the sub-space, which in turn joins that sub-space's channels.
//!
//! Only changes *into* or *out of* the joined state count, so a profile
//! update in a space doesn't re-join channels the user left.
//!
//! The work runs in the background after the triggering event is stored.

use std::sync::Arc;

use conduwuit::{
	Event, Result, Server, debug, debug_warn,
	matrix::{PduEvent, pdu::PartialPdu},
};
use futures::StreamExt;
use ruma::{
	OwnedRoomId, OwnedUserId, RoomId, UserId,
	events::{
		StateEventType, TimelineEventType,
		room::member::{MembershipState, RoomMemberEventContent},
		space::child::SpaceChildEventContent,
	},
	room::RoomType,
};
use serde_json::Value as JsonValue;

use crate::{Dep, globals, rooms};

pub struct Service {
	services: Services,
}

struct Services {
	server: Arc<Server>,
	globals: Dep<globals::Service>,
	membership: Dep<rooms::membership::Service>,
	state: Dep<rooms::state::Service>,
	state_accessor: Dep<rooms::state_accessor::Service>,
	state_cache: Dep<rooms::state_cache::Service>,
	timeline: Dep<rooms::timeline::Service>,
}

#[derive(Debug)]
enum Action {
	/// A local user newly joined the space.
	Joined(OwnedUserId),
	/// A local user left or was removed from the space.
	Left(OwnedUserId),
	/// A child room was added to the space.
	ChildAdded(OwnedRoomId),
}

impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			services: Services {
				server: args.server.clone(),
				globals: args.depend::<globals::Service>("globals"),
				membership: args.depend::<rooms::membership::Service>("rooms::membership"),
				state: args.depend::<rooms::state::Service>("rooms::state"),
				state_accessor: args
					.depend::<rooms::state_accessor::Service>("rooms::state_accessor"),
				state_cache: args.depend::<rooms::state_cache::Service>("rooms::state_cache"),
				timeline: args.depend::<rooms::timeline::Service>("rooms::timeline"),
			},
		}))
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	#[inline]
	#[must_use]
	pub fn enabled(&self) -> bool { self.services.server.config.auto_join_space_children }

	/// Called after a PDU has been appended to `room_id`. Returns at once when
	/// the event is irrelevant; otherwise schedules the work in the
	/// background.
	pub fn on_appended(self: &Arc<Self>, pdu: &PduEvent, room_id: &RoomId) {
		if !self.enabled() {
			return;
		}

		let Some(action) = self.classify(pdu) else {
			return;
		};

		let this = Arc::clone(self);
		let room_id = room_id.to_owned();
		self.services.server.runtime().spawn(async move {
			// Only spaces matter; checked here, off the append path.
			if !this.is_space(&room_id).await {
				return;
			}
			debug!(%room_id, ?action, "space auto-join");
			this.apply(&room_id, action).await;
		});
	}

	fn classify(&self, pdu: &PduEvent) -> Option<Action> {
		let state_key = pdu.state_key()?;
		match pdu.kind() {
			| TimelineEventType::RoomMember => {
				let user = UserId::parse(state_key).ok()?;
				if !self.services.globals.user_is_local(&user) {
					return None;
				}
				let now = pdu.get_content::<RoomMemberEventContent>().ok()?.membership;
				let before = pdu
					.get_unsigned_property::<JsonValue>("prev_content")
					.ok()
					.and_then(|prev| prev.get("membership").and_then(|m| m.as_str().map(str::to_owned)));
				let was_joined = before.as_deref() == Some("join");

				match now {
					| MembershipState::Join if !was_joined => Some(Action::Joined(user)),
					| MembershipState::Leave | MembershipState::Ban if was_joined =>
						Some(Action::Left(user)),
					| _ => None,
				}
			},
			| TimelineEventType::SpaceChild => {
				let child = RoomId::parse(state_key).ok()?;
				let content = pdu.get_content::<SpaceChildEventContent>().ok()?;
				// An empty `via` means the child was removed.
				(!content.via.is_empty()).then(|| Action::ChildAdded(child))
			},
			| _ => None,
		}
	}

	async fn apply(&self, space: &RoomId, action: Action) {
		match action {
			| Action::Joined(user) =>
				for child in self.children(space).await {
					self.join(&user, &child).await;
				},
			| Action::Left(user) =>
				for child in self.children(space).await {
					self.leave(&user, &child).await;
				},
			| Action::ChildAdded(child) => {
				let members: Vec<OwnedUserId> = self
					.services
					.state_cache
					.active_local_users_in_room(space)
					.collect()
					.await;
				for user in members {
					self.join(&user, &child).await;
				}
			},
		}
	}

	async fn is_space(&self, room_id: &RoomId) -> bool {
		matches!(
			self.services.state_accessor.get_room_type(room_id).await,
			Ok(RoomType::Space)
		)
	}

	/// Child rooms of a space (`m.space.child` events with a non-empty `via`).
	async fn children(&self, space: &RoomId) -> Vec<OwnedRoomId> {
		let Ok(shortstatehash) = self.services.state.get_room_shortstatehash(space).await else {
			return Vec::new();
		};

		let keys: Vec<(String, ruma::OwnedEventId)> = self
			.services
			.state_accessor
			.state_keys_with_ids(shortstatehash, &StateEventType::SpaceChild)
			.map(|(key, id): (conduwuit::matrix::StateKey, ruma::OwnedEventId)| (key.to_string(), id))
			.collect()
			.await;

		let mut children = Vec::new();
		for (key, event_id) in keys {
			let Ok(child) = RoomId::parse(&key) else {
				continue;
			};
			let Ok(pdu) = self.services.timeline.get_pdu(&event_id).await else {
				continue;
			};
			if pdu
				.get_content::<SpaceChildEventContent>()
				.is_ok_and(|c| !c.via.is_empty())
			{
				children.push(child);
			}
		}
		children
	}

	async fn join(&self, user: &UserId, room: &RoomId) {
		if self.services.state_cache.is_joined(user, room).await {
			return;
		}
		// Only rooms this server already takes part in (no federation).
		if !self
			.services
			.state_cache
			.server_in_room(self.services.globals.server_name(), room)
			.await
		{
			return;
		}
		if let Err(e) = self
			.services
			.membership
			.join_room(user, room, None, &[])
			.await
		{
			// Expected for rooms the user isn't allowed into.
			debug!(%user, %room, "space auto-join skipped: {e}");
		}
	}

	async fn leave(&self, user: &UserId, room: &RoomId) {
		if !self.services.state_cache.is_joined(user, room).await {
			return;
		}

		let state_lock = self.services.state.mutex.lock(room).await;
		let Ok(mut content) = self
			.services
			.state_accessor
			.room_state_get_content::<RoomMemberEventContent>(
				room,
				&StateEventType::RoomMember,
				user.as_str(),
			)
			.await
		else {
			return;
		};
		content.membership = MembershipState::Leave;
		content.reason = None;
		content.join_authorized_via_users_server = None;
		content.is_direct = None;

		if let Err(e) = self
			.services
			.timeline
			.build_and_append_pdu(
				PartialPdu::state(user.to_string(), &content),
				user,
				Some(room),
				&state_lock,
			)
			.await
		{
			debug_warn!(%user, %room, "space auto-leave failed: {e}");
		}
	}
}
