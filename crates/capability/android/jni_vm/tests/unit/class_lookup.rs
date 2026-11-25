use super::*;
use super::super::test_support;

fn local() -> Session {
    let s=Session::new();s.install_host(test_support::stub_host());s
}
fn pending(s:&Session)->String {s.with_thread(|t|t.pending.as_ref().unwrap().class.to_string())}

#[test]
fn unknown_queries_never_create_classes_and_use_each_entrys_exception() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local();assert!(s.install_class_catalog(vec!["sample/Present".into()]));
    for java in [false,true] {
        assert!(s.lookup_class("sample/Absent",java).is_none());
        assert_eq!(pending(&s),if java{"java/lang/ClassNotFoundException"}else{"java/lang/NoClassDefFoundError"});
        assert!(!s.classes.read().unwrap().by_name.contains_key("sample/Absent"));
        s.take_pending();
    }
    let a=s.lookup_class("sample/Present",false).unwrap();
    let b=s.lookup_class("sample/Present",true).unwrap();
    assert_eq!(s.resolve(a),s.resolve(b));
}

#[test]
fn array_lookup_checks_components_without_creating_missing_components() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local();assert!(s.install_class_catalog(vec!["sample/Present".into()]));
    for name in ["[Lsample/Absent;","[[Lsample/Absent;","[V","[Q","[L;","[L[I;","[L[[I;","sample.Absent",""] {
        assert!(s.lookup_class(name,false).is_none(),"{name}");s.take_pending();
        assert!(!s.classes.read().unwrap().by_name.contains_key(name));
    }
    assert!(!s.classes.read().unwrap().by_name.contains_key("sample/Absent"));
    for name in ["[I","[[Z","[[Lsample/Present;"] {assert!(s.lookup_class(name,false).is_some(),"{name}");}
}

#[test]
fn trusted_declarations_and_default_legacy_lookup_remain_available() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local();assert!(s.lookup_class("provided/Early",false).is_some());
    assert!(s.install_class_catalog(vec!["sample/Present".into()]));
    assert!(s.lookup_class("provided/Early",false).is_some());
    let declared=s.find_class("provided/Late").unwrap();
    let looked_up=s.lookup_class("provided/Late",true).unwrap();
    assert_eq!(s.resolve(declared),s.resolve(looked_up));
}

#[test]
fn invalid_or_conflicting_catalog_never_replaces_first_policy() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local();
    assert!(!s.install_class_catalog(vec!["sample/Good".into(),"bad.name".into()]));
    assert!(s.classes.read().unwrap().lookup_catalog.is_none());s.take_pending();
    assert!(s.install_class_catalog(vec!["sample/Present".into()]));
    assert!(s.install_class_catalog(vec!["sample/Present".into(),"sample/Present".into()]));
    assert!(!s.install_class_catalog(vec!["sample/Different".into()]));s.take_pending();
    assert!(s.lookup_class("sample/Different",false).is_none());
    assert_eq!(pending(&s),"java/lang/NoClassDefFoundError");
}

#[test]
fn provided_exception_types_are_cold_queryable_with_real_parent_chains() {
    let _serial=test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    let s=local();bootstrap_exceptions(&s);
    assert!(s.install_class_catalog(vec!["sample/Present".into()]));
    let class_id=|name:&str| {
        let local=s.lookup_class(name,false).expect("bootstrap type before any exception inspection");
        let id=s.resolve_class(local,"test exception type").unwrap();
        s.delete_ref(local,false,"test exception type");id
    };
    let missing=class_id("java/lang/NoSuchMethodError");
    let error=class_id("java/lang/Error");let exception=class_id("java/lang/Exception");
    let throwable=class_id("java/lang/Throwable");let not_found=class_id("java/lang/ClassNotFoundException");
    assert!(s.is_subclass(missing,error));assert!(s.is_subclass(missing,throwable));
    assert!(!s.is_subclass(missing,exception));assert!(s.is_subclass(not_found,exception));
    assert!(!s.is_subclass(not_found,error));assert!(s.with_thread(|t|t.pending.is_none()));
}
