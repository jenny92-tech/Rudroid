//! INPUT: shared-session typed JNI calls, catalog providers and object graphs
//! OUTPUT: inheritance/virtual dispatch, array ownership and native vararg regressions
//! POS: tests real table shims and entity reclamation rather than fixture success flags
use super::*;

fn local_session() -> Session {
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    let s = Session::new();
    s.install_host(test_support::stub_host());
    s
}

#[test]
fn jni_class_assignability_preserves_direction_interfaces_and_array_covariance() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let object = s.find_class("java/lang/Object").unwrap();
    let parent = s.find_class("test/AssignableParent").unwrap();
    let child = s.find_class("test/AssignableChild").unwrap();
    s.set_superclass(child, parent);
    assert_eq!(s.is_assignable_from(child, parent), 1);
    assert_eq!(s.is_assignable_from(parent, child), 0);
    assert_eq!(s.is_assignable_from(child, child), 1);
    let primitive = s.find_class("[I").unwrap();
    let other_primitive = s.find_class("[J").unwrap();
    let objects = s.find_class("[Ljava/lang/Object;").unwrap();
    let strings = s.find_class("[Ljava/lang/String;").unwrap();
    let cloneable = s.find_class("java/lang/Cloneable").unwrap();
    assert_eq!(s.is_assignable_from(primitive, object), 1);
    assert_eq!(s.is_assignable_from(primitive, cloneable), 1);
    assert_eq!(s.is_assignable_from(primitive, other_primitive), 0);
    assert_eq!(s.is_assignable_from(strings, objects), 1);
    assert_eq!(s.is_assignable_from(objects, strings), 0);
    let interface = s.find_class("java/lang/Runnable").unwrap();
    let classes = s.find_class("java/lang/Class").unwrap();
    let interfaces = s.new_object_array(1, classes, interface);
    let proxy = s.proxy_class(parent, interfaces).unwrap();
    assert_eq!(s.is_assignable_from(proxy, interface), 1);
    assert_eq!(s.is_assignable_from(interface, proxy), 0);
    assert_eq!(s.is_assignable_from(std::ptr::null_mut(), parent), 0);
    assert!(s.take_pending().is_some());
}

#[test]
fn inherited_catalog_resolution_respects_overrides_in_both_lookup_orders() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    for parent_first in [true, false] {
        let s = local_session();
        test_support::stub_set("test/Parent", "value", "()Z", MethodValue::Bool(false));
        test_support::stub_set("test/Child", "value", "()Z", MethodValue::Bool(true));
        test_support::stub_set("test/Parent", "inherited", "()I", MethodValue::Int(123));
        let parent = s.find_class("test/Parent").unwrap();
        let child = s.find_class("test/Child").unwrap();
        s.set_superclass(child, parent);
        if parent_first { s.resolve_method(parent, "value", "()Z", "test").unwrap(); }
        let id = s.resolve_method(child, "value", "()Z", "test").unwrap();
        assert_eq!(s.call_checked(child, id, abi::MethodKind::Bool, "test", false), Some(MethodValue::Bool(true)));
        let inherited = s.resolve_method(child, "inherited", "()I", "test").unwrap();
        let direct = s.resolve_method(parent, "inherited", "()I", "test").unwrap();
        assert_eq!(direct, inherited, "one declaring-class method identity");
        let object = s.alloc_object(child);
        let base = s.resolve_method(parent, "value", "()Z", "test").unwrap();
        assert_eq!(s.call_checked(object, base, abi::MethodKind::Bool, "test", true), Some(MethodValue::Bool(true)));
        assert!(s.resolve_method(child, "<init>", "()V", "test").is_none(), "constructors are not inherited");
    }
    test_support::stub_clear();
}

#[test]
fn object_arrays_preserve_elements_types_and_collect_rootless_cycles() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let object = s.find_class("java/lang/Object").unwrap();
    let string_class = s.find_class("java/lang/String").unwrap();
    let text = s.new_string(b"retained");
    let array = s.new_object_array(1, string_class, text);
    s.delete_ref(text, false, "test");
    assert_eq!(s.strings.lock().unwrap().len(), 1);
    let result = s.object_array_element(array, 0);
    assert_eq!(s.string_bytes(result, "test").unwrap().as_ref(), b"retained");
    assert!(s.object_array_element(array, -1).is_null());
    s.take_pending();
    s.delete_ref(array, false, "test");
    assert_eq!(s.strings.lock().unwrap().len(), 1, "returned local still pins string");
    s.delete_ref(result, false, "test");
    assert!(s.strings.lock().unwrap().is_empty());

    let strings = s.new_object_array(0, string_class, std::ptr::null_mut());
    let array_class = s.new_ref(RefTarget::Class(s.object_class(strings, "test").unwrap()), false);
    let nested = s.new_object_array(1, array_class, strings);
    assert_eq!(s.class_name(s.object_class(nested, "test").unwrap()).as_ref(), "[[Ljava/lang/String;");
    let objects_class = s.find_class("[Ljava/lang/Object;").unwrap();
    assert!(s.is_subclass(s.object_class(strings, "test").unwrap(), s.resolve_class(objects_class, "test").unwrap()), "array covariance");
    s.delete_ref(strings, false, "test");
    s.delete_ref(nested, false, "test");
    assert!(s.arrays.lock().unwrap().is_empty());

    for _ in 0..2_000 {
        let a = s.new_object_array(2, object, std::ptr::null_mut());
        let b = s.new_object_array(1, object, std::ptr::null_mut());
        let text = s.new_string(b"cycle element");
        s.set_object_array_element(a, 0, b);
        s.set_object_array_element(b, 0, a);
        s.set_object_array_element(a, 1, text);
        s.delete_ref(text, false, "test");
        let global = s.new_ref(s.resolve(a).unwrap(), true);
        s.delete_ref(a, false, "test");
        s.delete_ref(b, false, "test");
        assert_eq!(s.arrays.lock().unwrap().len(), 2, "global root keeps the graph");
        s.delete_ref(global, true, "test");
        assert!(s.arrays.lock().unwrap().is_empty());
        assert!(s.strings.lock().unwrap().is_empty());
        assert!(s.liveness.lock().unwrap().is_empty());
    }
    let array = s.new_object_array(1, object, std::ptr::null_mut());
    s.set_object_array_element(array, 0, array);
    s.delete_ref(array, false, "test");
    assert!(s.arrays.lock().unwrap().is_empty(), "self cycle reclaimed");
}

#[test]
fn parent_method_id_dispatches_to_string_builder_override_and_reclaims_payload() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    let s = session().unwrap();
    s.take_pending();
    let env = abi::env_ptr();
    let class = s.find_class("java/lang/StringBuilder").unwrap();
    let parent = s.find_class("java/lang/Object").unwrap();
    let ctor = s.resolve_method(class, "<init>", "()V", "test").unwrap();
    let append = s.resolve_method(class, "append", "(Ljava/lang/String;)Ljava/lang/StringBuilder;", "test").unwrap();
    let stringify = s.resolve_method(parent, "toString", "()Ljava/lang/String;", "test").unwrap();
    let baseline = s.objects.lock().unwrap().len();
    unsafe {
        let builder = shims::new_object_a(env, class, ctor, std::ptr::null());
        let text = s.new_string(b"content");
        let alias = shims::call_object_method_a(env, builder, append, [JValue { l:text }].as_ptr());
        let snapshot = shims::call_object_method_a(env, builder, stringify, std::ptr::null());
        assert_eq!(s.string_bytes(snapshot, "test").unwrap().as_ref(), b"content");
        assert_eq!(s.resolve(alias), s.resolve(builder));
        for handle in [snapshot, alias, text, builder, class, parent] { s.delete_ref(handle, false, "test"); }
    }
    assert!(s.objects.lock().unwrap().values().all(|o| o.builder.is_none()));
    assert_eq!(s.objects.lock().unwrap().len(), baseline);
    assert!(!s.with_thread(|t| t.pending.is_some()));
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn native_variadic_adapter_preserves_promotions_and_pending_failures() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear(); test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    let s = session().unwrap(); s.take_pending();
    static OBSERVED: AtomicU64 = AtomicU64::new(0);
    unsafe extern "C" fn prototype(_env: *mut c_void, _object: *mut c_void,
        _z: u8, _b: i8, _c: u16, _short: i16, _int: i32, _long: i64,
        _float: f32, _double: f64, _string: *mut c_void, _array: *mut c_void) {}
    unsafe extern "C" fn observe(env: *mut c_void, _object: *mut c_void,
        _method: *mut c_void, values: *const JValue) {
        let values = std::slice::from_raw_parts(values, 10);
        assert_eq!(values[0].z, 1); assert_eq!(values[1].b, -100);
        assert_eq!(values[2].c, 0xffee); assert_eq!(values[3].s, -12345);
        assert_eq!(values[4].i, -1234567); assert_eq!(values[5].j, -0x1122334455667788);
        assert_eq!(values[6].f, 1.25); assert_eq!(values[7].d, -123.125);
        let chars = shims::get_string_utf_chars(env, values[8].l, std::ptr::null_mut());
        assert_eq!(CStr::from_ptr(chars), c"native promotion");
        shims::release_string_utf_chars(env, values[8].l, chars);
        assert!(values[9].l.is_null());
        OBSERVED.fetch_add(1, Ordering::Relaxed);
    }
    let class = s.find_class("test/VariadicABI").unwrap();
    let object = s.alloc_object(class);
    let binding = JniNativeMethod { name:c"promotions".as_ptr(), signature:c"(ZBCSIJFDLjava/lang/String;[I)V".as_ptr(), fn_ptr:prototype as *mut c_void };
    let env = abi::env_ptr();
    unsafe { assert_eq!(shims::register_natives(env, class, &binding, 1), abi::JNI_OK); }
    let method = s.resolve_method(class, "promotions", "(ZBCSIJFDLjava/lang/String;[I)V", "test").unwrap();
    let text = s.new_string(b"native promotion");
    // Isolate the compiler-owned VA->A adapter with a real ABI observer. The
    // method descriptor still comes from the shared session, and all payloads
    // are valid JNI references. Unsupported arbitrary native dispatch is never
    // enabled to run this transport test.
    let mut table = *abi::tables_for_tests().1;
    table[abi::env_slot::CallVoidMethodA] = observe as *const c_void as usize;
    let table_ptr = table.as_ptr();
    let adapter_env = (&table_ptr as *const *const usize).cast_mut().cast::<c_void>();
    let call: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, ...) =
        unsafe { std::mem::transmute(table[abi::env_slot::CallVoidMethod]) };
    let before = OBSERVED.load(Ordering::Relaxed);
    unsafe {
        call(adapter_env, object, method, 1i32, -100i32, 0xffeei32, -12345i32,
            -1234567i32, -0x1122334455667788i64, 1.25f64, -123.125f64, text, std::ptr::null_mut::<c_void>());
        assert_eq!(OBSERVED.load(Ordering::Relaxed), before + 1);
        s.set_pending("java/lang/RuntimeException", "transport blocked");
        call(adapter_env, object, method);
        assert_eq!(OBSERVED.load(Ordering::Relaxed), before + 1, "pending exception blocks va_arg consumption");
        s.take_pending();
        call(adapter_env, object, std::ptr::null_mut());
        assert!(s.with_thread(|t| t.pending.is_some()), "invalid method fails before reading varargs");
        s.take_pending();
    }
    for handle in [text, object, class] { s.delete_ref(handle, false, "test"); }
}

