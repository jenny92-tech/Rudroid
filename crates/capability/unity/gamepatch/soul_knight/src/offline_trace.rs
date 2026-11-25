//! INPUT: bounded observation events from offline hooks
//! OUTPUT: private stage messages and frame counters
//! POS: Soul Knight diagnostic state

#![forbid(unsafe_code)]
use super::*;

static TRACE: OnceLock<OfflineTrace> = OnceLock::new();

#[derive(Clone, PartialEq, Eq)]
pub(super) enum SceneObservation {
    Stage(&'static str),
    State {
        current_type: String,
        ready: [bool; 3],
        receiver_current: bool,
    },
}

#[derive(Default)]
pub(super) struct SceneTrace {
    pub(super) lines: usize,
    pub(super) last: Option<SceneObservation>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum ManagerObservation {
    Stage(&'static str),
    State {
        current_type: String,
        stopped: bool,
        ready: [bool; 3],
    },
}

#[derive(Default)]
pub(super) struct ManagerTrace {
    pub(super) lines: usize,
    pub(super) last: Option<ManagerObservation>,
}

// Diagnostic-only owned text/scalars: <=15 provider labels, 16 state changes,
// 16 passive manager changes, two frame checkpoints and two once-only Title
// attempt/terminal records. Manager observations never participate in SceneOnce.
// No managed pointer/root is retained and no observer error changes control flow.
#[derive(Default)]
pub(super) struct OfflineTrace {
    pub(super) enabled: bool,
    pub(super) providers: AtomicUsize,
    pub(super) update_hits: AtomicUsize,
    pub(super) update_returns: AtomicUsize,
    pub(super) manager_hits: AtomicUsize,
    pub(super) manager_returns: AtomicUsize,
    pub(super) frames: AtomicUsize,
    pub(super) title: AtomicUsize,
    pub(super) scene: Mutex<SceneTrace>,
    pub(super) manager: Mutex<ManagerTrace>,
}

impl OfflineTrace {
    pub(super) fn provider(&self, index: usize, phase: usize) -> Option<&'static str> {
        const LABELS: [[&str; 3]; 5] = [
            [
                "provider configuration hit",
                "provider configuration succeeded",
                "provider configuration failed",
            ],
            [
                "provider manifest hit",
                "provider manifest succeeded",
                "provider manifest failed",
            ],
            [
                "provider time hit",
                "provider time succeeded",
                "provider time failed",
            ],
            [
                "provider login hit",
                "provider login succeeded",
                "provider login failed",
            ],
            [
                "provider android-id hit",
                "provider android-id succeeded",
                "provider android-id failed",
            ],
        ];
        let slot = match index {
            0..=3 => index,
            5 => 4,
            _ => return None,
        };
        if !self.enabled || phase >= 3 {
            return None;
        }
        let bit = 1usize << (slot * 3 + phase);
        (self.providers.fetch_or(bit, Ordering::Relaxed) & bit == 0).then_some(LABELS[slot][phase])
    }
    pub(super) fn title(&self, phase: usize) -> Option<&'static str> {
        const LABELS: [&str; 3] = [
            "Title invoke-attempt",
            "Title invoke-returned",
            "Title checked-call-failed",
        ];
        if !self.enabled || phase >= LABELS.len() {
            return None;
        }
        let bit = 1usize << phase;
        (self.title.fetch_or(bit, Ordering::Relaxed) & bit == 0).then_some(LABELS[phase])
    }
    pub(super) fn wants_scene(&self) -> bool {
        self.enabled && self.scene.lock().unwrap_or_else(|e| e.into_inner()).lines < 16
    }
    pub(super) fn observe(&self, observation: SceneObservation) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let mut state = self.scene.lock().unwrap_or_else(|e| e.into_inner());
        if state.lines >= 16 || state.last.as_ref() == Some(&observation) {
            return None;
        }
        state.lines += 1;
        state.last = Some(observation.clone());
        drop(state);
        Some(match observation {
            SceneObservation::Stage(label) => label.to_owned(),
            SceneObservation::State { current_type, ready, receiver_current } => format!(
                "update manager={MANAGER} current={current_type} init={} resources={} data={} receiver-current={receiver_current}",
                ready[0], ready[1], ready[2]),
        })
    }
    pub(super) fn heartbeat(&self, title_claimed: bool) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let frame = self.frames.fetch_add(1, Ordering::Relaxed) + 1;
        if frame != 120 && frame != 1200 {
            return None;
        }
        let providers = self.providers.load(Ordering::Relaxed);
        let source_seen = (0..5).fold(0, |mask, index| {
            mask | (((providers >> (index * 3)) & 1) << index)
        });
        let title = self.title.load(Ordering::Relaxed);
        Some(format!("heartbeat frame={frame} update-hit={} update-return={} source-seen={source_seen} title-claimed={title_claimed} title-attempted={} title-returned={} manager-hit={} manager-return={}",
            self.update_hits.load(Ordering::Relaxed), self.update_returns.load(Ordering::Relaxed), title & 1 != 0, title & 2 != 0,
            self.manager_hits.load(Ordering::Relaxed), self.manager_returns.load(Ordering::Relaxed)))
    }
    pub(super) fn wants_manager(&self) -> bool {
        self.enabled && self.manager.lock().unwrap_or_else(|e| e.into_inner()).lines < 16
    }
    pub(super) fn observe_manager(&self, observation: ManagerObservation) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let mut state = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        if state.lines >= 16 || state.last.as_ref() == Some(&observation) {
            return None;
        }
        state.lines += 1;
        state.last = Some(observation.clone());
        drop(state);
        Some(match observation {
            ManagerObservation::Stage(label) => label.to_owned(),
            ManagerObservation::State { current_type, stopped, ready } => format!(
                "manager-update current={current_type} is-stop={stopped} init={} resources={} data={}",
                ready[0], ready[1], ready[2]),
        })
    }
}

pub(super) fn trace() -> &'static OfflineTrace {
    TRACE.get_or_init(|| OfflineTrace {
        enabled: rudroid_common::logging::diagnostics_enabled(),
        ..OfflineTrace::default()
    })
}

pub(super) fn trace_line(line: &str) {
    if let Some(host) = HOST.get() {
        host.log_line(TAG, line);
    }
}

pub(super) fn trace_stage(label: &'static str) {
    if let Some(line) = trace().observe(SceneObservation::Stage(label)) {
        trace_line(&line);
    }
}
