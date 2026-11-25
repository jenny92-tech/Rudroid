use super::*;

#[test]
fn creator_services_blocked_native_and_cleanup_before_join() {
    let creator = std::thread::current().id();
    let (advance, wait) = mpsc::channel();
    let mut ticks = 0;
    let result = run(move |pump| {
        assert_ne!(std::thread::current().id(), creator);
        assert_eq!(pump.poll(), Ok(false));
        // Both a blocked render and a later nativeDone need creator progress.
        assert_eq!(wait.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(wait.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        47
    }, |enabled| {
        assert_eq!(std::thread::current().id(), creator);
        if enabled && ticks < 2 {
            ticks += 1;
            advance.send(ticks).unwrap();
        }
        Ok(false)
    }).unwrap().unwrap();
    assert_eq!(result, 47);
    assert_eq!(ticks, 2);
}

#[test]
fn input_stop_and_error_are_sticky_until_native_returns() {
    for stop in [Ok(true), Err(())] {
        let mut calls = 0;
        run(move |pump| {
            assert_eq!(pump.poll(), stop);
            assert_eq!(pump.poll(), stop);
            assert_eq!(pump.quiesce().unwrap(), stop);
        }, |enabled| {
            if calls != 0 { assert!(!enabled,"no new game input after stop/error"); }
            if !enabled { return Ok(false); }
            calls += 1;
            if calls == 1 { stop } else { Ok(false) }
        }).unwrap().unwrap();
    }
}

#[test]
fn cleanup_ack_waits_for_inflight_creator_input_and_keeps_ui_live() {
    use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
    let active=Arc::new(AtomicBool::new(false));
    let cleanup=Arc::new(AtomicBool::new(false));
    let native_active=Arc::clone(&active);let native_cleanup=Arc::clone(&cleanup);
    let (entered,wait_entered)=mpsc::channel();
    let (attempted,wait_attempted)=mpsc::channel();
    let (advance,wait_cleanup)=mpsc::channel();
    let mut input_calls=0;let mut replied=false;
    run(move |pump| {
        assert_eq!(pump.poll(),Ok(false));
        wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
        // The creator is still inside input, so even the rendezvous request
        // cannot be accepted. No scheduling sleep is used to arrange this.
        assert!(matches!(pump.request.try_send(Request::Quiesce),Err(mpsc::TrySendError::Full(_))));
        attempted.send(()).unwrap();
        assert_eq!(pump.quiesce().unwrap(),Ok(false));
        assert!(!native_active.load(Ordering::Acquire),"input still in native owner");
        native_cleanup.store(true,Ordering::Release);
        wait_cleanup.recv_timeout(Duration::from_secs(2)).unwrap();
    }, |enabled| {
        if enabled {
            input_calls+=1;
            if input_calls==2 {
                active.store(true,Ordering::Release);
                entered.send(()).unwrap();
                wait_attempted.recv_timeout(Duration::from_secs(2)).unwrap();
                active.store(false,Ordering::Release);
            }
        }
        if cleanup.load(Ordering::Acquire) && !replied {
            assert!(!enabled,"input must remain closed throughout native cleanup");
            advance.send(()).unwrap();replied=true;
        }
        Ok(false)
    }).unwrap().unwrap();
    assert!(replied);
}

#[test]
fn quiescence_preserves_error_after_the_last_native_frame() {
    let (failed,wait_failed)=mpsc::channel();let mut calls=0;
    let state=run(move |pump| {
        assert_eq!(pump.poll(),Ok(false));
        // The native frame has finished, but creator input is still running.
        // There is no later frame poll to discover this non-throwing failure.
        wait_failed.recv_timeout(Duration::from_secs(2)).unwrap();
        pump.quiesce().unwrap()
    }, |enabled| {
        if enabled {
            calls+=1;
            if calls==2 {failed.send(()).unwrap();return Err(());}
        }
        Ok(false)
    }).unwrap().unwrap();
    assert_eq!(state,Err(()));
}

#[test]
fn creator_panic_delivers_failure_and_joins_the_native_owner() {
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = finished.clone();
    let result = run(move |pump| {
        assert_eq!(pump.poll(), Err(()));
        assert_eq!(pump.quiesce().unwrap(), Err(()));
        worker.store(true, std::sync::atomic::Ordering::Release);
    }, |_| panic!("creator fixture")).unwrap();
    assert!(result.is_err());
    assert!(finished.load(std::sync::atomic::Ordering::Acquire));
}