#[test]
fn declared_fields_validate_access_and_collect_mixed_object_array_cycles() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let _object = s.find_class("java/lang/Object").unwrap();
    let parent = s.find_class("test/Node").unwrap();
    let child = s.find_class("test/ChildNode").unwrap(); s.set_superclass(child,parent);
    let class = s.resolve_class(parent,"test").unwrap();
    let id = s.define_field(class,"next","Ljava/lang/Object;",false).unwrap();
    let field = make_handle(TAG_FIELD,id);
    assert_eq!(s.resolve_field(child,"next","Ljava/lang/Object;",false),field);
    let integer = s.define_field(class,"number","I",true).unwrap();
    let integer = make_handle(TAG_FIELD,integer);
    s.set_field(child,integer,fields::FieldValue::Int(42),true);
    assert_eq!(s.get_field(parent,integer,fields::FieldValue::Int(0),true),Some(fields::FieldValue::Int(42)));
    assert!(s.get_field(parent,integer,fields::FieldValue::Long(0),true).is_none()); s.take_pending();
    assert!(s.define_field(class,"bad","Igarbage",false).is_none()); s.take_pending();
    let object_class = s.find_class("java/lang/Object").unwrap();
    for _ in 0..2_000 {
        let a = s.alloc_object(child);
        let b = s.alloc_object(parent);
        let array = s.new_object_array(2,object_class,std::ptr::null_mut());
        let text = s.new_string(b"field cycle");
        s.set_field(a,field,fields::FieldValue::Object(s.resolve(b)),false);
        s.set_field(b,field,fields::FieldValue::Object(s.resolve(array)),false);
        s.set_object_array_element(array,0,a); s.set_object_array_element(array,1,text);
        s.delete_ref(b,false,"test"); s.delete_ref(array,false,"test"); s.delete_ref(text,false,"test");
        let Some(fields::FieldValue::Object(Some(target))) = s.get_field(a,field,fields::FieldValue::Object(None),false) else { panic!("field reference"); };
        s.delete_ref(a,false,"test");
        assert_eq!(s.objects.lock().unwrap().len(),2,"snapshot holds temporary root");
        s.release_target(target);
        assert!(s.objects.lock().unwrap().is_empty()); assert!(s.arrays.lock().unwrap().is_empty()); assert!(s.strings.lock().unwrap().is_empty()); assert!(s.liveness.lock().unwrap().is_empty());
    }
}

#[test]
fn dropping_owner_with_two_edges_and_a_self_cycle_reclaims_the_complete_graph() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session(); let object = s.find_class("java/lang/Object").unwrap();
    let a = s.new_object_array(1,object,std::ptr::null_mut());
    let b = s.alloc_object(object);
    let root = s.new_object_array(2,object,std::ptr::null_mut());
    s.set_object_array_element(a,0,a);
    s.set_object_array_element(root,0,a); s.set_object_array_element(root,1,b);
    s.delete_ref(a,false,"test"); s.delete_ref(b,false,"test"); s.delete_ref(root,false,"test");
    assert!(s.objects.lock().unwrap().is_empty()); assert!(s.arrays.lock().unwrap().is_empty()); assert!(s.liveness.lock().unwrap().is_empty());
}

#[test]
fn utf16_borrows_and_weak_promotions_share_the_same_string_owner() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();let env=abi::env_ptr();
    let units=[65,0,0x4e2d,0xd83d,0xde00,0xd800];
    let text=unsafe{shims::new_string(env,units.as_ptr(),units.len() as i32)};
    assert_eq!(shims::get_string_length(env,text),6);
    let weak=shims::new_weak_global_ref(env,text);
    let first=unsafe{shims::get_string_chars(env,text,std::ptr::null_mut())};
    let second=unsafe{shims::get_string_chars(env,text,std::ptr::null_mut())};
    assert_eq!(first,second);assert_eq!(unsafe{std::slice::from_raw_parts(first,6)},units);
    shims::delete_local_ref(env,text);
    let promoted=shims::new_local_ref(env,weak);assert!(!promoted.is_null());
    unsafe{shims::release_string_chars(env,promoted,first);shims::release_string_chars(env,promoted,second);shims::delete_local_ref(env,promoted)};
    assert!(shims::new_local_ref(env,weak).is_null());assert_eq!(shims::exception_check(env),0);
    assert_eq!(shims::get_object_ref_type(env,weak),3);shims::delete_weak_global_ref(env,weak);
}

#[test]
fn critical_utf16_table_aliases_keep_storage_pinned_until_the_last_release() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host()); let s = session().unwrap(); s.take_pending();
    let table = abi::tables_for_tests().1;
    assert_eq!(table[abi::env_slot::GetStringCritical], table[abi::env_slot::GetStringChars]);
    assert_eq!(table[abi::env_slot::ReleaseStringCritical], table[abi::env_slot::ReleaseStringChars]);
    let units = [0, 0xd800, 0xdc00, 0x4e2d];
    let env = abi::env_ptr();
    let string = s.new_string_units(&units); let weak = shims::new_weak_global_ref(env, string);
    let first = s.borrow_string_units(string); let second = s.borrow_string_units(string);
    assert_eq!(first, second);
    // VM-internal root mutation deliberately stresses collection; guest JNI
    // code must not perform unrelated calls while it owns a critical lease.
    s.delete_ref(string, false, "unit test root mutation");
    s.release_string_chars(first.cast());
    // SAFETY: the second immutable UTF-16 lease pins four units despite zero JNI roots.
    assert_eq!(unsafe { std::slice::from_raw_parts(second, units.len()) }, units);
    s.release_string_chars(second.cast());
    assert!(shims::new_local_ref(env, weak).is_null());
    shims::delete_weak_global_ref(env, weak);
}

#[test]
fn long_shared_mixed_cycle_traces_each_node_once_and_reclaims_its_leaves() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let text = s.new_string(b"two heap edges to one leaf");
    let leaf = s.resolve(text).unwrap();
    let ids: Vec<_> = (0..4096).map(|_| s.next_serial().unwrap()).collect();
    let target = |i: usize| if i % 2 == 0 { RefTarget::Object(ids[i]) } else { RefTarget::Array(ids[i]) };
    let root = s.new_ref(target(0), false);
    s.retain_edge(leaf); s.retain_edge(leaf);
    {
        let mut objects = s.objects.lock().unwrap();
        let mut arrays = s.arrays.lock().unwrap();
        for i in 0..ids.len() {
            let mut edges = vec![Some(target((i + 1) % ids.len())), Some(target((i + 2) % ids.len()))];
            if i == 0 || i == ids.len() - 1 { edges.push(Some(leaf)); }
            if i % 2 == 0 {
                let mut object = fields::Object::new(0);
                for (field, edge) in edges.into_iter().enumerate() { object.fields.insert(field as u64, fields::FieldValue::Object(edge)); }
                objects.insert(ids[i], object);
            } else {
                arrays.insert(ids[i], objects::ObjectArray { class:0, element_class:0, elements:edges, primitive:None, marked:std::cell::Cell::new(false), trace_next:std::cell::Cell::new(None), incoming:std::cell::Cell::new(0) });
            }
        }
    }
    for i in 0..ids.len() { s.retain_edge(target((i + 1) % ids.len())); s.retain_edge(target((i + 2) % ids.len())); }
    s.delete_ref(text, false, "test");
    for _ in 0..3 {
        let mut live = s.liveness.lock().unwrap();
        let mut objects = s.objects.lock().unwrap();
        let mut arrays = s.arrays.lock().unwrap();
        assert_eq!(s.collect_objects(&mut live, &mut objects, &mut arrays), ids.len());
        assert!(objects.values().all(|o| o.trace_next.get().is_none()));
        assert!(arrays.values().all(|a| a.trace_next.get().is_none()));
    }
    let global = s.new_ref(target(ids.len() - 1), true);
    s.delete_ref(root, false, "test");
    assert_eq!(s.objects.lock().unwrap().len() + s.arrays.lock().unwrap().len(), ids.len());
    assert_eq!(s.strings.lock().unwrap().len(), 1);
    s.delete_ref(global, true, "test");
    assert!(s.objects.lock().unwrap().is_empty()); assert!(s.arrays.lock().unwrap().is_empty());
    assert!(s.strings.lock().unwrap().is_empty()); assert!(s.liveness.lock().unwrap().is_empty());
}

