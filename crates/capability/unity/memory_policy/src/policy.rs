//! INPUT:  allocation headroom, monotonic time, available engine mechanisms and policy settings
//! OUTPUT:  ReclaimPolicy and ordered notification/recheck/unload actions
//! POS:    gives developer callbacks a bounded opportunity to recover before engine cleanup
#![forbid(unsafe_code)]

use crate::pressure::PressureGate;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Wait,
    Notify,
    Unload,
    Recovered,
    NotificationOnly,
}

enum Stage {
    Idle,
    Notifying,
    Grace { until: Duration },
    Unloading,
}

pub struct ReclaimPolicy {
    gate: PressureGate,
    recovery_kib: u64,
    grace: Duration,
    notify: bool,
    unload: bool,
    stage: Stage,
}

impl ReclaimPolicy {
    pub fn new(
        gate: PressureGate,
        recovery_kib: u64,
        grace: Duration,
        notify: bool,
        unload: bool,
    ) -> Self {
        Self {
            gate,
            recovery_kib,
            grace,
            notify,
            unload,
            stage: Stage::Idle,
        }
    }

    fn fallback(&mut self, now: Duration) -> Action {
        if self.unload {
            self.stage = Stage::Unloading;
            Action::Unload
        } else {
            self.completed(now);
            Action::NotificationOnly
        }
    }

    pub fn next(&mut self, available_kib: u64, now: Duration) -> Action {
        match self.stage {
            Stage::Idle => {
                if !self.gate.should_start(available_kib, now) {
                    return Action::Wait;
                }
                self.gate.started();
                if self.notify {
                    self.stage = Stage::Notifying;
                    Action::Notify
                } else {
                    self.fallback(now)
                }
            }
            Stage::Grace { until } => {
                if available_kib >= self.recovery_kib {
                    self.completed(now);
                    Action::Recovered
                } else if now >= until || available_kib == 0 {
                    // Zero allocation headroom cannot safely wait out the grace.
                    self.fallback(now)
                } else {
                    Action::Wait
                }
            }
            Stage::Notifying | Stage::Unloading => Action::Wait,
        }
    }

    pub fn notified(&mut self, now: Duration, succeeded: bool) {
        debug_assert!(matches!(self.stage, Stage::Notifying));
        self.stage = Stage::Grace {
            until: now.saturating_add(if succeeded {
                self.grace
            } else {
                Duration::ZERO
            }),
        };
    }

    pub fn completed(&mut self, now: Duration) {
        self.gate.completed(now);
        self.stage = Stage::Idle;
    }

    pub fn awaiting_developer(&self) -> bool {
        matches!(self.stage, Stage::Notifying | Stage::Grace { .. })
    }
}

#[cfg(test)]
#[path = "../tests/unit/policy.rs"]
mod tests;
