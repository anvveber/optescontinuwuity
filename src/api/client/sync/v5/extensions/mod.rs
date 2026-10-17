mod e2ee;
mod to_device;

use conduwuit::Result;
use conduwuit_service::Services;
use futures::future::try_join;
use ruma::{api::client::sync::sync_events::v5, assign};

pub(super) use self::to_device::acknowledge as acknowledge_to_device;
use super::SyncContext;

/// Builds every enabled extension.
pub(super) async fn build_extensions(
	services: &Services,
	context: SyncContext<'_>,
) -> Result<v5::response::Extensions> {
	let (e2ee, to_device) =
		try_join(e2ee::collect(services, context), to_device::collect(services, context)).await?;

	Ok(assign!(v5::response::Extensions::default(), { e2ee, to_device }))
}