#[test]
fn mixed_graph_collection_matches_independent_reachability_oracle() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let mut random = 0x72b6_59c1_u64;
    for trial in 0..200 {
        let mut objects = HashMap::new(); let mut arrays = HashMap::new(); let mut live = HashMap::new();
        let mut edges = vec![Vec::new(); 64];
        let target = |i: usize| if i % 2 == 0 { RefTarget::Object(i as u64 + 1) } else { RefTarget::Array(i as u64 + 1) };
        let mut expected = std::collections::HashSet::new(); let mut pending = Vec::new();
        for (i, outgoing) in edges.iter_mut().enumerate() {
            random ^= random << 13; random ^= random >> 7; random ^= random << 17;
            let rooted = (random % 23 == 0) || trial % 7 == 0;
            live.insert(i as u64 + 1, usize::from(rooted));
            if rooted { pending.push(i); }
            for j in 0..(random % 4) { outgoing.push(((random >> (j * 8)) % 64) as usize); }
            if i % 2 == 0 {
                let mut object = fields::Object::new(0);
                for (field, child) in outgoing.iter().enumerate() { object.fields.insert(field as u64, fields::FieldValue::Object(Some(target(*child)))); }
                objects.insert(i as u64 + 1, object);
            } else {
                arrays.insert(i as u64 + 1, objects::ObjectArray { class:0, element_class:0, elements:outgoing.iter().map(|child|Some(target(*child))).collect(), primitive:None, marked:std::cell::Cell::new(false), trace_next:std::cell::Cell::new(None), incoming:std::cell::Cell::new(0) });
            }
        }
        for child in edges.iter().flatten() {
            let incoming = if *child % 2 == 0 { &objects[&(*child as u64 + 1)].incoming } else { &arrays[&(*child as u64 + 1)].incoming };
            incoming.set(incoming.get() + 1);
        }
        while let Some(i) = pending.pop() { if expected.insert(i) { pending.extend(&edges[i]); } }
        assert_eq!(s.collect_objects(&mut live, &mut objects, &mut arrays), expected.len());
        for i in 0..64 { assert_eq!(objects.contains_key(&(i as u64 + 1)) || arrays.contains_key(&(i as u64 + 1)), expected.contains(&i), "trial {trial}, node {i}"); }
        live.values_mut().for_each(|count| *count = 0);
        assert_eq!(s.collect_objects(&mut live, &mut objects, &mut arrays), 0);
        assert!(objects.is_empty() && arrays.is_empty() && live.is_empty());
    }
}

#[test]
fn graph_leaves_keep_incoming_owners_and_release_each_duplicate_edge() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session(); let class = s.find_class("java/lang/Object").unwrap();
    let leaf = s.alloc_object(class); let target = s.resolve(leaf).unwrap();
    let a = s.new_object_array(2, class, leaf);
    let b = s.new_object_array(1, class, leaf);
    let RefTarget::Object(id) = target else { unreachable!() };
    assert_eq!(s.objects.lock().unwrap()[&id].incoming.get(), 3);
    s.delete_ref(leaf, false, "test");
    assert_eq!(s.objects.lock().unwrap().len(), 1, "incoming roots preserve the zero-root leaf");
    s.set_object_array_element(a, 0, std::ptr::null_mut());
    assert_eq!(s.objects.lock().unwrap()[&id].incoming.get(), 2);
    s.set_object_array_element(a, 1, std::ptr::null_mut());
    assert_eq!(s.objects.lock().unwrap()[&id].incoming.get(), 1);
    let promoted = s.object_array_element(b, 0); assert_eq!(s.resolve(promoted), Some(target));
    s.delete_ref(b, false, "test");
    assert_eq!(s.objects.lock().unwrap()[&id].incoming.get(), 0);
    assert_eq!(s.resolve(promoted), Some(target));
    s.delete_ref(promoted, false, "test"); s.delete_ref(a, false, "test");
    assert!(s.objects.lock().unwrap().is_empty() && s.arrays.lock().unwrap().is_empty());

    // Primitive storage has no graph children but can still be an object-array
    // element. An outstanding copy lease and a promoted local are real roots.
    let bytes = s.new_byte_array(16); let bytes_target = s.resolve(bytes).unwrap();
    let owner = s.new_object_array(1, class, bytes);
    let lease = s.borrow_byte_array(bytes);
    s.delete_ref(bytes, false, "test");
    let promoted = s.object_array_element(owner, 0);
    s.delete_ref(owner, false, "test");
    assert_eq!(s.resolve(promoted), Some(bytes_target));
    // SAFETY: lease is the owned copy returned by this session and released once.
    unsafe { s.release_byte_array(promoted, lease, 2); }
    s.delete_ref(promoted, false, "test");
    assert!(s.objects.lock().unwrap().is_empty() && s.arrays.lock().unwrap().is_empty());
    assert!(s.liveness.lock().unwrap().is_empty());
}

#[test]
fn deep_unowned_mixed_chain_reclaims_without_recursion_and_keeps_other_roots() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session(); let class = s.find_class("java/lang/Object").unwrap();
    let keeper = s.alloc_object(class); let text = s.new_string(b"tail leaf"); let leaf = s.resolve(text).unwrap();
    let ids: Vec<_> = (0..4096).map(|_|s.next_serial().unwrap()).collect();
    let target = |i: usize| if i % 2 == 0 { RefTarget::Object(ids[i]) } else { RefTarget::Array(ids[i]) };
    let root = s.new_ref(target(0), false); s.retain_edge(leaf); s.retain_edge(leaf);
    {
        let mut objects = s.objects.lock().unwrap(); let mut arrays = s.arrays.lock().unwrap();
        for i in 0..ids.len() {
            let edges = if i + 1 == ids.len() { vec![Some(leaf), Some(leaf)] } else { vec![Some(target(i + 1))] };
            if i % 2 == 0 {
                let mut object = fields::Object::new(0); object.incoming.set(usize::from(i != 0));
                for (id, edge) in edges.into_iter().enumerate() { object.fields.insert(id as u64, fields::FieldValue::Object(edge)); }
                objects.insert(ids[i], object);
            } else {
                arrays.insert(ids[i], objects::ObjectArray { class:0, element_class:0, elements:edges, primitive:None, marked:std::cell::Cell::new(false), trace_next:std::cell::Cell::new(None), incoming:std::cell::Cell::new(1) });
            }
        }
    }
    s.delete_ref(text, false, "test"); s.delete_ref(root, false, "test");
    assert_eq!(s.objects.lock().unwrap().len(), 1); assert!(s.arrays.lock().unwrap().is_empty());
    assert!(s.strings.lock().unwrap().is_empty()); assert!(s.resolve(keeper).is_some());
    s.delete_ref(keeper, false, "test"); assert!(s.objects.lock().unwrap().is_empty());
    assert!(s.liveness.lock().unwrap().is_empty());
}

#[test]
fn peeling_owner_collects_residual_cycle_and_preserves_shared_global_child() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session(); let class = s.find_class("java/lang/Object").unwrap();
    let cycle = s.new_object_array(2, class, std::ptr::null_mut());
    let live = s.new_object_array(0, class, std::ptr::null_mut());
    let owner = s.new_object_array(3, class, std::ptr::null_mut());
    let text = s.new_string(b"dead cycle leaf");
    s.set_object_array_element(cycle, 0, cycle); s.set_object_array_element(cycle, 1, text);
    s.set_object_array_element(owner, 0, cycle); s.set_object_array_element(owner, 1, live); s.set_object_array_element(owner, 2, live);
    let global = s.new_ref(s.resolve(live).unwrap(), true);
    s.delete_ref(text, false, "test"); s.delete_ref(cycle, false, "test"); s.delete_ref(live, false, "test");
    s.delete_ref(owner, false, "test");
    assert_eq!(s.arrays.lock().unwrap().len(), 1); assert!(s.strings.lock().unwrap().is_empty());
    assert_eq!(s.array_length(global), 0);
    let Some(RefTarget::Array(id)) = s.resolve(global) else { unreachable!() };
    assert_eq!(s.arrays.lock().unwrap()[&id].incoming.get(), 0);
    s.delete_ref(global, true, "test"); assert!(s.arrays.lock().unwrap().is_empty()); assert!(s.liveness.lock().unwrap().is_empty());
}

#[test]
fn byte_array_copy_leases_commit_abort_and_reclaim_without_exposing_rust_storage() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=local_session();
    let array=s.new_byte_array(3);let id=s.resolve(array).unwrap();
    let source=[1i8,2,3];unsafe{s.byte_region(array,0,3,source.as_ptr().cast_mut(),true)};
    let elements=s.borrow_byte_array(array);unsafe{*elements.add(1)=9;s.release_byte_array(array,elements,1)};
    let mut output=[0i8;3];unsafe{s.byte_region(array,0,3,output.as_mut_ptr(),false)};assert_eq!(output,[1,9,3]);
    unsafe{*elements=7;s.release_byte_array(array,elements,2);s.byte_region(array,0,3,output.as_mut_ptr(),false)};assert_eq!(output,[1,9,3]);
    assert!(s.primitive_leases.lock().unwrap().is_empty());s.delete_ref(array,false,"test");assert!(s.arrays.lock().unwrap().is_empty());
    assert!(!s.liveness.lock().unwrap().contains_key(&match id {RefTarget::Array(id)=>id,_=>unreachable!()}));
}

