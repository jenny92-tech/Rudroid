//! INPUT: consumed scene generation and a runtime with no available managed binding
//! OUTPUT: finished callbacks avoid binding/owner work and never retry discovery
//! POS: protects the player-frame idle path; no managed pointers or native fixture
#![forbid(unsafe_code)]
use super::*;

#[test]
fn finished_scene_callbacks_do_not_enter_owner_or_binding_locks() {
    // These globals are also used by integration fixtures, which run separately.
    let features =
        SilksongFeatures::from_config(&rudroid_host_config::HostConfig::parse("").unwrap());
    let runtime = Runtime::new(HostApiV0::stubbed(), &features);
    let identity = &runtime as *const Runtime as usize;
    ENGINE.with(|slot| slot.replace(Some((identity, None))));
    GENERATION.store(1, Ordering::Release);
    HOOKED.store(true, Ordering::Release);
    STOPPED.store(false, Ordering::Release);
    // A competing thread holds both locks until the frame loop returns. A
    // bounded two-second release makes the old locking path fail instead of
    // hanging the test. The TLS fast path completes while both are held.
    std::thread::scope(|scope| {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let shared = &runtime;
        scope.spawn(move || {
            let _binding = shared.binding.lock().unwrap();
            let _owner = shared.owner.lock().unwrap();
            ready_tx.send(()).unwrap();
            let _ = done_rx.recv_timeout(std::time::Duration::from_secs(2));
        });
        ready_rx.recv().unwrap();
        let started = std::time::Instant::now();
        for _ in 0..10000 {
            runtime.frame(identity);
        }
        let elapsed = started.elapsed();
        let _ = done_tx.send(());
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "idle callbacks waited for unrelated locks: {elapsed:?}"
        );
    });
    assert!(runtime.owner.lock().unwrap().is_none());
    assert!(runtime.binding.lock().unwrap().is_none());
    ENGINE.with(|slot| slot.replace(None));
    STOPPED.store(true, Ordering::Release);
    HOOKED.store(false, Ordering::Release);
    GENERATION.store(0, Ordering::Release);
}
