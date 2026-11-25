//! INPUT: one native player owner and a creator-thread UI/input pump
//! OUTPUT: joined native result; UI remains serviced during native calls and cleanup
//! POS: private scheduling seam, independent of JNI/engine/SDL implementations
#![forbid(unsafe_code)]
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

pub(super) type PumpResult = Result<bool, ()>;
enum Request { Poll, Quiesce }
pub(super) struct Pump {
    request: SyncSender<Request>,
    reply: Receiver<PumpResult>,
}
impl Pump {
    pub(super) fn poll(&self) -> PumpResult {
        self.request.send(Request::Poll).map_err(|_| ())?;
        self.reply.recv().unwrap_or(Err(()))
    }
    /// Acknowledge only after the creator's previous input dispatch returned.
    /// Subsequent ticks service UI but cannot enter another game input call.
    pub(super) fn quiesce(&self) -> Result<PumpResult, ()> {
        self.request.send(Request::Quiesce).map_err(|_| ())?;
        self.reply.recv().map_err(|_| ())
    }
}

/// Never abandon/join a live native owner while its UI work is outstanding.
/// Stop/error is sticky and is observed at the next native frame boundary;
/// it cannot cancel a native call. A hung guest remains process-owned.
pub(super) fn run<T: Send + 'static>(
    native: impl FnOnce(Pump) -> T + Send + 'static,
    mut ui: impl FnMut(bool) -> PumpResult,
) -> std::io::Result<std::thread::Result<T>> {
    let (request, requests) = mpsc::sync_channel(0);
    let (reply, replies) = mpsc::sync_channel(0);
    let owner = std::thread::Builder::new().name("UnityMain".into())
        .spawn(move || native(Pump {request, reply: replies}))?;
    let mut enabled = false;
    let mut quiesced = false;
    let mut state = Ok(false);
    let mut ui_panic = None;
    loop {
        let requested = match requests.recv_timeout(Duration::from_millis(8)) {
            Ok(Request::Poll) => { enabled = true; true }
            Ok(Request::Quiesce) => {
                quiesced = true;
                // No UI/native callback before this acknowledgement: a UI
                // cleanup job may itself wait for the owner's nativeDone.
                if reply.send(state).is_err() { break; }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let next = if ui_panic.is_some() {
            Err(())
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ui(enabled && !quiesced && state == Ok(false))
            })).unwrap_or_else(|panic| { ui_panic = Some(panic); Err(()) })
        };
        state = match (state, next) {
            (Err(()), _) | (_, Err(())) => Err(()),
            (Ok(a), Ok(b)) => Ok(a || b),
        };
        if requested && reply.send(state).is_err() { break; }
    }
    let result = owner.join();
    Ok(match ui_panic { Some(panic) => Err(panic), None => result })
}

#[cfg(test)]
#[path = "../tests/unit/player_thread.rs"]
mod tests;
