//! INPUT:  private items of `src/audio.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/audio.rs`; declared there with `#[path]`
use super::*;
#[test]
fn shutdown_joins_active_and_inactive_entries_before_releasing_owners() {
    assert!(runners().lock().unwrap().is_empty());
    let mut completed = Vec::new();
    for (id, is_active) in [(100, true), (101, false)] {
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(is_active));
        let done = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_done = Arc::clone(&done);
        let thread = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_done.store(true, Ordering::Release);
        });
        let owner = thread.thread().id();
        runners().lock().unwrap().insert(
            id,
            Arc::new(Runner {
                stop,
                active,
                thread: Mutex::new(Some(thread)),
                owner,
            }),
        );
        completed.push(done);
    }
    stop_all();
    assert!(completed.iter().all(|d| d.load(Ordering::Acquire)));
    assert!(runners().lock().unwrap().is_empty());
    CLOSING.store(false, Ordering::Release);
}

#[test]
fn transient_formats_reuse_only_a_complete_bounded_valid_format() {
    assert_eq!(
        choose_format((48000, 1024, 2), None, false),
        Some(((48000, 1024, 2), false))
    );
    assert!(choose_format((0, 1024, 2), Some((48000, 1024, 2)), false).is_none());
    assert_eq!(
        choose_format((0, 0, 0), Some((48000, 1024, 2)), true),
        Some(((48000, 1024, 2), true))
    );
    for bad in [
        (0, 1, 2),
        (48000, 0, 2),
        (48000, 1024, 9),
        (48000, 16385, 2),
    ] {
        assert!(choose_format((0, 0, 0), Some(bad), true).is_none());
    }
}
