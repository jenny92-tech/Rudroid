//! INPUT: exact field index, live declarations and class inheritance
//! OUTPUT: collision, shadowing, duplicate and concurrent-definition regressions
//! POS: validates the field lookup optimization without caching Java references
use super::*;

#[test]
fn fingerprint_collisions_check_class_name_signature_and_static_kind() {
    let mut fields = Fields::default();
    for (id, name) in [(1, "first"), (2, "second")] {
        fields.insert(id, Field {
            class: 7, name: name.into(), signature: "I".into(), is_static: false,
            value: FieldValue::Int(0), reference_class: None, next_collision: 0,
        });
    }
    // Force exactly the chain shape produced by a hash collision.
    let key = fields.by_key.hasher().hash_one((7u64, "first", "I", false));
    fields.by_key.insert(key, 2);
    fields.records.get_mut(&2).unwrap().next_collision = 1;
    assert_eq!(fields.find(7, "first", "I", false), Some(1));
    assert_eq!(fields.find_from(2, 7, "second", "I", false), Some(2));
    assert_eq!(fields.find_from(2, 8, "first", "I", false), None);
    assert_eq!(fields.find_from(2, 7, "absent", "I", false), None);
    assert_eq!(fields.find_from(2, 7, "first", "J", false), None);
    assert_eq!(fields.find_from(2, 7, "first", "I", true), None);
}

#[test]
fn inherited_lookup_observes_late_shadowing_and_keeps_static_values() {
    let s = Session::new();
    let parent = s.find_class("fields/Parent").unwrap();
    let child = s.find_class("fields/Child").unwrap();
    s.set_superclass(child, parent);
    let parent_id = s.resolve_class(parent, "test").unwrap();
    let child_id = s.resolve_class(child, "test").unwrap();
    let first = s.define_field(parent_id, "value", "I", true).unwrap();
    let first_handle = make_handle(TAG_FIELD, first);
    s.set_field(parent, first_handle, FieldValue::Int(42), true);
    assert_eq!(s.resolve_field(child, "value", "I", true), first_handle);
    assert_eq!(s.reflected_field_id(child, "value", "I", true), Some(first));
    let second = s.define_field(child_id, "value", "I", true).unwrap();
    assert_ne!(first, second);
    assert_eq!(s.resolve_field(child, "value", "I", true), make_handle(TAG_FIELD, second));
    assert_eq!(s.reflected_field_id(child, "value", "I", true), Some(second));
    assert_eq!(s.get_field(child, first_handle, FieldValue::Int(0), true), Some(FieldValue::Int(42)));
    assert_eq!(s.define_field(parent_id, "value", "I", true), Some(first));
    assert_eq!(s.get_field(parent, first_handle, FieldValue::Int(0), true), Some(FieldValue::Int(42)));
    // No negative cache may hide a later definition.
    assert!(s.resolve_field(child, "later", "J", false).is_null());
    s.take_pending();
    let later = s.define_field(child_id, "later", "J", false).unwrap();
    assert_eq!(s.resolve_field(child, "later", "J", false), make_handle(TAG_FIELD, later));
}

#[test]
fn lookup_keeps_parent_before_interface_and_exact_field_kinds() {
    let s = Session::new();
    let parent = s.find_class("fields/OrderedParent").unwrap();
    let interface = s.find_class("fields/Interface").unwrap();
    let child = s.find_class("fields/OrderedChild").unwrap();
    s.set_superclass(child, parent);
    let parent_id = s.resolve_class(parent, "test").unwrap();
    let interface_id = s.resolve_class(interface, "test").unwrap();
    let child_id = s.resolve_class(child, "test").unwrap();
    s.classes.lock().unwrap().interfaces.insert(child_id, vec![interface_id]);
    let expected = s.define_field(parent_id, "value", "I", true).unwrap();
    s.define_field(interface_id, "value", "I", true).unwrap();
    assert_eq!(s.resolve_field(child, "value", "I", true), make_handle(TAG_FIELD, expected));
    assert!(s.resolve_field(child, "value", "I", false).is_null());
    s.take_pending();
    assert!(s.resolve_field(child, "value", "J", true).is_null());
    s.take_pending();
    let instance = s.define_field(parent_id, "value", "I", false).unwrap();
    assert_ne!(instance, expected);
    assert_eq!(s.resolve_field(child, "value", "I", false), make_handle(TAG_FIELD, instance));
}

#[test]
fn concurrent_declarations_publish_one_field_and_one_index_entry() {
    let s = std::sync::Arc::new(Session::new());
    let class = s.find_class("fields/Concurrent").unwrap();
    let id = s.resolve_class(class, "test").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let workers: Vec<_> = (0..4).map(|_| {
        let s = s.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            s.define_field(id, "value", "J", false).unwrap()
        })
    }).collect();
    let ids: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    let fields = s.fields.lock().unwrap();
    assert_eq!(fields.records.len(), 1);
    assert_eq!(fields.by_key.len(), 1);
}
