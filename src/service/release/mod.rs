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
//! Local events also get their `origin_server_ts` rounded up to the next
//! tick, so the visible timestamp doesn't reveal when the sender uploaded.
//!
//! With `release_interval_secs = 0` (the default) the watermark reads through
//! to the live counter and timestamps are untouched: behaviour is identical to
//! upstream.
//!
//! See `optes/delayed-release-spec.md`.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use conduwuit::{Result, debug, matrix::PduCount, utils};
use tokio::sync::{Notify, watch};

use crate::{Dep, globals};

pub struct Service {
	/// Tick length in milliseconds. 0 = disabled.
	interval_ms: u64,
	/// Counter value at the last tick.
	watermark: watch::Sender<u64>,
	interrupt: Notify,
	services: Services,
}

struct Services {
	globals: Dep<globals::Service>,
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
			watermark: watch::Sender::new(0),
			interrupt: Notify::new(),
			services: Services {
				globals: args.depend::<globals::Service>("globals"),
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
		self.advance()?;

		loop {
			let now = utils::millis_since_unix_epoch();
			let wait = next_tick_after(now, self.interval_ms).saturating_sub(now);

			tokio::select! {
				() = self.interrupt.notified() => break,
				() = tokio::time::sleep(Duration::from_millis(wait)) => self.advance()?,
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

		Ok((*self.watermark.borrow()).min(current))
	}

	/// Whether an item with this count has been released to clients.
	/// Backfilled (federation) items are always released.
	pub fn is_released(&self, count: PduCount) -> bool {
		if !self.enabled() {
			return true;
		}

		match count {
			| PduCount::Normal(count) => count <= *self.watermark.borrow(),
			| PduCount::Backfilled(_) => true,
		}
	}

	/// `origin_server_ts` for a new local event: the next tick boundary, or
	/// `now_ms` unchanged when disabled.
	#[must_use]
	pub fn event_timestamp(&self, now_ms: u64) -> u64 {
		round_up_to_tick(now_ms, self.interval_ms)
	}

	/// Long-poll wait used by `/sync`.
	///
	/// Disabled: waits for the user's wake signal, exactly as upstream.
	/// Enabled: waits for the user's wake signal (new data exists), then for
	/// the next tick (the data becomes visible). So a waiting client wakes at
	/// most once per tick, and only when it has something new.
	///
	/// Returns true if woken before `timeout`.
	pub async fn wait_for_updates(
		&self,
		wake: &mut watch::Receiver<u64>,
		timeout: Duration,
	) -> bool {
		if !self.enabled() {
			return tokio::time::timeout(timeout, wake.changed())
				.await
				.is_ok();
		}

		let mut release = self.watermark.subscribe();
		tokio::time::timeout(timeout, async {
			wake.changed().await.ok()?;
			release.changed().await.ok()
		})
		.await
		.is_ok_and(|woke| woke.is_some())
	}

	/// Moves the watermark to the current counter value. Notifies waiters only
	/// if it actually moved.
	fn advance(&self) -> Result<()> {
		let current = self.services.globals.current_count()?;
		self.watermark.send_if_modified(|watermark| {
			if *watermark == current {
				false
			} else {
				*watermark = current;
				true
			}
		});

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
