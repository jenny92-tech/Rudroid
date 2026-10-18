//! INPUT:  private items of `src/session.rs` via `super`
//! OUTPUT:  `thread_isolation_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/session.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::HostStr;
use std::sync::Arc;

#[test]
fn locals_do_not_cross_threads_but_globals_do() {
    let session = Session::new();
    let class = session
        .with_thread(|s| {
            let id = s.locals.len() as u32;
            s.locals.insert(9000, Local { target: RefTarget::Class(7), frame: 0 });
            id
        });
    let _ = class;
    let local = session.new_ref(RefTarget::Class(7), false);
    let global = session.new_ref(RefTarget::Class(7), true);

    let local2 = &local;
    let global2 = &global;
    // Cross-thread view: the local from ANOTHER thread's helper context
    // would not resolve — simulate by detaching + re-attaching (fresh
    // thread state, same session).
    session.detach_current_thread();

    // The global survives detach (session-wide).
    assert!(
        session.resolve(*global2).is_some(),
        "globals are cross-thread"
    );
    // The local died with the detach.
    assert!(
        session.resolve(*local2).is_none(),
        "locals are dropped on detach"
    );
    let _ = (local2, global2);
}

#[test]
fn pending_exception_is_per_thread() {
    let session = Arc::new(Session::new());
    session.set_pending("java/lang/IllegalStateException", "thread A only");

    // A second "thread": a detached key would see no pending — simulate
    // via detach/attach of a helper thread with a channel barrier.
    let session2 = Arc::clone(&session);
    let seen = Arc::new(std::sync::Mutex::new(None::<bool>));
    let seen2 = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let is_pending = session2.with_thread(|s| s.pending.is_some());
        *seen2.lock().unwrap() = Some(is_pending);
    });
    handle.join().unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        Some(false),
        "another thread must not see thread A's pending exception"
    );
    assert!(
        session.with_thread(|s| s.pending.is_some()),
        "thread A's own pending is intact"
    );
}

#[test]
fn detach_clears_locals_and_pending_and_thread_record() {
    let session = Session::new();
    let _local = session.new_ref(RefTarget::Class(3), false);
    session.set_pending("java/lang/RuntimeException", "bye");
    assert_eq!(session.attached_thread_count(), 1);

    session.detach_current_thread();
    assert_eq!(session.attached_thread_count(), 0, "record withdrawn");
    // Re-attach (fresh state): no locals, no pending.
    assert!(
        session.with_thread(|s| s.locals.is_empty() && s.pending.is_none()),
        "detached state is gone"
    );
}

#[test]
fn method_resolution_dedupes_by_identity() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = Session::new();
    session.install_host(test_support::stub_host());
    test_support::stub_set("cache/Method", "run", "()I", MethodValue::Int(1));
    let class = session.find_class("cache/Method").unwrap();
    let first = session.resolve_method(class, "run", "()I", "GetStaticMethodID").unwrap();
    // The fixed reply has already been bound: a changed backing test
    // table must neither re-resolve nor replace this session identity.
    test_support::stub_set("cache/Method", "run", "()I", MethodValue::Int(2));
    assert_eq!(session.resolve_method(class, "run", "()I", "GetStaticMethodID"), Some(first));
    assert_eq!(session.call_checked(class, first, abi::MethodKind::Int, "CallStaticIntMethodA", false), Some(MethodValue::Int(1)));
    assert_eq!(session.methods.lock().unwrap().by_id.len(), 1);
}

#[test]
fn method_index_checks_exact_keys_on_fingerprint_collision() {
    let mut methods = MethodTable::default();
    for (serial, name) in [(1, "first"), (2, "second")] {
        methods.insert(serial, Method { class: 7, name: name.into(), sig: "()I".into(), value: MethodValue::Int(serial as i64), next_collision: 0 });
    }
    // Force the same bucket shape a real hash collision would create.
    let key = methods.by_key.hasher().hash_one((7u64, "first", "()I"));
    methods.by_key.insert(key, 2);
    methods.by_id.get_mut(&2).unwrap().next_collision = 1;
    assert_eq!(methods.find(7, "first", "()I"), Some(1));
    assert_eq!(methods.find_from(2, 7, "second", "()I"), Some(2));
    assert_eq!(methods.find_from(2, 8, "first", "()I"), None);
    assert_eq!(methods.find_from(2, 7, "first", "()Z"), None);
}

#[test]
fn concurrent_cold_method_resolution_publishes_one_id() {
    use std::sync::Barrier;
    static RESOLVING: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
    extern "C" fn miss_bool(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, _: Option<&mut c_int>) -> c_int { 0 }
    extern "C" fn value(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, out: Option<&mut i64>) -> c_int {
        let barrier = RESOLVING.lock().unwrap().as_ref().cloned();
        if let Some(barrier) = barrier { barrier.wait(); }
        *out.unwrap() = 42;
        1
    }
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = Arc::new(Session::new());
    let mut host = test_support::stub_host();
    host.catalog_query_bool = miss_bool;
    host.catalog_query_int = value;
    session.install_host(host);
    let local = session.find_class("cache/Concurrent").unwrap();
    let global = session.new_ref(session.resolve(local).unwrap(), true) as usize;
    *RESOLVING.lock().unwrap() = Some(Arc::new(Barrier::new(4)));
    let workers: Vec<_> = (0..4).map(|_| {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let method = session.resolve_method(global as *mut c_void, "run", "()I", "GetStaticMethodID").unwrap() as usize;
            session.detach_current_thread();
            method
        })
    }).collect();
    let ids: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    *RESOLVING.lock().unwrap() = None;
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(session.methods.lock().unwrap().by_id.len(), 1);
    session.delete_ref(global as *mut c_void, true, "DeleteGlobalRef");
}

#[test]
fn thread_keys_are_unique_across_threads() {
    let session = Arc::new(Session::new());
    let keys: Arc<std::sync::Mutex<Vec<u64>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let session = Arc::clone(&session);
        let keys = Arc::clone(&keys);
        handles.push(std::thread::spawn(move || {
            session.with_thread(|_s| {});
            keys.lock().unwrap().push(Session::thread_key().unwrap());
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let mut keys = keys.lock().unwrap().clone();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 4, "each thread gets its own key: {keys:?}");
}
