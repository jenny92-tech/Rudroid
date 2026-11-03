//! INPUT:  validated sample rate, callback frame count, missing capacity and deadline
//! OUTPUT:  AudioBackpressure's bounded producer sleep duration
//! POS:    backend-independent audio pacing arithmetic; no device, callback or thread ownership
#![forbid(unsafe_code)]

use std::time::Duration;

/// Use half a callback period with the old 1ms floor and a 10ms ceiling.
/// Queue occupancy is rechecked by the producer after each wait.
/// This is a scheduling hint, not a prediction of hardware callback phase.
pub struct AudioBackpressure {
    rate: u32,
    maximum_ns: u64,
}

impl AudioBackpressure {
    pub fn new(rate: u32, callback_frames: u32) -> Option<Self> {
        if rate == 0 || callback_frames == 0 { return None; }
        let half_period = u64::from(callback_frames) * 1_000_000_000 / (u64::from(rate) * 2);
        Some(Self { rate, maximum_ns: half_period.clamp(1_000_000, 10_000_000) })
    }

    pub fn delay(&self, missing_frames: usize, remaining: Duration) -> Duration {
        if missing_frames == 0 { return Duration::ZERO; }
        let missing_ns = (missing_frames as u128 * 1_000_000_000 / u128::from(self.rate))
            .min(u128::from(self.maximum_ns)) as u64;
        Duration::from_nanos(missing_ns.max(1_000_000)).min(remaining)
    }
}

#[cfg(test)]
#[path = "../tests/unit/audio_backpressure.rs"]
mod tests;
