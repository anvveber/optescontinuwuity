//! # Release service ("delayed release")
//!
//! Holds new events back from clients and releases them in batches, once per
//! tick. Ticks fall on wall-clock multiples of `release_interval_secs` since
//! the Unix epoch.
//!
//! Every item a client can receive (PDUs, to-device messages, account data,
//! key changes) is ordered by the same global counter. The service keeps a
//! *release watermark*: the counter value at the last tick. Client-facing
//! reads are bounded by the watermark instead of the live counter, so
//! everything created between two ticks becomes visible to clients at the
//! same moment.
//!
//! `/sync` long-polls are woken per user. While delayed release is on, the
//! sync service holds those wake-ups and this service fires them right after
//! each tick, so a waiting client wakes exactly when its new data becomes
//! visible.
//!
//! Local events also get their `origin_server_ts` rounded up to the next
//! tick, so the visible timestamp doesn't reveal when the sender uploaded.
//!
//! With `release_interval_secs = 0` (the default) the watermark reads through
//! to the live counter and timestamps are untouched: behaviour is identical to
//! upstream.
//!
//! See `optes/delayed-release-spec.md`.

use std::{
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
	time::Duration,
};

use async_trait::async_trait;
use conduwuit::{Result, debug, matrix::PduCount, utils};
use tokio::sync::Notify;

use crate::{Dep, globals, sync};

pub struct Service {
	/// Tick length in milliseconds. 0 = disabled.
	interval_ms: u64,
	/// Counter value at the last tick.
	watermark: AtomicU64,
	interrupt: Notify,
	services: Services,
}

struct Services {
	globals: Dep<globals::Service>,
	sync: Dep<sync::Service>,
}

#[async_trait]
impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		let interval_ms = args
			.server
			.config
			.release_interval_secs
			.saturating_mul(1000);

		Ok(Arc::new(Self {
			interval_ms,
			watermark: AtomicU64::new(0),
			interrupt: Notify::new(),
			services: Services {
				globals: args.depend::<globals::Service>("globals"),
				sync: args.depend::<sync::Service>("sync"),
			},
		}))
	}

	#[tracing::instrument(skip_all, name = "release", level = "debug")]
	async fn worker(self: Arc<Self>) -> Result<()> {
		if !self.enabled() {
			return Ok(());
		}

		debug!(interval_ms = self.interval_ms, "Delayed release enabled");

		// Release anything created before startup.
		self.advance().await?;

		loop {
			let now = utils::millis_since_unix_epoch();
			let wait = next_tick_after(now, self.interval_ms).saturating_sub(now);

			tokio::select! {
				() = self.interrupt.notified() => break,
				() = tokio::time::sleep(Duration::from_millis(wait)) => self.advance().await?,
			}
		}

		Ok(())
	}

	fn interrupt(&self) { self.interrupt.notify_waiters(); }

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	/// Whether delayed release is on.
	#[inline]
	#[must_use]
	pub fn enabled(&self) -> bool { self.interval_ms > 0 }

	/// The highest counter value clients may see. Use this wherever a client
	/// read would otherwise use `globals.current_count()`.
	pub fn released_count(&self) -> Result<u64> {
		let current = self.services.globals.current_count()?;
		if !self.enabled() {
			return Ok(current);
		}

		Ok(self.watermark.load(Ordering::Acquire).min(current))
	}

	/// Whether an item with this count has been released to clients.
	/// Backfilled (federation) items are always released.
	pub fn is_released(&self, count: PduCount) -> bool {
		if !self.enabled() {
			return true;
		}

		match count {
			| PduCount::Normal(count) => count <= self.watermark.load(Ordering::Acquire),
			| PduCount::Backfilled(_) => true,
		}
	}

	/// `origin_server_ts` for a new local event: the next tick boundary, or
	/// `now_ms` unchanged when disabled.
	#[must_use]
	pub fn event_timestamp(&self, now_ms: u64) -> u64 {
		round_up_to_tick(now_ms, self.interval_ms)
	}

	/// One tick: moves the watermark to the current counter value, then wakes
	/// the users whose wake-ups were held since the last tick.
	///
	/// The held wake-ups are taken *before* reading the counter. A wake-up is
	/// only queued after its event is appended, so every event behind a taken
	/// wake-up has a count at or below the new watermark. Wake-ups queued after
	/// the take wait for the next tick.
	async fn advance(&self) -> Result<()> {
		let users = self.services.sync.take_deferred_wakes();
		let current = self.services.globals.current_count()?;
		self.watermark.fetch_max(current, Ordering::AcqRel);
		self.services.sync.wake_users(users).await;

		Ok(())
	}
}

/// Smallest multiple of `interval` that is >= `now`. Returns `now` if
/// `interval` is 0.
#[must_use]
pub fn round_up_to_tick(now: u64, interval: u64) -> u64 {
	if interval == 0 {
		return now;
	}

	now.div_ceil(interval).saturating_mul(interval)
}

/// Smallest multiple of `interval` that is strictly > `now`.
/// `interval` must not be 0.
#[must_use]
fn next_tick_after(now: u64, interval: u64) -> u64 {
	debug_assert!(interval > 0, "interval must not be 0");
	now.checked_div(interval)
		.unwrap_or(0)
		.saturating_add(1)
		.saturating_mul(interval)
}

#[cfg(test)]
mod tests {
	use super::{next_tick_after, round_up_to_tick};

	#[test]
	fn round_up_disabled_is_identity() {
		assert_eq!(round_up_to_tick(1_234_567, 0), 1_234_567);
	}

	#[test]
	fn round_up_on_boundary_stays() {
		assert_eq!(round_up_to_tick(30_000, 15_000), 30_000);
	}

	#[test]
	fn round_up_between_boundaries() {
		assert_eq!(round_up_to_tick(30_001, 15_000), 45_000);
		assert_eq!(round_up_to_tick(44_999, 15_000), 45_000);
	}

	#[test]
	fn next_tick_is_strictly_later() {
		assert_eq!(next_tick_after(30_000, 15_000), 45_000);
		assert_eq!(next_tick_after(30_001, 15_000), 45_000);
		assert_eq!(next_tick_after(0, 15_000), 15_000);
	}
}
