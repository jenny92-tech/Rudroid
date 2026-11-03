//! INPUT:  private items of `src/handles.rs` via `super`
//! OUTPUT:  handle round-trip, removal and foreign-handle checks
//! POS:    keeps test bodies out of `src/handles.rs`; declared there with `#[path]`
use super::*;

static TABLE: HandleTable<String> = HandleTable::new();

#[test]
fn handles_round_trip_and_are_never_reused() {
    let a = TABLE.insert("a".to_owned());
    let b = TABLE.insert("b".to_owned());
    assert!(!a.is_null() && a != b);
    assert_eq!(TABLE.get(a).as_deref().map(String::as_str), Some("a"));
    assert_eq!(TABLE.remove(a).as_deref().map(String::as_str), Some("a"));
    assert!(TABLE.get(a).is_none());
    assert_ne!(TABLE.insert("c".to_owned()), a);
    assert_eq!(TABLE.get(b).as_deref().map(String::as_str), Some("b"));
}

#[test]
fn unknown_handles_resolve_to_none() {
    let table = HandleTable::<u8>::new();
    assert!(table.get(std::ptr::null_mut()).is_none());
    assert!(table.get(0xdead_beef as *mut c_void).is_none());
    assert!(table.remove(0x10 as *mut c_void).is_none());
}

#[test]
fn handles_from_another_table_do_not_select_or_remove_an_owner() {
    let first = HandleTable::new();
    let second = HandleTable::new();
    let a = first.insert("first");
    let b = second.insert("second");
    assert_ne!(a, b);
    assert!(second.get(a).is_none());
    assert!(second.remove(a).is_none());
    assert_eq!(second.get(b).as_deref(), Some(&"second"));
}

#[test]
fn removing_a_handle_does_not_free_an_in_flight_lookup() {
    struct Value(Arc<AtomicUsize>);
    impl Drop for Value {
        fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let table = HandleTable::new();
    let handle = table.insert(Value(dropped.clone()));
    let lease = table.get(handle).unwrap();
    drop(table.remove(handle));
    assert!(table.get(handle).is_none());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(lease);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn serial_exhaustion_never_wraps_even_after_catching_panics() {
    let counter = AtomicUsize::new(usize::MAX - 1);
    assert_eq!(next_serial(&counter), usize::MAX - 1);
    for _ in 0..2 {
        assert!(std::panic::catch_unwind(|| next_serial(&counter)).is_err());
        assert_eq!(counter.load(Ordering::Relaxed), usize::MAX);
    }
}