#[test]
fn builtin_string_is_an_object_and_proxy_interfaces_share_class_identity() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local_session(); install_builtin_types(&s);
    let object=s.find_class("java/lang/Object").unwrap();
    let text=s.new_string(b"Object value");
    let array=s.new_object_array(1,object,text);assert!(!array.is_null());
    let base=s.find_class("test/Proxy").unwrap();let runnable=s.find_class("java/lang/Runnable").unwrap();
    let classes=s.find_class("java/lang/Class").unwrap();let interfaces=s.new_object_array(1,classes,runnable);
    let proxy=s.proxy_class(base,interfaces).unwrap();let proxy2=s.proxy_class(base,interfaces).unwrap();
    let id=s.resolve_class(proxy,"test").unwrap();assert_eq!(id,s.resolve_class(proxy2,"test").unwrap());
    assert!(s.is_subclass(id,s.resolve_class(runnable,"test").unwrap()));
    assert!(s.is_subclass(id,s.resolve_class(object,"test").unwrap()));
    test_support::stub_set("java/lang/Runnable","defaultValue","()Z",MethodValue::Bool(true));
    let inherited=s.resolve_method(proxy,"defaultValue","()Z","test").unwrap();
    let interface_method=s.resolve_method(runnable,"defaultValue","()Z","test").unwrap();assert_eq!(inherited,interface_method);
    let instance=s.alloc_object(proxy);
    assert_eq!(s.call_checked(instance,interface_method,abi::MethodKind::Bool,"test",true),Some(MethodValue::Bool(true)));
    s.set_superclass(runnable,proxy);assert!(s.with_thread(|t|t.pending.is_some()));s.take_pending();
    let unrelated=s.find_class("test/Unrelated").unwrap();assert!(!s.is_subclass(id,s.resolve_class(unrelated,"test").unwrap()));s.delete_ref(unrelated,false,"test");
    let field=s.define_field(s.resolve_class(base,"test").unwrap(),"job","Ljava/lang/Runnable;",false).unwrap();
    s.set_field(instance,make_handle(TAG_FIELD,field),fields::FieldValue::Object(s.resolve(instance)),false);
    assert!(!s.with_thread(|t|t.pending.is_some()));
    for handle in [instance,proxy2,proxy,interfaces,classes,runnable,base,array,text,object]{s.delete_ref(handle,false,"test");}
    assert!(s.objects.lock().unwrap().is_empty());assert!(s.arrays.lock().unwrap().is_empty());
}

#[test]
fn boolean_int_native_preserves_signed_width_and_rejects_invalid_calls() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn accepts(_env: *mut c_void, _receiver: *mut c_void, value: i32) -> u8 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        u8::from(value == i32::MIN || value == i32::MAX)
    }
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host());
    let s = session().unwrap(); s.take_pending();
    let env = abi::env_ptr();
    let class = s.find_class("test/BooleanIntDispatch").unwrap();
    let receiver = s.alloc_object(class);
    let binding = JniNativeMethod { name: c"accepts".as_ptr(), signature: c"(I)Z".as_ptr(), fn_ptr: accepts as *mut c_void };
    // SAFETY: live class and binding with an exact (JNIEnv*, jobject, jint) -> jboolean ABI.
    assert_eq!(unsafe { s.register_natives(class, &binding, 1) }, abi::JNI_OK);
    assert_eq!(s.registered_native("test/BooleanIntDispatch", "accepts", "(I)Z"), Some(accepts as usize));
    let method = s.resolve_method(class, "accepts", "(I)Z", "test").unwrap();
    for value in [i32::MIN, -1, 0, 1, i32::MAX] {
        let args = [JValue { i: value }];
        let expected = u8::from(value == i32::MIN || value == i32::MAX);
        // SAFETY: live receiver/class, matching method and one initialized jint argument.
        unsafe {
            assert_eq!(shims::call_boolean_method_a(env, receiver, method, args.as_ptr()), expected);
            assert_eq!(shims::call_static_boolean_method_a(env, class, method, args.as_ptr().cast_mut().cast()), expected);
        }
        assert!(s.take_pending().is_none());
    }
    let calls = CALLS.load(Ordering::SeqCst);
    // SAFETY: null args must be rejected before this one-argument native is invoked;
    // a mismatched return accessor must likewise fail without dispatch.
    unsafe {
        assert_eq!(shims::call_boolean_method_a(env, receiver, method, std::ptr::null()), 0);
        assert!(s.take_pending().is_some());
        assert_eq!(shims::call_int_method_a(env, receiver, method, [JValue { i: 1 }].as_ptr()), 0);
        assert!(s.take_pending().is_some());
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), calls);
    assert_eq!(s.unregister_natives(class), abi::JNI_OK);
    assert_eq!(s.registered_native("test/BooleanIntDispatch", "accepts", "(I)Z"), None);
    // SAFETY: the cached method ID is still valid metadata, but its revoked binding must fail.
    assert_eq!(unsafe { shims::call_boolean_method_a(env, receiver, method, [JValue { i: 1 }].as_ptr()) }, 0);
    assert!(s.take_pending().is_some());
    assert_eq!(CALLS.load(Ordering::SeqCst), calls);
    for handle in [receiver, class] { s.delete_ref(handle, false, "test"); }
}

#[test]
fn void_float_native_preserves_bits_and_rejects_invalid_calls() {
    use std::sync::atomic::{AtomicU32,AtomicUsize,Ordering};
    static BITS:AtomicU32=AtomicU32::new(0);
    static CALLS:AtomicUsize=AtomicUsize::new(0);
    extern "C" fn accepts(_env:*mut c_void,_receiver:*mut c_void,value:f32) {
        BITS.store(value.to_bits(),Ordering::SeqCst);CALLS.fetch_add(1,Ordering::SeqCst);
    }
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    let env=abi::env_ptr();let class=s.find_class("test/VoidFloatDispatch").unwrap();let object=s.alloc_object(class);
    let binding=JniNativeMethod{name:c"accepts".as_ptr(),signature:c"(F)V".as_ptr(),fn_ptr:accepts as *mut c_void};
    // SAFETY: live class, exact JNIEnv*/jobject/jfloat -> void native prototype.
    assert_eq!(unsafe{s.register_natives(class,&binding,1)},abi::JNI_OK);
    assert_eq!(s.registered_native("test/VoidFloatDispatch","accepts","(F)V"),Some(accepts as usize));
    let method=s.resolve_method(class,"accepts","(F)V","test").unwrap();
    for bits in [0,0x80000000,1,0x42f00000,0x7f800000,0x7fc12345] {
        let value=f32::from_bits(bits);let args=[JValue{f:value}];
        // SAFETY: one live float argument and exact accessor; the variadic
        // form uses C's mandatory double promotion for a float argument.
        unsafe {
            shims::call_void_method_a(env,object,method,args.as_ptr());assert_eq!(BITS.load(Ordering::SeqCst),bits);
            shims::call_static_void_method_a(env,class,method,args.as_ptr().cast_mut().cast());assert_eq!(BITS.load(Ordering::SeqCst),bits);
            #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))] {
                let call:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...)=std::mem::transmute((*(env as *const *const usize)).add(abi::env_slot::CallVoidMethod).read());
                call(env,object,method,f64::from(value));assert_eq!(BITS.load(Ordering::SeqCst),bits);
            }
        }
        assert!(s.take_pending().is_none());
    }
    let calls=CALLS.load(Ordering::SeqCst);
    // SAFETY: invalid/missing arguments and revoked natives must fail before dispatch.
    unsafe {
        shims::call_void_method_a(env,object,method,std::ptr::null());assert!(s.take_pending().is_some());
        assert_eq!(shims::call_int_method_a(env,object,method,[JValue{f:30.0}].as_ptr()),0);assert!(s.take_pending().is_some());
        assert_eq!(s.unregister_natives(class),abi::JNI_OK);
        shims::call_void_method_a(env,object,method,[JValue{f:30.0}].as_ptr());assert!(s.take_pending().is_some());
    }
    assert_eq!(s.registered_native("test/VoidFloatDispatch","accepts","(F)V"),None);
    assert_eq!(CALLS.load(Ordering::SeqCst),calls);
    for handle in [object,class]{s.delete_ref(handle,false,"test");}
}

