//! INPUT:  opt-in frame work duration and render-loop elapsed time
//! OUTPUT:  FrameWorkTiming with bounded all/45–90-second work histograms
//! POS:    diagnostic-only aggregates; no guest pointers, clocks or per-frame allocations
#![forbid(unsafe_code)]

const BUCKET_US: u64 = 250;
const FINITE_BUCKETS: usize = 512;
const OVERFLOW_US: u64 = BUCKET_US * FINITE_BUCKETS as u64;

struct Histogram {
    buckets: [u64; FINITE_BUCKETS + 1],
    samples: u64,
    sum_us: u128,
    max_us: u64,
    over_50ms: u64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            buckets: [0; FINITE_BUCKETS + 1],
            samples: 0,
            sum_us: 0,
            max_us: 0,
            over_50ms: 0,
        }
    }

    fn record(&mut self, work_us: u128) {
        let work_us = work_us.min(u128::from(u64::MAX)) as u64;
        let index = (work_us / BUCKET_US).min(FINITE_BUCKETS as u64) as usize;
        self.buckets[index] += 1;
        self.samples += 1;
        self.sum_us += u128::from(work_us);
        self.max_us = self.max_us.max(work_us);
        self.over_50ms += u64::from(work_us >= 50_000);
    }

    // Nearest-rank percentile, reported as the inclusive bucket upper bound.
    // Overflow has no finite upper bound; never disguise it as 128 ms.
    fn percentile_upper(&self, percent: u64) -> Option<u64> {
        if self.samples == 0 {
            return None;
        }
        let rank = (u128::from(self.samples) * u128::from(percent)).div_ceil(100);
        let mut seen = 0u128;
        for (index, count) in self.buckets.iter().enumerate() {
            seen += u128::from(*count);
            if seen >= rank {
                return (index < FINITE_BUCKETS).then_some((index as u64 + 1) * BUCKET_US - 1);
            }
        }
        None
    }

    fn report(&self, scope: &str) -> String {
        let percentile = |p| {
            self.percentile_upper(p).map_or_else(
                || {
                    if self.samples == 0 {
                        "unavailable".into()
                    } else {
                        format!(">={OVERFLOW_US}")
                    }
                },
                |value| value.to_string(),
            )
        };
        format!(
            "render work histogram: scope={scope} samples={} mean_work_us={} p50_upper_us={} p95_upper_us={} p99_upper_us={} max_work_us={} frames_over_50ms={} bucket_us={BUCKET_US} overflow_samples={}",
            self.samples,
            self.sum_us.checked_div(u128::from(self.samples)).unwrap_or(0),
            percentile(50), percentile(95), percentile(99), self.max_us,
            self.over_50ms, self.buckets[FINITE_BUCKETS],
        )
    }
}

/// Created only when unity.log_frame_progress is explicitly enabled. The
/// caller allocates it once; updates are fixed integer arithmetic. Work
/// includes input/render/maintenance and excludes the pacing sleep.
pub struct FrameWorkTiming {
    all: Histogram,
    gameplay: Histogram,
}

impl Default for FrameWorkTiming {
    fn default() -> Self {
        Self {
            all: Histogram::new(),
            gameplay: Histogram::new(),
        }
    }
}

impl FrameWorkTiming {
    pub fn record(&mut self, frame_start_ms: u128, work_us: u128) {
        self.all.record(work_us);
        if (45_000..90_000).contains(&frame_start_ms) {
            self.gameplay.record(work_us);
        }
    }

    /// Formatting occurs only after the render loop; no hot-path strings.
    pub fn reports(&self) -> [String; 2] {
        [self.all.report("all"), self.gameplay.report("45-90s")]
    }
}

#[cfg(test)]
#[path = "../tests/unit/frame_timing.rs"]
mod tests;
