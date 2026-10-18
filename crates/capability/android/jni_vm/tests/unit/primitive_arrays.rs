//! INPUT:  all primitive POD kinds, copy/critical leases and shared JNI root identities
//! OUTPUT:  precision, bounds, commit/abort, growth, pin and rejected-release regressions
//! POS:    adversarial tests of the converged primitive array owner
use super::*;
use crate::session::test_support;
fn session() -> Session {
    let s = Session::new();
    s.install_host(test_support::stub_host());
    s
}
#[test]
fn every_primitive_kind_preserves_bits_regions_and_release_modes() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = session();
    macro_rules! check {($kind:ident,$ty:ty,$input:expr)=>{{
        let input:[$ty;3]=$input;let a=s.new_primitive_array(Kind::$kind,3);assert!(!a.is_null());assert_eq!(s.array_length(a),3);
        // SAFETY: exact initialized POD spans; each returned copy has exclusive native access until release.
        unsafe {
            s.primitive_region(a,Kind::$kind,0,3,input.as_ptr().cast_mut().cast(),true);
            let p=s.borrow_primitive_array(a,Some(Kind::$kind),false).cast::<$ty>();assert!(!p.is_null());
            assert_eq!(p as usize%std::mem::align_of::<$ty>(),0);
            let mut expected=[0 as $ty;3];s.primitive_region(a,Kind::$kind,0,3,expected.as_mut_ptr().cast(),false);
            assert_eq!(std::slice::from_raw_parts(expected.as_ptr().cast::<u8>(),std::mem::size_of_val(&expected)),std::slice::from_raw_parts(input.as_ptr().cast::<u8>(),std::mem::size_of_val(&input)));
            *p=0 as $ty;s.release_primitive_array(a,Some(Kind::$kind),p.cast(),1,false);
            s.primitive_region(a,Kind::$kind,0,3,expected.as_mut_ptr().cast(),false);assert_eq!(expected[0],0 as $ty);
            *p=1 as $ty;s.release_primitive_array(a,Some(Kind::$kind),p.cast(),2,false);
            s.primitive_region(a,Kind::$kind,0,3,expected.as_mut_ptr().cast(),false);assert_eq!(expected[0],0 as $ty);
            s.primitive_region(a,Kind::$kind,i32::MAX,3,expected.as_mut_ptr().cast(),false);assert!(s.take_pending().is_some());
            s.primitive_region(a,Kind::$kind,3,0,std::ptr::null_mut(),false);assert!(s.take_pending().is_none());
        }
        s.delete_ref(a,false,"test");assert!(s.primitive_leases.lock().unwrap().is_empty());assert!(s.arrays.lock().unwrap().is_empty());
    }};}
    check!(Boolean, u8, [0, 1, 1]);
    check!(Byte, i8, [i8::MIN, 0, i8::MAX]);
    check!(Char, u16, [0, 0xd800, u16::MAX]);
    check!(Short, i16, [i16::MIN, 0, i16::MAX]);
    check!(Int, i32, [i32::MIN, 0, i32::MAX]);
    check!(Long, i64, [i64::MIN, (1i64 << 53) + 1, i64::MAX]);
    check!(
        Float,
        f32,
        [-0.0, f32::from_bits(0x7fc00001), f32::INFINITY]
    );
    check!(
        Double,
        f64,
        [-0.0, f64::from_bits(0x7ff8000000000001), f64::NEG_INFINITY]
    );
}
#[test]
fn critical_copies_keep_roots_and_addresses_through_ledger_growth_and_commit() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = session();
    let a = s.new_primitive_array(Kind::Long, 2);
    let alias = s.promote_reference(a, true);
    let id = s.resolve(a).unwrap();
    let p = s.borrow_primitive_array(a, None, true).cast::<i64>();
    s.delete_ref(a, false, "test");
    let other = s.new_primitive_array(Kind::Double, 0);
    let mut held = Vec::new();
    for _ in 0..1000 {
        held.push(s.borrow_primitive_array(other, Some(Kind::Double), false));
    }
    // SAFETY: copy remains owned/rooted across map reallocation and caller local-reference removal.
    unsafe {
        *p = (1i64 << 53) + 7;
        s.release_primitive_array(alias, None, p.cast(), 1, true);
        *p.add(1) = i64::MAX;
        s.release_primitive_array(alias, None, p.cast(), 0, true);
        let mut output = [0i64; 2];
        s.primitive_region(alias, Kind::Long, 0, 2, output.as_mut_ptr().cast(), false);
        assert_eq!(output, [(1i64 << 53) + 7, i64::MAX]);
        for pointer in held {
            s.release_primitive_array(other, Some(Kind::Double), pointer, 2, false);
        }
    }
    s.delete_ref(alias, true, "test");
    s.delete_ref(other, false, "test");
    assert!(s.arrays.lock().unwrap().is_empty());
    assert!(!s.liveness.lock().unwrap().contains_key(&match id {
        RefTarget::Array(id) => id,
        _ => unreachable!(),
    }));
}
#[test]
fn invalid_kind_mode_pointer_and_critical_entry_never_retire_a_valid_copy() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = session();
    let a = s.new_primitive_array(Kind::Float, 1);
    let b = s.new_primitive_array(Kind::Float, 1);
    let p = s.borrow_primitive_array(a, None, true);
    assert!(s
        .borrow_primitive_array(a, Some(Kind::Double), false)
        .is_null());
    s.take_pending();
    // SAFETY: rejected forged pointers are checked as ledger keys before any dereference; owned copy stays live.
    unsafe {
        for (array, kind, pointer, mode, critical) in [
            (b, None, p, 0, true),
            (a, Some(Kind::Double), p, 0, true),
            (a, None, p, 9, true),
            (a, None, 1usize as *mut c_void, 0, true),
            (a, None, p, 0, false),
        ] {
            s.release_primitive_array(array, kind, pointer, mode, critical);
            assert!(s.take_pending().is_some());
            assert!(s
                .primitive_leases
                .lock()
                .unwrap()
                .contains_key(&(p as usize)));
        }
        s.release_primitive_array(a, None, p, 2, true);
        s.release_primitive_array(a, None, p, 2, true);
        assert!(s.take_pending().is_some());
    }
    assert!(s.new_primitive_array(Kind::Boolean, -1).is_null());
    s.take_pending();
    s.delete_ref(a, false, "test");
    s.delete_ref(b, false, "test");
    assert!(s.arrays.lock().unwrap().is_empty());
}