#[test]
fn ten_argument_native_preserves_registers_stack_references_and_revocation() {
    use std::sync::atomic::{AtomicUsize,Ordering};
    static CALLS:AtomicUsize=AtomicUsize::new(0);
    extern "C" fn accepts(env:*mut c_void,_receiver:*mut c_void,text:*mut c_void,kind:i32,a:u8,b:u8,c:u8,d:u8,hint:*mut c_void,limit:i32,e:u8,f:u8) {
        // SAFETY: this callback receives the shared VM's live JNIEnv and its
        // automatic native frame owns both reference arguments until return.
        let env=unsafe{rudroid_common::jni_env::JniEnv::from_raw(env)};
        if kind==-123&&[a,b,c,d,e,f]==[1,0,1,0,1,0]&&limit==0x12345678
            &&env.string_utf(text).is_some_and(|s|s.as_c_str()==c"text")
            &&env.string_utf(hint).is_some_and(|s|s.as_c_str()==c"hint") {CALLS.fetch_add(1,Ordering::SeqCst);}
    }
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    let env=abi::env_ptr();let class=s.find_class("test/TenArgumentDispatch").unwrap();let object=s.alloc_object(class);
    let text=s.new_string(b"text");let hint=s.new_string(b"hint");
    let signature=c"(Ljava/lang/String;IZZZZLjava/lang/String;IZZ)V";
    let binding=JniNativeMethod{name:c"show".as_ptr(),signature:signature.as_ptr(),fn_ptr:accepts as *mut c_void};
    let args=[JValue{l:text},JValue{i:-123},JValue{z:1},JValue{z:0},JValue{z:1},JValue{z:0},JValue{l:hint},JValue{i:0x12345678},JValue{z:1},JValue{z:0}];
    let before=CALLS.load(Ordering::SeqCst);
    // SAFETY: exact ten-parameter native ABI; A calls use ten initialized
    // jvalues and variadic boolean arguments use C integer promotion.
    unsafe {
        assert_eq!(s.register_natives(class,&binding,1),abi::JNI_OK);
        let method=s.resolve_method(class,"show",signature.to_str().unwrap(),"test").unwrap();
        shims::call_void_method_a(env,object,method,args.as_ptr());
        shims::call_static_void_method_a(env,class,method,args.as_ptr().cast_mut().cast());
        #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))] {
            let call:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...)=std::mem::transmute((*(env as *const *const usize)).add(abi::env_slot::CallVoidMethod).read());
            call(env,object,method,text,-123i32,1i32,0i32,1i32,0i32,hint,0x12345678i32,1i32,0i32);
        }
        assert!(s.take_pending().is_none());
        let calls=CALLS.load(Ordering::SeqCst);
        assert_eq!(calls,before+if cfg!(all(target_arch="aarch64",target_os="linux",target_env="gnu")){3}else{2});
        shims::call_void_method_a(env,object,method,std::ptr::null());assert!(s.take_pending().is_some());
        assert_eq!(shims::call_int_method_a(env,object,method,args.as_ptr()),0);assert!(s.take_pending().is_some());
        assert_eq!(s.unregister_natives(class),abi::JNI_OK);
        shims::call_void_method_a(env,object,method,args.as_ptr());assert!(s.take_pending().is_some());
        assert_eq!(s.registered_native("test/TenArgumentDispatch","show",signature.to_str().unwrap()),None);
        assert_eq!(CALLS.load(Ordering::SeqCst),calls);
    }
    for handle in [hint,text,object,class]{s.delete_ref(handle,false,"test");}
}

#[test]
fn registered_delayed_boolean_dispatch_keeps_object_and_long_arguments() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();let env=abi::env_ptr();
    unsafe extern "C" fn posted(_env:*mut c_void,receiver:*mut c_void,job:*mut c_void,delay:i64)->u8 {
        u8::from(!receiver.is_null() && !job.is_null() && delay==0x1_0000_1234)
    }
    let class=s.find_class("test/DelayDispatch").unwrap();let receiver=s.alloc_object(class);let job=s.alloc_object(class);
    let binding=JniNativeMethod{name:c"postDelayed".as_ptr(),signature:c"(Ljava/lang/Runnable;J)Z".as_ptr(),fn_ptr:posted as *mut c_void};
    assert_eq!(unsafe{s.register_natives(class,&binding,1)},abi::JNI_OK);
    let method=s.resolve_method(class,"postDelayed","(Ljava/lang/Runnable;J)Z","test").unwrap();let args=[JValue{l:job},JValue{j:0x1_0000_1234}];
    assert_eq!(unsafe{shims::call_boolean_method_a(env,receiver,method,args.as_ptr())},1);
    assert_eq!(unsafe{shims::call_static_boolean_method_a(env,class,method,args.as_ptr().cast_mut().cast())},1);
    assert!(!s.with_thread(|t|t.pending.is_some()));
    for handle in [job,receiver,class]{s.delete_ref(handle,false,"test");}
}

#[test]
fn file_stream_reads_non_bmp_paths_and_closes_when_its_last_owner_is_deleted() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();let env=abi::env_ptr();
    let path=std::env::temp_dir().join(format!("rudroid-jni-stream-{}-😀.txt",std::process::id()));
    std::fs::write(&path,b"owned-stream").unwrap();
    let fd_count=||std::fs::read_dir("/proc/self/fd").unwrap().filter_map(Result::ok).filter(|entry|std::fs::read_link(entry.path()).ok().as_ref()==Some(&path)).count();
    let before=fd_count();let units:Vec<u16>=path.to_str().unwrap().encode_utf16().collect();
    let name=unsafe{shims::new_string(env,units.as_ptr(),units.len() as i32)};
    let class=s.find_class("java/io/FileInputStream").unwrap();let ctor=s.resolve_method(class,"<init>","(Ljava/lang/String;)V","test").unwrap();
    let stream=unsafe{shims::new_object_a(env,class,ctor,[JValue{l:name}].as_ptr())};
    assert!(!stream.is_null());assert!(!s.with_thread(|t|t.pending.is_some()));assert_eq!(fd_count(),before+1);
    let array=s.new_byte_array(20);let read=s.resolve_method(class,"read","([B)I","test").unwrap();
    assert_eq!(unsafe{shims::call_int_method_a(env,stream,read,[JValue{l:array}].as_ptr())},12);
    s.delete_ref(stream,false,"test");assert_eq!(fd_count(),before);
    for handle in [array,class,name]{s.delete_ref(handle,false,"test");}std::fs::remove_file(path).unwrap();
}

#[test]
fn scanner_absolute_anchors_read_whole_utf8_stream_and_retain_its_owner() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host());
    let s = session().unwrap();
    s.take_pending();
    let env = abi::env_ptr();
    let stream_class = s.find_class("java/io/FileInputStream").unwrap();
    let scanner_class = s.find_class("java/util/Scanner").unwrap();
    let stream_ctor = s.resolve_method(stream_class, "<init>", "(Ljava/lang/String;)V", "test").unwrap();
    let scanner_ctor = s.resolve_method(scanner_class, "<init>", "(Ljava/io/InputStream;Ljava/lang/String;)V", "test").unwrap();
    let delimiter_method = s.resolve_method(scanner_class, "useDelimiter", "(Ljava/lang/String;)Ljava/util/Scanner;", "test").unwrap();
    let has_next = s.resolve_method(scanner_class, "hasNext", "()Z", "test").unwrap();
    let next = s.resolve_method(scanner_class, "next", "()Ljava/lang/String;", "test").unwrap();
    let close = s.resolve_method(scanner_class, "close", "()V", "test").unwrap();
    let encoding = s.new_string(b"UTF-8");
    for (index, (delimiter, input)) in [
        ("\\A", ""),
        ("\\z", ""),
        ("\\A", "job-worker-count=1\nbackground-job-worker-count=1\n😀尾\n"),
        ("\\z", "job-worker-count=1\nbackground-job-worker-count=1\n😀尾\n"),
    ].into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("rudroid-scanner-{}-{index}.txt", std::process::id()));
        std::fs::write(&path, input.as_bytes()).unwrap();
        let fd_count = || std::fs::read_dir("/proc/self/fd").unwrap().filter_map(Result::ok)
            .filter(|entry| std::fs::read_link(entry.path()).ok().as_ref() == Some(&path)).count();
        let before = fd_count();
        let path_units: Vec<u16> = path.to_str().unwrap().encode_utf16().collect();
        let name = s.new_string_units(&path_units);
        // SAFETY: method descriptors and jvalue arities were checked above; all refs are live.
        let stream = unsafe { shims::new_object_a(env, stream_class, stream_ctor, [JValue { l: name }].as_ptr()) };
        // SAFETY: both constructor object arguments and receiver class stay live for this call.
        let scanner = unsafe { shims::new_object_a(env, scanner_class, scanner_ctor,
            [JValue { l: stream }, JValue { l: encoding }].as_ptr()) };
        assert!(!scanner.is_null());
        assert!(!s.with_thread(|t| t.pending.is_some()));
        assert_eq!(fd_count(), before + 1);
        s.delete_ref(stream, false, "Scanner retains source");
        s.delete_ref(name, false, "test path");
        assert_eq!(fd_count(), before + 1, "the Scanner owns the last source reference");
        let pattern = s.new_string(delimiter.as_bytes());
        // SAFETY: the live Scanner and String match useDelimiter's one-object descriptor.
        let returned = unsafe { shims::call_object_method_a(env, scanner, delimiter_method, [JValue { l: pattern }].as_ptr()) };
        assert!(!returned.is_null(), "delimiter {delimiter} was rejected");
        assert_eq!(s.resolve(returned), s.resolve(scanner));
        s.delete_ref(pattern, false, "test delimiter");
        s.delete_ref(scanner, false, "returned Scanner keeps ownership");
        // SAFETY: hasNext is a validated zero-argument boolean method on the live returned Scanner.
        assert_eq!(unsafe { shims::call_boolean_method_a(env, returned, has_next, std::ptr::null()) }, u8::from(!input.is_empty()));
        if !input.is_empty() {
            // SAFETY: next is a validated zero-argument String-returning method.
            let text = unsafe { shims::call_object_method_a(env, returned, next, std::ptr::null()) };
            assert_eq!(s.with_string_units(text, |units| String::from_utf16(units).unwrap()).as_deref(), Some(input));
            s.delete_ref(text, false, "test token");
            // SAFETY: as above; next consumed the complete stream without dropping its final newline.
            assert_eq!(unsafe { shims::call_boolean_method_a(env, returned, has_next, std::ptr::null()) }, 0);
        }
        // SAFETY: close is a validated zero-argument void method on the live Scanner.
        unsafe { shims::call_void_method_a(env, returned, close, std::ptr::null()) };
        assert_eq!(fd_count(), before);
        assert!(!s.with_thread(|t| t.pending.is_some()));
        s.delete_ref(returned, false, "test Scanner");
        std::fs::remove_file(path).unwrap();
    }
    for handle in [encoding, scanner_class, stream_class] { s.delete_ref(handle, false, "test class/encoding"); }
}

