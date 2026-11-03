//! INPUT:  extracted-asset resolver and host cwd
//! OUTPUT:  namespace, traversal and native-path regression tests
//! POS:    checks URI mapping without changing process cwd or global state
use super::*;

#[test]
fn empty_archive_and_android_asset_use_the_extracted_cwd() {
    let expected = std::env::current_dir().unwrap().join("assets/aa/settings.json");
    for uri in [c"jar:file://!/assets/aa/settings.json", c"jar:file:/!/assets/aa/settings.json",
        c"/android_asset/aa/settings.json", c"file:///android_asset/aa/settings.json",
        c"/assets/aa/settings.json"] {
        assert_eq!(extracted_asset(uri).unwrap().to_bytes(), expected.as_os_str().as_bytes());
    }
    let prefixed = CString::new(format!("{}/jar:file://!/assets/aa/settings.json", std::env::current_dir().unwrap().display())).unwrap();
    assert_eq!(extracted_asset(&prefixed).unwrap().to_bytes(), expected.as_os_str().as_bytes());
    assert_eq!(extracted_asset(c"jar:file:///tmp/game/base.apk!/assets/aa/settings.json").unwrap().as_c_str(),
        c"/tmp/game/assets/aa/settings.json");
}

#[test]
fn ordinary_files_and_invalid_asset_names_are_not_rewritten() {
    for uri in [c"/tmp/settings.json", c"assets/aa/settings.json", c"relative.txt",
        c"jar:file://!/assets/../save", c"jar:file://!/assets//tmp/save",
        c"jar:file://!/assets/aa/../../save", c"jar:file://!/assets/aa\\save",
        c"jar:file://!/assets/", c"jar:file://!/other/aa/settings.json"] {
        assert!(extracted_asset(uri).is_none(), "{uri:?}");
    }
}
