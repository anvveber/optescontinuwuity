mod data;
mod presence;
mod recipients;

use std::{borrow::ToOwned, collections::HashSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use conduwuit::{
	Error, Result, Server, checked, debug, debug_warn, error, result::LogErr, trace, warn,
};
use database::Database;
use futures::{Stream, StreamExt, TryFutureExt, stream::FuturesUnordered};
use itertools::Itertools;
use loole::{Receiver, Sender};
pub use recipients::PresenceRecipients;
use ruma::{
OwnedUserId, UInt, UserId,
	api::federation::transactions::edu::{PresenceRecipientListUpdates, PresenceUpdate},
	events::{
		GlobalAccountDataEventType, StateEventType,
		presence::{
			PresenceEvent,
			sharing::{
				PresenceSharingEvent, RoomPresenceSharingState, ServerPresenceSharingState,
				UserPresenceSharingState,
			},
		},
		room::presence_sharing::{PresenceSharingHint, RoomPresenceSharingEventContent},
	},
	presence::PresenceState,
	uint,
};
use tokio::time::sleep;

use self::{data::Data, presence::Presence};
use crate::{
	Dep, account_data, config, globals, rooms,
	sending::{self, EduBuf},
	users,
};

pub struct Service {
	timer_channel: (Sender<TimerType>, Receiver<TimerType>),
	timeout_remote_users: bool,
	idle_timeout: u64,
	offline_timeout: u64,
	db: Data,
	services: Services,
}

struct Services {
	server: Arc<Server>,
	db: Arc<Database>,
	globals: Dep<globals::Service>,
	users: Dep<users::Service>,
	account_data: Dep<account_data::Service>,
	sending: Dep<sending::Service>,
	state_accessor: Dep<rooms::state_accessor::Service>,
	state_cache: Dep<rooms::state_cache::Service>,
	config: Dep<config::Service>,
}

type TimerType = (OwnedUserId, Duration);

#[async_trait]
impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		let config = &args.server.config;
		let idle_timeout_s = config.presence_idle_timeout_s;
		let offline_timeout_s = config.presence_offline_timeout_s;
		Ok(Arc::new(Self {
			timer_channel: loole::unbounded(),
			timeout_remote_users: config.presence_timeout_remote_users,
			idle_timeout: checked!(idle_timeout_s * 1_000)?,
			offline_timeout: checked!(offline_timeout_s * 1_000)?,
			db: Data::new(&args),
			services: Services {
				server: args.server.clone(),
				db: args.db.clone(),
				globals: args.depend::<globals::Service>("globals"),
				users: args.depend::<users::Service>("users"),
				account_data: args.depend::<account_data::Service>("account_data"),
				sending: args.depend::<sending::Service>("sending"),
				state_accessor: args
					.depend::<rooms::state_accessor::Service>("rooms::state_accessor"),
				state_cache: args.depend::<rooms::state_cache::Service>("rooms::state_cache"),
				config: args.depend::<config::Service>("config"),
			},
		}))
	}

	async fn worker(self: Arc<Self>) -> Result<()> {
		let receiver = self.timer_channel.1.clone();

		let mut presence_timers = FuturesUnordered::new();
		while !receiver.is_closed() {
			tokio::select! {
				Some(user_id) = presence_timers.next() => {
					self.process_presence_timer(&user_id).await.log_err().ok();
				},
				event = receiver.recv_async() => match event {
					Err(_) => break,
					Ok((user_id, timeout)) => {
						debug!("Adding timer {}: {user_id} timeout:{timeout:?}", presence_timers.len());
						presence_timers.push(presence_timer(user_id, timeout));
					},
				},
			}
		}

		Ok(())
	}

	fn interrupt(&self) {
		let (timer_sender, _) = &self.timer_channel;
		if !timer_sender.is_closed() {
			timer_sender.close();
		}
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	/// Returns the latest presence event for the given user.
	#[inline]
	pub async fn get_presence(&self, user_id: &UserId) -> Result<PresenceEvent> {
		self.db
			.get_presence(user_id)
			.map_ok(|(_, presence)| presence)
			.await
	}

	/// Pings the presence of the given user in the given room, setting the
	/// specified state.
	pub async fn ping_presence(&self, user_id: &UserId, new_state: &PresenceState) -> Result<()> {
		const REFRESH_TIMEOUT: u64 = 60 * 1000;

		let last_presence = self.db.get_presence(user_id).await;
		let state_changed = match last_presence {
			| Err(_) => true,
			| Ok((_, ref presence)) => presence.content.presence != *new_state,
		};

		let last_last_active_ago = match last_presence {
			| Err(_) => 0_u64,
			| Ok((_, ref presence)) =>
				presence.content.last_active_ago.unwrap_or_default().into(),
		};

		if !state_changed && last_last_active_ago < REFRESH_TIMEOUT {
			return Ok(());
		}

		let status_msg = match last_presence {
			| Ok((_, ref presence)) => presence.content.status_msg.clone(),
			| Err(_) => Some(String::new()),
		};

		let last_active_ago = UInt::new(0);
		let currently_active = *new_state == PresenceState::Online;
		self.set_presence(user_id, new_state, Some(currently_active), last_active_ago, status_msg)
			.await
	}

	/// Adds a presence event which will be saved until a new event replaces it.
	pub async fn set_presence(
		&self,
		user_id: &UserId,
		state: &PresenceState,
		currently_active: Option<bool>,
		last_active_ago: Option<UInt>,
		status_msg: Option<String>,
	) -> Result<()> {
		let presence_state = match state.as_str() {
			| "" => &PresenceState::Offline, // default an empty string to 'offline'
			| &_ => state,
		};

		self.db
			.set_presence(user_id, presence_state, currently_active, last_active_ago, status_msg)
			.await?;

		if (self.timeout_remote_users || self.services.globals.user_is_local(user_id))
			&& user_id != self.services.globals.server_user
		{
			let timeout = match presence_state {
				| PresenceState::Online => self.services.server.config.presence_idle_timeout_s,
				| _ => self.services.server.config.presence_offline_timeout_s,
			};

			self.timer_channel
				.0
				.send((user_id.to_owned(), Duration::from_secs(timeout)))
				.map_err(|e| {
					error!("Failed to add presence timer: {}", e);
					Error::bad_database("Failed to add presence timer")
				})?;
		}

		Ok(())
	}

	/// Removes the presence record for the given user from the database.
	///
	/// TODO: Why is this not used?
	#[allow(dead_code)]
	pub async fn remove_presence(&self, user_id: &UserId) {
		self.db.remove_presence(user_id).await;
	}

	pub async fn get_recipients(&self, user_id: &UserId) -> Result<PresenceRecipients> {
		self.db.get_recipients(user_id).await
	}

	pub async fn set_recipients(
		&self,
		user_id: &UserId,
		recipients: PresenceRecipients,
	) -> Result<()> {
		self.db.set_recipients(user_id, recipients).await
	}

	pub async fn has_recipients(&self, user_id: &UserId) -> bool {
		self.db.has_recipients(user_id).await
	}

	pub async fn remove_recipients(&self, user_id: &UserId) -> () {
		self.db.remove_recipients(user_id).await;
	}

	/// Checks if some local requesting_user has permission to view the presence
	/// of a target_user
	pub async fn can_view_presence(
		&self,
		requesting_user: &UserId,
		target_user: &UserId,
	) -> bool {
		debug_assert!(
			self.services.globals.user_is_local(requesting_user),
			"can_view_presence called for non-local user {requesting_user}"
		);

		if !self.services.config.enable_msc4495_selective_presence {
			return self
				.services
				.state_cache
				.user_sees_user(requesting_user, target_user)
				.await;
		}

		requesting_user == target_user
			|| self
				.get_recipients(target_user)
				.await
				.map_or(true, |recipients| recipients.recipient_users.contains(requesting_user))
	}

	pub async fn apply_recipients_delta(
		&self,
		user_id: &UserId,
		new_stream_id: i64,
		delta: PresenceRecipientListUpdates,
	) -> Result<()> {
		let mut recipients = self.get_recipients(user_id).await?;
		recipients.apply_delta(new_stream_id, delta);
		self.set_recipients(user_id, recipients).await
	}

	// Unset online/unavailable presence to offline on startup
	pub async fn unset_all_presence(&self) {
		let _cork = self.services.db.cork();

		for user_id in &self
			.services
			.users
			.stream_local_users()
			.collect::<Vec<OwnedUserId>>()
			.await
		{
			let presence = self.db.get_presence(user_id).await;

			let presence = match presence {
				| Ok((_, ref presence)) => &presence.content,
				| _ => continue,
			};

			if matches!(presence.presence, PresenceState::Offline) {
				trace!(%user_id, ?presence, "Skipping user");
				continue;
			}

			trace!(%user_id, ?presence, "Resetting presence to offline");

			_ = self
				.set_presence(
					user_id,
					&PresenceState::Offline,
					Some(false),
					presence.last_active_ago,
					presence.status_msg.clone(),
				)
				.await
				.inspect_err(|e| {
					debug_warn!(
						?presence,
						"{user_id} has invalid presence in database and failed to reset it to \
						 offline: {e}"
					);
				});
		}
	}

	/// Returns the most recent presence updates that happened after the event
	/// with id `since`.
	pub fn presence_since(
		&self,
		since: u64,
	) -> impl Stream<Item = (&UserId, u64, &[u8])> + Send + '_ {
		self.db.presence_since(since)
	}

	#[inline]
	pub async fn from_json_bytes_to_event(
		&self,
		bytes: &[u8],
		user_id: &UserId,
	) -> Result<PresenceEvent> {
		let presence = Presence::from_json_bytes(bytes)?;
		let event = presence
			.to_presence_event(user_id, &self.services.users)
			.await;

		Ok(event)
	}

	async fn process_presence_timer(&self, user_id: &OwnedUserId) -> Result<()> {
		let mut presence_state = PresenceState::Offline;
		let mut last_active_ago = None;
		let mut status_msg = None;

		let presence_event = self.get_presence(user_id).await;

		if let Ok(presence_event) = presence_event {
			presence_state = presence_event.content.presence;
			last_active_ago = presence_event.content.last_active_ago;
			status_msg = presence_event.content.status_msg;
		}

		let new_state = match (&presence_state, last_active_ago.map(u64::from)) {
			| (PresenceState::Online, Some(ago)) if ago >= self.idle_timeout =>
				Some(PresenceState::Unavailable),
			| (PresenceState::Unavailable, Some(ago)) if ago >= self.offline_timeout =>
				Some(PresenceState::Offline),
			| _ => None,
		};

		debug!(
			"Processed presence timer for user '{user_id}': Old state = {presence_state}, New \
			 state = {new_state:?}"
		);

		if let Some(new_state) = new_state {
			self.set_presence(user_id, &new_state, Some(false), last_active_ago, status_msg)
				.await?;
		}

		Ok(())
	}

	/// Get a user's `m.presence.sharing` account data, if it exists.
	async fn get_user_presence_sharing_config(
		&self,
		user_id: &UserId,
	) -> Result<PresenceSharingEvent> {
		self.services
			.account_data
			.get_global(user_id, GlobalAccountDataEventType::PresenceSharing)
			.await
	}

	/// Check if a room's `m.room.presence_sharing` state event is set to
	/// `suggest`.
	async fn room_allows_presence_sharing(&self, room_id: &ruma::RoomId) -> bool {
		self.services
			.state_accessor
			.room_state_get_content::<RoomPresenceSharingEventContent>(
				room_id,
				&StateEventType::RoomPresenceSharing,
				"",
			)
			.await
			.map_or(PresenceSharingHint::Forbid, |content| content.presence_sharing)
			== PresenceSharingHint::Suggest
	}

	/// Determine the recipient user set for a user's presence transition.
	pub async fn calculate_recipient_users(&self, sending_user: &UserId) -> HashSet<OwnedUserId> {
		let mut recipient_users: HashSet<OwnedUserId> = HashSet::new();

		// it must be a local user to have a presence sharing config
		if !self.services.globals.user_is_local(sending_user) {
			debug_assert!(
				false,
				"calculate_recipient_users called for non-local user {sending_user}"
			);
			debug!("User {sending_user} is not local, skipping presence recipient calculation");
			return recipient_users;
		}

		debug!("Collecting presence recipients for {sending_user}");

		// 1. Include all members of rooms listed as `allow` in the user's
		//    presence sharing config, if the room's presence sharing state is
		//    set to `suggest`.
		let user_presence_sharing_config = self
			.get_user_presence_sharing_config(sending_user)
			.await
			.map(|config| config.content)
			.unwrap_or_default();

		for (room, sharing_state) in user_presence_sharing_config.rooms {
			match sharing_state {
				| RoomPresenceSharingState::Allow
					if self.room_allows_presence_sharing(&room).await =>
				{
					// add all of the room's members to the recipient set
					let room_members = self
						.services
						.state_cache
						.room_members(&room)
						.collect::<HashSet<OwnedUserId>>()
						.await;
					debug!(
						"Adding {} members from room {room} to recipients for {sending_user}",
						room_members.len()
					);
					recipient_users.extend(room_members);
				},
				| RoomPresenceSharingState::Allow => {
					debug!("Room {room} is not set to suggest presence sharing, skipping");
				},
				| _ => {},
			}
		}

		// 2. Exclude any user IDs with a server name corresponding to any server listed as `deny` 
		//    in the user's m.presence.sharing configuration.
		for (server, sharing_state) in user_presence_sharing_config.servers {
			if sharing_state == ServerPresenceSharingState::Deny {
				debug!("Excluding users from server {server} from recipients for {sending_user}");
				recipient_users.retain(|user_id| user_id.server_name() != server);
			}
		}

		// 3. Include all local users that the user shares a room with if the
		//    user's m.presence.sharing configuration has `share_locally` set to
		//    true.
		if user_presence_sharing_config.share_locally {
			let local_users_with_mutual_rooms = self
				.services
				.users
				.stream_local_users()
				.filter(|local_user| {
					let local_user = local_user.clone();
					async move {
						self.services
							.state_cache
							.user_sees_user(sending_user, &local_user)
							.await
					}
				})
				.collect::<HashSet<OwnedUserId>>()
				.await;

			debug!(
				"Adding {} local users with mutual rooms to recipients for {sending_user}",
				local_users_with_mutual_rooms.len()
			);

			recipient_users.extend(local_users_with_mutual_rooms);
		}

		for (user, sharing_state) in user_presence_sharing_config.users {
			match sharing_state {
				// 4. Include all users listed as `allow` in the user's m.presence.sharing
				//    configuration.
				| UserPresenceSharingState::Allow => {
					debug!("Adding user {user} to recipients for {sending_user}");
					recipient_users.insert(user);
				},

				// 5. Exclude any users listed as `deny` in the user's m.presence.sharing
				//    configuration.
				| UserPresenceSharingState::Deny => {
					debug!("Excluding user {user} from recipients for {sending_user}");
					recipient_users.remove(&user);
				},

				| _ => {},
			}
		}

		debug!("Collected {} recipients for {sending_user}", recipient_users.len());
		recipient_users
	}

	pub async fn calculate_and_persist_recipient_users(
		&self,
		sending_user: &UserId,
	) -> Result<Option<(i64, PresenceRecipientListUpdates)>> {
		let current_recipients = self.get_recipients(sending_user).await.unwrap_or_default();
		let recipient_users = self.calculate_recipient_users(sending_user).await;

		let delta = current_recipients.delta_to(&recipient_users);

		if delta.is_empty() {
			debug!("No changes in presence recipients for {sending_user}, using previous stream");
			return Ok(None);
		}

		let stream_id = self.services.globals.next_count()?.cast_signed();
		let recipients = PresenceRecipients::new(stream_id, recipient_users);
		self.set_recipients(sending_user, recipients).await.ok();

		debug!(
			"Presence recipients for {} changed (+{}, -{}) stream {}",
			sending_user,
			delta.add.len(),
			delta.delete.len(),
			stream_id,
		);

		Ok(Some((stream_id, delta)))
	}

	/// Ask a remote server for a user's presence recipients.
	pub async fn ask_remote_server_for_recipients(
		&self,
		sending_user: &UserId,
	) -> Result<PresenceRecipients> {
		use ruma::api::federation::query::get_presence_recipients::msc4495::Request;

		let server_name = sending_user.server_name();

		debug!("Asking remote server {server_name} for presence recipients for {sending_user}");

		let request = Request::new(sending_user.to_owned());

		self.services
			.sending
			.send_federation_request(server_name, request)
			.await
			.map(Into::into)
	}

	/// Ask a remote server for a user's presence recipients, and store it.
	pub async fn fetch_and_store_remote_recipients(&self, sending_user: &UserId) -> Result<()> {
		let recipients = self
			.ask_remote_server_for_recipients(sending_user)
			.await
			.unwrap_or_default();

		debug!(
			"Storing {} presence recipients for {sending_user} from remote server",
			recipients.recipient_users.len()
		);
		self.set_recipients(sending_user, recipients.clone()).await
	}
}

async fn presence_timer(user_id: OwnedUserId, timeout: Duration) -> OwnedUserId {
	sleep(timeout).await;

	user_id
}