#[test]
fn registered_object_int_dispatch_matches_a_and_native_variadic_calls() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();let env=abi::env_ptr();
    unsafe extern "C" fn indexed(env:*mut c_void,receiver:*mut c_void,index:i32)->*mut c_void{if index==-1234567{shims::new_local_ref(env,receiver)}else{std::ptr::null_mut()}}
    let class=s.find_class("test/IndexedObject").unwrap();let receiver=s.alloc_object(class);
    let binding=JniNativeMethod{name:c"get".as_ptr(),signature:c"(I)Ljava/lang/Object;".as_ptr(),fn_ptr:indexed as *mut c_void};assert_eq!(unsafe{s.register_natives(class,&binding,1)},abi::JNI_OK);
    let method=s.resolve_method(class,"get","(I)Ljava/lang/Object;","test").unwrap();
    let result=unsafe{shims::call_object_method_a(env,receiver,method,[JValue{i:-1234567}].as_ptr())};assert_eq!(s.resolve(result),s.resolve(receiver));s.delete_ref(result,false,"test");
    #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))]unsafe{
        let table=abi::tables_for_tests().1;let call:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...)->*mut c_void=std::mem::transmute(table[abi::env_slot::CallObjectMethod]);let result=call(env,receiver,method,-1234567i32);assert_eq!(s.resolve(result),s.resolve(receiver));s.delete_ref(result,false,"test");
    }
    assert!(!s.with_thread(|t|t.pending.is_some()));for handle in [receiver,class]{s.delete_ref(handle,false,"test");}
}
#[test]
fn reflected_constructor_identity_and_primitive_boxes_use_vm_owned_fields(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();let env=abi::env_ptr();
    let class=s.find_class("java/lang/Boolean").unwrap();let ctor=s.resolve_method(class,"<init>","(Z)V","test").unwrap();let method=s.resolve_method(class,"booleanValue","()Z","test").unwrap();
    let reflected=shims::to_reflected_method(env,class,ctor,0);assert_eq!(s.class_name(s.object_class(reflected,"test").unwrap()).as_ref(),"java/lang/reflect/Constructor");
    for value in [0u8,1]{let object=unsafe{shims::new_object_a(env,class,ctor,[JValue{z:value}].as_ptr())};assert!(!object.is_null());assert_eq!(unsafe{shims::call_boolean_method_a(env,object,method,std::ptr::null())},value);s.delete_ref(object,false,"test");}
    let integer_class = s.find_class("java/lang/Integer").unwrap();
    let integer = s.alloc_object(integer_class);
    for value in [i32::MIN, -1, 0, i32::MAX] {
        shims::integer_init(env, integer, value);
        assert_eq!(shims::integer_value(env, integer), value);
    }
    let long_class = s.find_class("java/lang/Long").unwrap();
    let long = s.alloc_object(long_class);
    for value in [i64::MIN, -1, 0, i64::MAX] {
        shims::long_init(env, long, value);
        assert_eq!(shims::long_value(env, long), value);
    }
    for handle in [integer, integer_class, long, long_class] { s.delete_ref(handle, false, "test"); }
    assert!(!s.with_thread(|t|t.pending.is_some()));for handle in [reflected,class]{s.delete_ref(handle,false,"test");}
}

#[test]
fn unity_reflection_object_hint_reuses_declared_field_and_method_ids() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let base = s.find_class("test/ReflectionBase").unwrap();
    let object = s.find_class("java/lang/Object").unwrap();
    s.set_superclass(base, object);
    let child = s.find_class("test/ReflectionChild").unwrap();
    s.set_superclass(child, base);
    let class = s.resolve_class(base, "test").unwrap();
    let id = s.define_field(class, "currentActivity", "Ljava/lang/Object;", true).unwrap();
    let value = s.alloc_object(child);
    s.set_field(base, make_handle(TAG_FIELD, id), fields::FieldValue::Object(s.resolve(value)), true);
    assert_eq!(s.reflected_field_id(child, "currentActivity", "Ljava/lang/Object;", true), Some(id));
    let activity = s.find_class("android/app/Activity").unwrap();
    let activity_id = s.define_field(class, "typedActivity", "Landroid/app/Activity;", true).unwrap();
    let activity_obj = s.alloc_object(activity);
    s.set_field(base, make_handle(TAG_FIELD, activity_id), fields::FieldValue::Object(s.resolve(activity_obj)), true);
    assert_eq!(s.reflected_field_id(child, "typedActivity", "Ljava/lang/Object;", true), Some(activity_id));
    assert_eq!(s.get_field(child, make_handle(TAG_FIELD, activity_id), fields::FieldValue::Object(None), true), Some(fields::FieldValue::Object(s.resolve(activity_obj))));
    // Release the temporary snapshot root.
    s.release_target(s.resolve(activity_obj).unwrap());
    assert!(s.reflected_field_id(child, "typedActivity", "I", true).is_none()); s.take_pending();
    unsafe extern "C" fn callback(_: *mut c_void, _: *mut c_void) -> *mut c_void { std::ptr::null_mut() }
    let methods = [JniNativeMethod { name:c"getActivity".as_ptr(), signature:c"()Landroid/app/Activity;".as_ptr(), fn_ptr:callback as *mut c_void }];
    unsafe { assert_eq!(s.register_natives(base, methods.as_ptr(), 1), abi::JNI_OK); }
    let exact = s.resolve_method(base, "getActivity", "()Landroid/app/Activity;", "test").unwrap();
    assert_eq!(s.reflected_method_id(child, "getActivity", "()Ljava/lang/Object;"), Some(exact));
    assert!(s.reflected_method_id(child, "getActivity", "(I)Ljava/lang/Object;").is_none()); s.take_pending();
    assert!(s.reflected_method_id(child, "<init>", "()Ljava/lang/Object;").is_none());
}

#[test]
fn int_array_leases_are_owned_and_commit_abort_and_bounds_are_checked() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let array = s.new_int_array(2);
    let other = s.new_int_array(0);
    assert_eq!(s.array_length(array), 2);
    let source = [i32::MIN, i32::MAX];
    unsafe { s.int_region(array, 0, 2, source.as_ptr().cast_mut(), true); }
    let lease = s.borrow_int_array(array);
    assert!(!lease.is_null());
    unsafe {
        assert_eq!(*lease, i32::MIN); *lease = 17;
        s.release_int_array(other, lease, 0);
        assert!(s.take_pending().is_some());
        s.release_int_array(array, lease, 1);
        let mut result = [0;2]; s.int_region(array,0,2,result.as_mut_ptr(),false); assert_eq!(result,[17,i32::MAX]);
        *lease = 99; s.release_int_array(array,lease,2);
        s.int_region(array,0,2,result.as_mut_ptr(),false); assert_eq!(result,[17,i32::MAX]);
        s.int_region(array, i32::MAX, 2, result.as_mut_ptr(), false); assert!(s.take_pending().is_some());
    }
    assert!(s.primitive_leases.lock().unwrap().is_empty());
    s.delete_ref(array,false,"test"); s.delete_ref(other,false,"test"); assert!(s.arrays.lock().unwrap().is_empty());
}

#[test]
fn float_native_calls_use_the_float_register_abi_and_revoke_cached_bindings() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host()); let s = session().unwrap(); s.take_pending();
    unsafe extern "C" fn rate(_: *mut c_void,_: *mut c_void) -> f32 { 59.94 }
    let class = s.find_class("test/FloatRate").unwrap(); let object = s.alloc_object(class);
    let native = [JniNativeMethod{name:c"rate".as_ptr(),signature:c"()F".as_ptr(),fn_ptr:rate as *mut c_void}];
    unsafe {
        assert_eq!(s.register_natives(class,native.as_ptr(),1),abi::JNI_OK);
        let method = s.resolve_method(class,"rate","()F","test").unwrap();
        assert_eq!(shims::call_float_method_a(abi::env_ptr(),object,method,std::ptr::null()),59.94);
        #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))] {
            let call: unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...) -> f32 = std::mem::transmute((*(abi::env_ptr() as *const *const usize)).add(abi::env_slot::CallFloatMethod).read());
            assert_eq!(call(abi::env_ptr(),object,method),59.94);
        }
        s.unregister_natives(class);
        assert_eq!(shims::call_float_method_a(abi::env_ptr(),object,method,std::ptr::null()),0.0);
        assert!(s.take_pending().is_some());
    }
    s.delete_ref(object,false,"test"); s.delete_ref(class,false,"test");
}

#[test]
fn float_native_integer_arguments_preserve_coordinates_and_missing_arguments_fail() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    unsafe extern "C" fn coordinate(_: *mut c_void,_: *mut c_void,index:i32)->f32 {index as f32+0.25}
    unsafe extern "C" fn axis(_: *mut c_void,_: *mut c_void,axis:i32,index:i32)->f32 {axis as f32*100.0+index as f32+0.5}
    let class=s.find_class("test/InputFloatCoordinates").unwrap();let object=s.alloc_object(class);
    let methods=[JniNativeMethod{name:c"coordinate".as_ptr(),signature:c"(I)F".as_ptr(),fn_ptr:coordinate as *mut c_void},JniNativeMethod{name:c"axis".as_ptr(),signature:c"(II)F".as_ptr(),fn_ptr:axis as *mut c_void}];
    unsafe {
        assert_eq!(s.register_natives(class,methods.as_ptr(),2),abi::JNI_OK);
        for (name,sig,expected) in [("coordinate","(I)F",-2.75),("axis","(II)F",-292.5)] {
            let method=s.resolve_method(class,name,sig,"test").unwrap();let args=[JValue{i:-3},JValue{i:7}];
            assert_eq!(shims::call_float_method_a(abi::env_ptr(),object,method,args.as_ptr()),expected);
            #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))] {
                let call:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...)->f32=std::mem::transmute((*(abi::env_ptr() as *const *const usize)).add(abi::env_slot::CallFloatMethod).read());
                assert_eq!(call(abi::env_ptr(),object,method,-3i32,7i32),expected);
            }
            assert_eq!(shims::call_float_method_a(abi::env_ptr(),object,method,std::ptr::null()),0.0);assert!(s.take_pending().is_some());
        }
        let cached=s.resolve_method(class,"axis","(II)F","test").unwrap();s.unregister_natives(class);assert_eq!(shims::call_float_method_a(abi::env_ptr(),object,cached,[JValue{i:0},JValue{i:0}].as_ptr()),0.0);assert!(s.take_pending().is_some());
    }
    s.delete_ref(object,false,"test");s.delete_ref(class,false,"test");
}

