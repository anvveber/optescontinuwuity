use clap::Subcommand;
use conduwuit::{
	Result,
	utils::{IterStream, stream::BroadbandExt},
};
use futures::StreamExt;
use ruma::{OwnedDeviceId, OwnedUserId, api::client::push::PusherIds};

use crate::Context;

#[derive(Debug, Subcommand)]
pub enum PusherCommand {
	/// Returns all the pushers for the user.
	GetPushers {
		/// Full user ID
		user_id: OwnedUserId,
	},

	/// Deletes a specific pusher by ID
	DeletePusher {
		user_id: OwnedUserId,
		app_id: String,
		push_key: String,
	},

	/// Deletes all pushers for a user
	DeleteAllUser {
		user_id: OwnedUserId,
	},

	/// Deletes all pushers associated with a device ID
	DeleteAllDevice {
		user_id: OwnedUserId,
		device_id: OwnedDeviceId,
	},
}

pub(super) async fn process(subcommand: PusherCommand, context: &Context<'_>) -> Result {
	let services = context.services;

	match subcommand {
		| PusherCommand::GetPushers { user_id } => {
			let timer = tokio::time::Instant::now();
			let results = services.pusher.get_pushers(&user_id).await;
			let query_time = timer.elapsed();

			write!(context, "Query completed in {query_time:?}:\n\n```rs\n{results:#?}\n```")
		},
		| PusherCommand::DeletePusher { user_id, app_id, push_key } => {
			let pusher_ids = PusherIds::new(push_key, app_id);
			services.pusher.delete_pusher(&user_id, &pusher_ids).await;
			write!(context, "Deleted pusher {pusher_ids:?} for {user_id}.")
		},
		| PusherCommand::DeleteAllUser { user_id } => {
			let pushers = services
				.pusher
				.get_pusher_ids(&user_id)
				.collect::<Vec<_>>()
				.await;
			let pusher_count = pushers.len();
			pushers
				.stream()
				.for_each(async |pusher_ids| {
					services.pusher.delete_pusher(&user_id, &pusher_ids).await;
				})
				.await;
			write!(context, "Deleted {pusher_count} pushers for {user_id}.")
		},
		| PusherCommand::DeleteAllDevice { user_id, device_id } => {
			let pushers = services
				.pusher
				.get_pusher_ids(&user_id)
				.broad_filter_map(async |pusher_ids| {
					services
						.pusher
						.get_pusher_device(&pusher_ids)
						.await
						.ok()
						.as_ref()
						.is_some_and(|pusher_device| pusher_device == &device_id)
						.then_some(pusher_ids)
				})
				.collect::<Vec<_>>()
				.await;
			let pusher_count = pushers.len();
			pushers
				.stream()
				.for_each(async |pusher_ids| {
					services.pusher.delete_pusher(&user_id, &pusher_ids).await;
				})
				.await;
			write!(context, "Deleted {pusher_count} pushers for {device_id}.")
		},
	}
	.await
}
