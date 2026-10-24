//! When frames are sent. See spec section 4.

use std::time::Duration;

use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
	/// Send on demand; pad each burst to a power of two (min 4 frames).
	Burst = 1,
	/// Send only at window boundaries, every window, padded like `Burst`.
	Window = 2,
	/// One frame every tick, always.
	Constant = 3,
}

impl Level {
	pub fn from_u8(value: u8) -> Result<Self> {
		Ok(match value {
			| 1 => Self::Burst,
			| 2 => Self::Window,
			| 3 => Self::Constant,
			| other => bail!("level must be 1, 2 or 3, got {other}"),
		})
	}
}

/// Most frames sent in one burst or window. The rest waits.
pub const MAX_BURST: usize = 1024;

/// Smallest frame count a burst of `n` frames is padded to.
#[must_use]
pub fn burst_target(n: usize) -> usize { n.max(4).next_power_of_two() }

/// Time from `now_ms` (Unix ms) until the next window boundary plus
/// `offset`. Boundaries are wall-clock multiples of `window`.
#[must_use]
pub fn until_next_window(now_ms: u64, window: Duration, offset: Duration) -> Duration {
	let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX).max(1);
	let offset_ms = u64::try_from(offset.as_millis()).unwrap_or(0) % window_ms;
	let phase = now_ms.wrapping_sub(offset_ms) % window_ms;
	Duration::from_millis(window_ms - phase)
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
	pub level: Level,
	pub tick: Duration,
	pub window: Duration,
	/// Added to window boundaries on this side (the gateway sends 1 s after
	/// the boundary so data released at the tick is included).
	pub window_offset: Duration,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn burst_targets() {
		assert_eq!(burst_target(0), 4);
		assert_eq!(burst_target(1), 4);
		assert_eq!(burst_target(4), 4);
		assert_eq!(burst_target(5), 8);
		assert_eq!(burst_target(9), 16);
		assert_eq!(burst_target(1024), 1024);
	}

	#[test]
	fn window_timing() {
		let w = Duration::from_secs(15);
		assert_eq!(until_next_window(30_000, w, Duration::ZERO), Duration::from_secs(15));
		assert_eq!(until_next_window(30_001, w, Duration::ZERO), Duration::from_millis(14_999));
		assert_eq!(until_next_window(44_999, w, Duration::ZERO), Duration::from_millis(1));
		// with a 1 s offset the next send is at 31 s
		assert_eq!(until_next_window(30_000, w, Duration::from_secs(1)), Duration::from_secs(1));
		assert_eq!(until_next_window(31_000, w, Duration::from_secs(1)), Duration::from_secs(15));
	}
}