#[test]
fn two_integer_native_calls_keep_both_values_and_revoke_cached_method_ids() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host()); let s = session().unwrap(); s.take_pending();
    unsafe extern "C" fn combine(_: *mut c_void, _: *mut c_void, a: i32, b: i32) -> i32 { a * 100 + b }
    let class = s.find_class("test/TwoIntegerNative").unwrap(); let object = s.alloc_object(class);
    let native = [JniNativeMethod { name: c"combine".as_ptr(), signature: c"(II)I".as_ptr(), fn_ptr: combine as *mut c_void }];
    unsafe {
        assert_eq!(s.register_natives(class, native.as_ptr(), 1), abi::JNI_OK);
        let method = s.resolve_method(class, "combine", "(II)I", "test").unwrap();
        let args = [JValue { i: -3 }, JValue { i: 7 }];
        assert_eq!(shims::call_int_method_a(abi::env_ptr(), object, method, args.as_ptr()), -293);
        #[cfg(all(target_arch="aarch64", target_os="linux", target_env="gnu"))] {
            let call: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, ...) -> i32 =
                std::mem::transmute((*(abi::env_ptr() as *const *const usize)).add(abi::env_slot::CallIntMethod).read());
            assert_eq!(call(abi::env_ptr(), object, method, -3i32, 7i32), -293);
        }
        assert_eq!(shims::call_int_method_a(abi::env_ptr(), object, method, std::ptr::null()), 0);
        assert!(s.take_pending().is_some());
        s.unregister_natives(class);
        assert_eq!(shims::call_int_method_a(abi::env_ptr(), object, method, args.as_ptr()), 0);
        assert!(s.take_pending().is_some());
    }
    s.delete_ref(object, false, "test"); s.delete_ref(class, false, "test");
}

#[test]
fn mixed_reference_primitive_reference_native_is_never_called_with_two_reference_prototype() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    static CALLED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
    unsafe extern "C" fn mixed(_: *mut c_void,_:*mut c_void,_:*mut c_void,_:i32,_:*mut c_void){CALLED.store(true,std::sync::atomic::Ordering::Relaxed);}
    let class=s.find_class("test/MixedPrototype").unwrap();let object=s.alloc_object(class);
    let native=[JniNativeMethod{name:c"mixed".as_ptr(),signature:c"(Ljava/lang/Object;ILjava/lang/Object;)V".as_ptr(),fn_ptr:mixed as *mut c_void}];
    unsafe {s.register_natives(class,native.as_ptr(),1);let method=s.resolve_method(class,"mixed","(Ljava/lang/Object;ILjava/lang/Object;)V","test").unwrap();shims::call_void_method_a(abi::env_ptr(),object,method,[JValue{l:object},JValue{i:17},JValue{l:object}].as_ptr());}
    assert!(!CALLED.load(std::sync::atomic::Ordering::Relaxed));assert!(s.take_pending().is_some());s.delete_ref(object,false,"test");s.delete_ref(class,false,"test");
}

#[test]
fn reflected_object_hint_finds_interface_methods_and_static_fields_without_alias_storage() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=local_session();
    let interface=s.find_class("test/ReflectionInterface").unwrap();let class=s.find_class("test/ReflectionImplementation").unwrap();
    let interface_id=s.resolve_class(interface,"test").unwrap();let class_id=s.resolve_class(class,"test").unwrap();
    s.classes.lock().unwrap().interfaces.insert(class_id,vec![interface_id]);
    unsafe extern "C" fn getter(_: *mut c_void,_:*mut c_void)->*mut c_void{std::ptr::null_mut()}
    let native=[JniNativeMethod{name:c"getValue".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:getter as *mut c_void}];
    unsafe{s.register_natives(interface,native.as_ptr(),1);}
    let exact=s.resolve_method(class,"getValue","()Ljava/lang/String;","test").unwrap();
    assert_eq!(s.reflected_method_id(class,"getValue","()Ljava/lang/Object;"),Some(exact));
    let field=s.define_field(interface_id,"TOKEN","Ljava/lang/String;",true).unwrap();
    assert_eq!(s.reflected_field_id(class,"TOKEN","Ljava/lang/Object;",true),Some(field));
    assert_eq!(s.resolve_field(class,"TOKEN","Ljava/lang/String;",true),make_handle(TAG_FIELD,field));
}

#[test]
fn vm_owned_direct_buffer_survives_local_release_and_is_freed_with_final_jni_root() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    let before=s.objects.lock().unwrap().len();let buffer=s.allocate_direct_buffer(16);let target=s.resolve(buffer).unwrap();let global=s.new_ref(target,true);let weak=shims::new_weak_global_ref(abi::env_ptr(),buffer);
    let address=s.direct_buffer_address(buffer).cast::<u8>();assert!(!address.is_null());assert_eq!(s.direct_buffer_capacity(buffer),16);
    unsafe {address.write(0x7f);address.add(15).write(0x80);}
    s.delete_ref(buffer,false,"test");assert_eq!(s.direct_buffer_address(global),address.cast());unsafe{assert_eq!(address.read(),0x7f);assert_eq!(address.add(15).read(),0x80);}
    s.delete_ref(global,true,"test");assert!(s.resolve(weak).is_none());assert_eq!(s.objects.lock().unwrap().len(),before);assert_eq!(s.direct_buffer_capacity(weak),-1);
    {shims::delete_weak_global_ref(abi::env_ptr(),weak);};assert!(s.allocate_direct_buffer(-1).is_null());assert!(s.take_pending().is_some());
}

#[test]
fn reflected_binary_argument_names_resolve_subclasses_to_the_original_method_id(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=local_session();
    let context=s.find_class("android/content/Context").unwrap();let activity=s.find_class("android/app/Activity").unwrap();s.set_superclass(activity,context);
    let class=s.find_class("test/ReflectionArguments").unwrap();
    unsafe extern "C" fn init(_:*mut c_void,_:*mut c_void,_:*mut c_void){}
    let bindings=[JniNativeMethod{name:c"initialize".as_ptr(),signature:c"(Landroid/content/Context;)V".as_ptr(),fn_ptr:init as *mut c_void}];
    unsafe{assert_eq!(s.register_natives(class,bindings.as_ptr(),1),abi::JNI_OK);}
    let exact=s.resolve_method(class,"initialize","(Landroid/content/Context;)V","test").unwrap();
    assert_eq!(s.reflected_method_id(class,"initialize","(Landroid.app.Activity;)V"),Some(exact));
    assert!(s.reflected_method_id(class,"initialize","(Ljava.lang.String;)V").is_none());s.take_pending();
    // Raw JNI remains strict: only the reflection facade accepts binary names.
    assert!(s.resolve_method(class,"initialize","(Landroid.app.Activity;)V","test").is_none());
}

#[test]
fn inherited_exact_overload_beats_an_assignable_child_overload(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=local_session();
    let base=s.find_class("test/OverloadBase").unwrap();let child=s.find_class("test/OverloadChild").unwrap();s.set_superclass(child,base);
    unsafe extern "C" fn method(_:*mut c_void,_:*mut c_void,_:*mut c_void){}
    let bindings=[JniNativeMethod{name:c"f".as_ptr(),signature:c"(Ljava/lang/String;)V".as_ptr(),fn_ptr:method as *mut c_void}];
    let general=[JniNativeMethod{name:c"f".as_ptr(),signature:c"(Ljava/lang/Object;)V".as_ptr(),fn_ptr:method as *mut c_void}];
    unsafe{s.register_natives(base,bindings.as_ptr(),1);s.register_natives(child,general.as_ptr(),1);}
    let exact=s.resolve_method(base,"f","(Ljava/lang/String;)V","test").unwrap();
    assert_eq!(s.reflected_method_id(child,"f","(Ljava.lang.String;)V"),Some(exact));
}

#[test]
fn cold_catalog_exact_overload_has_the_same_identity_as_a_prewarmed_lookup(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    for warm in [false,true]{
        let s=local_session();let base=s.find_class("test/CatalogOverloadBase").unwrap();let child=s.find_class("test/CatalogOverloadChild").unwrap();s.set_superclass(child,base);
        test_support::stub_set("test/CatalogOverloadBase","f","(Ljava/lang/String;)I",MethodValue::Int(37));
        unsafe extern "C" fn generic(_:*mut c_void,_:*mut c_void,_:*mut c_void)->i32{99}
        let bindings=[JniNativeMethod{name:c"f".as_ptr(),signature:c"(Ljava/lang/Object;)I".as_ptr(),fn_ptr:generic as *mut c_void}];
        unsafe{s.register_natives(child,bindings.as_ptr(),1);}
        if warm{s.resolve_method(base,"f","(Ljava/lang/String;)I","test").unwrap();}
        let reflected=s.reflected_method_id(child,"f","(Ljava.lang.String;)I").unwrap();
        assert_eq!(Some(reflected),s.resolve_method(base,"f","(Ljava/lang/String;)I","test"));
        assert_eq!(s.call_checked(base,reflected,abi::MethodKind::Int,"test", false),Some(MethodValue::Int(37)));
        assert!(s.take_pending().is_none());
    }test_support::stub_clear();
}

#[test]
fn mixed_native_layout_preserves_register_and_stack_arguments(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    static OBSERVED:AtomicU64=AtomicU64::new(0);
    unsafe extern "C" fn observe(env: *mut c_void,object:*mut c_void,a:*mut c_void,b:*mut c_void,c:u8,d:u8,e:*mut c_void,f:*mut c_void,g:u8,h:*mut c_void,i:u8){
        // SAFETY: actual shared VM; compare Java identity rather than the
        // distinct local-reference tokens owned by the automatic native frame.
        let env=rudroid_common::jni_env::JniEnv::from_raw(env);
        if env.is_same_object(a,object)&&b.is_null()&&c==1&&d==0&&env.is_same_object(e,object)&&f.is_null()&&g==1&&env.is_same_object(h,object)&&i==0{OBSERVED.fetch_add(1,Ordering::Relaxed);}
    }
    let class=s.find_class("test/MixedStackArguments").unwrap();let object=s.alloc_object(class);
    let signature=c"(Ljava/lang/Object;Ljava/lang/String;ZZ[ILjava/lang/String;ZLjava/lang/Object;Z)V";
    let native=[JniNativeMethod{name:c"stack".as_ptr(),signature:signature.as_ptr(),fn_ptr:observe as *mut c_void}];
    let args=[JValue{l:object},JValue{l:std::ptr::null_mut()},JValue{z:1},JValue{z:0},JValue{l:object},JValue{l:std::ptr::null_mut()},JValue{z:1},JValue{l:object},JValue{z:0}];
    let before=OBSERVED.load(Ordering::Relaxed);
    unsafe{assert_eq!(s.register_natives(class,native.as_ptr(),1),abi::JNI_OK);let method=s.resolve_method(class,"stack",signature.to_str().unwrap(),"test").unwrap();shims::call_void_method_a(abi::env_ptr(),object,method,args.as_ptr());
        #[cfg(all(target_arch="aarch64",target_os="linux",target_env="gnu"))]{
            let call:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,...)=std::mem::transmute((*(abi::env_ptr() as *const *const usize)).add(abi::env_slot::CallVoidMethod).read());
            call(abi::env_ptr(),object,method,object,std::ptr::null_mut::<c_void>(),1i32,0i32,object,std::ptr::null_mut::<c_void>(),1i32,object,0i32);
        }
    }
    let count=if cfg!(all(target_arch="aarch64",target_os="linux",target_env="gnu")){2}else{1};
    assert_eq!(OBSERVED.load(Ordering::Relaxed),before+count);assert!(s.take_pending().is_none());
    for malformed in ["(IL;)V","(ILtest/A;)VX","(I[V)V"]{assert!(mixed_void_native(malformed,1).is_none());}
    s.delete_ref(object,false,"test");s.delete_ref(class,false,"test");
}

#[test]
fn complete_reference_native_descriptors_reject_mixed_and_malformed_shapes(){
    for signature in ["()[B", "()[Z", "()[C", "()[S", "()[J", "()[F", "()[D", "()[[B", "()Ljava/lang/Object;"] {
        assert!(matches!(reference_native(signature, 1), Some(MethodValue::NativeObject(1, ObjectShape::NoArgs))), "{signature}");
    }
    for signature in ["()I", "()V", "()[V", "()[Bjunk"] { assert!(reference_native(signature, 1).is_none(), "{signature}"); }
    for valid in ["(Ltest/A;)V","(Ltest/A;[I)Ltest/B;","(Ltest/A;[I[[Ljava/lang/String;)V"]{assert!(reference_native(valid,1).is_some(),"{valid}");}
    for invalid in ["(Ljava/lang/Object;ILjava/lang/Object;)V","(Ljava/lang/Object;I[I;)V","(L;Ltest/B;)V","(Ltest/A;)Ltest/B;junk","(Ltest/A;[V)V","(Ltest/A;Ltest/B;Ltest/C;Ltest/D;)V"]{assert!(reference_native(invalid,1).is_none(),"{invalid}");}
}

#[test]
fn empty_string_constructor_returns_a_real_immutable_jni_string(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    let class=s.find_class("java/lang/String").unwrap();let ctor=s.resolve_method(class,"<init>","()V","test").unwrap();
    unsafe{let value=shims::new_object_a(abi::env_ptr(),class,ctor,std::ptr::null());assert_eq!(shims::get_string_length(abi::env_ptr(),value),0);let chars=shims::get_string_utf_chars(abi::env_ptr(),value,std::ptr::null_mut());assert_eq!(CStr::from_ptr(chars),c"");shims::release_string_utf_chars(abi::env_ptr(),value,chars);s.delete_ref(value,false,"test");}
    s.delete_ref(class,false,"test");assert!(s.take_pending().is_none());
}

#[test]
fn string_byte_conversions_roundtrip_utf8_including_nul_and_supplementary_characters(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());init_with_host(test_support::stub_host());let s=session().unwrap();s.take_pending();
    let env=abi::env_ptr();let units:Vec<u16>="a\0🎮é".encode_utf16().collect();
    let class=s.find_class("java/lang/String").unwrap();let method=s.resolve_method(class,"getBytes","(Ljava/lang/String;)[B","test").unwrap();
    unsafe{let string=shims::new_string(env,units.as_ptr(),units.len() as i32);let encoding=s.new_string(b"UTF-8");let bytes=shims::call_object_method_a(env,string,method,[JValue{l:encoding}].as_ptr());
        let mut result=vec![0i8;s.array_length(bytes) as usize];s.byte_region(bytes,0,result.len() as i32,result.as_mut_ptr(),false);
        assert_eq!(result.iter().map(|v|*v as u8).collect::<Vec<_>>(),"a\0🎮é".as_bytes());
        let ctor = s.resolve_method(class, "<init>", "([BLjava/lang/String;)V", "test").unwrap();
        let borrows = s.string_chars.lock().unwrap().len();
        let restored = shims::new_object_a(env, class, ctor, [JValue { l: bytes }, JValue { l: encoding }].as_ptr());
        assert_eq!(s.with_string_units(restored, |value| value.to_vec()).unwrap(), units);
        assert_eq!(s.string_chars.lock().unwrap().len(), borrows);
        let unsupported = s.new_string(b"UTF-16");
        assert!(shims::new_object_a(env, class, ctor, [JValue { l: bytes }, JValue { l: unsupported }].as_ptr()).is_null());
        assert_eq!(s.take_pending().unwrap().class, "java/io/UnsupportedEncodingException");
        assert!(shims::new_object_a(env, class, ctor, [JValue { l: bytes }, JValue { l: std::ptr::null_mut() }].as_ptr()).is_null());
        assert_eq!(s.take_pending().unwrap().class, "java/lang/StringIndexOutOfBoundsException");
        assert_eq!(s.string_chars.lock().unwrap().len(), borrows);
        for h in [class,string,encoding,bytes,restored,unsupported]{s.delete_ref(h,false,"test");}
    }assert!(s.take_pending().is_none());
}

#[test]
fn native_registration_rejects_jni_tokens_instead_of_executing_them_as_callbacks(){
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=local_session();
    let class=s.find_class("test/InvalidCallback").unwrap();let class_id=s.resolve_class(class,"test").unwrap();
    let field=s.define_field(class_id,"number","I",true).unwrap();
    unsafe extern "C" fn valid(_:*mut c_void,_:*mut c_void){}
    let binding=JniNativeMethod{name:c"valid".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:valid as *mut c_void};
    unsafe{assert_eq!(s.register_natives(class,&binding,1),abi::JNI_OK);}
    let method=s.resolve_method(class,"valid","()V","test").unwrap();
    for tag in [TAG_LOCAL,TAG_GLOBAL,TAG_WEAK,TAG_METHOD,TAG_FIELD]{
        let bad=JniNativeMethod{name:c"bad".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:make_handle(tag,handle_parts(if tag==TAG_METHOD{method}else if tag==TAG_FIELD{make_handle(TAG_FIELD,field)}else{class}).1)};
        unsafe{assert_eq!(s.register_natives(class,&bad,1),abi::JNI_ERR);}
        assert!(s.take_pending().is_some());
        assert!(s.methods.lock().unwrap().find(class_id,"bad","()V").is_none());
    }s.delete_ref(class,false,"test");
}

#[test]
fn internal_string_reads_preserve_encodings_and_leave_no_native_borrow_pins() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = local_session();
    let units = [b'A' as u16, 0, 0xd83d, 0xde00];
    let text = s.new_string_units(&units);
    let pins = s.liveness.lock().unwrap().clone();
    let copied = s.with_string_units(text, <[u16]>::to_vec).unwrap();
    assert_eq!(copied, units);
    assert_eq!(s.with_string_bytes(text, <[u8]>::to_vec).unwrap(),
        [b'A', 0xc0, 0x80, 0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]);
    assert_eq!(*s.liveness.lock().unwrap(), pins);
    assert!(s.string_chars.lock().unwrap().is_empty(), "internal reads create no native leases");
    s.delete_ref(text, false, "test");
    assert!(s.strings.lock().unwrap().is_empty());
    assert_eq!(copied, units, "owned result survives deletion of its JNI source");

    let literal_nul = s.new_string(b"before\0after");
    assert_eq!(s.with_string_bytes(literal_nul, <[u8]>::to_vec).unwrap(), b"before",
        "byte view keeps the original C-string prefix semantics");
    s.delete_ref(literal_nul, false, "test");
    assert!(s.with_string_bytes(text, |_| ()).is_none(), "deleted handle is rejected");
    assert!(s.take_pending().is_some());
    assert!(s.with_string_units(std::ptr::null_mut(), |_| ()).is_none());
    assert!(s.take_pending().is_some());
    assert!(s.liveness.lock().unwrap().is_empty());
}
