//! INPUT:  private items of `src/catalog.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/catalog.rs`; declared there with `#[path]`
use super::*;

#[test]
fn duplicate_keys_keep_last_value_and_tuple_boundaries() {
    let catalog = Catalog::from_blob(&encode(&[
        Entry { class: "ab", method: "c", sig: "()I", value: ReplyValue::Int(1) },
        Entry { class: "a", method: "bc", sig: "()I", value: ReplyValue::Int(2) },
        Entry { class: "ab", method: "c", sig: "()I", value: ReplyValue::Int(3) },
    ])).unwrap();
    assert_eq!(catalog.len(), 2);
    assert_eq!(catalog.query_int("ab", "c", "()I", 0), 3);
    assert_eq!(catalog.query_int("a", "bc", "()I", 0), 2);
    assert!(Catalog::default().query("a", "bc", "()I").is_none());
}

fn sample_blob() -> Vec<u8> {
    encode(&[
        Entry {
            class: "android/media/AudioManager",
            method: "isBluetoothA2dpOn",
            sig: "()Z",
            value: ReplyValue::Bool(false),
        },
        Entry {
            class: "android/media/AudioManager",
            method: "getStreamVolume",
            sig: "(I)I",
            value: ReplyValue::Int(100),
        },
        Entry {
            class: "x/Y",
            method: "name",
            sig: "()Ljava/lang/String;",
            value: ReplyValue::Str("silksong".to_owned()),
        },
    ])
}

#[test]
fn blob_round_trip_and_query() {
    let catalog = Catalog::from_blob(&sample_blob()).unwrap();
    assert_eq!(catalog.len(), 3);
    assert!(!catalog.query_bool("android/media/AudioManager", "isBluetoothA2dpOn", "()Z", true));
    assert_eq!(
        catalog.query_int("android/media/AudioManager", "getStreamVolume", "(I)I", -1),
        100
    );
    assert_eq!(
        catalog.query_str("x/Y", "name", "()Ljava/lang/String;", "fallback"),
        "silksong"
    );
}

#[test]
fn missing_key_returns_fallback() {
    let catalog = Catalog::from_blob(&sample_blob()).unwrap();
    assert_eq!(
        catalog.query("a/B", "missing", "()V"),
        None,
        "missing-key default is None; typed getters apply the fallback"
    );
    assert!(catalog.query_bool("a/B", "missing", "()V", true));
    assert_eq!(catalog.query_int("a/B", "missing", "()V", 7), 7);
    assert_eq!(catalog.query_str("a/B", "missing", "()V", "dflt"), "dflt");
}

#[test]
fn wrong_signature_is_a_miss() {
    let catalog = Catalog::from_blob(&sample_blob()).unwrap();
    assert!(catalog
        .query("android/media/AudioManager", "getStreamVolume", "(I)V")
        .is_none());
}

#[test]
fn type_mismatch_falls_back() {
    let catalog = Catalog::from_blob(&sample_blob()).unwrap();
    // Entry exists but is a bool: int getter applies the fallback.
    assert_eq!(
        catalog.query_int("android/media/AudioManager", "isBluetoothA2dpOn", "()Z", 5),
        5
    );
}

#[test]
fn source_table_produces_valid_catalog() {
    let catalog = Catalog::from_source();
    assert_eq!(catalog.len(), SOURCE_TABLE.len());
}

#[test]
fn source_table_carries_the_three_android_base_setdefaults() {
    // Ticket 15: byte-identical literals from the read-only oracle
    // (android_base.cpp). These must never drift.
    let catalog = Catalog::from_source();
    assert_eq!(
        catalog.query_bool("android/media/AudioManager", "isBluetoothA2dpOn", "()Z", true),
        false,
        "JNI_FALSE"
    );
    assert_eq!(
        catalog.query_int("android/media/AudioManager", "getStreamVolume", "(I)I", -1),
        100
    );
    // jnivm::android::media::AudioDeviceInfo::TYPE_WIRED_HEADPHONES == 4.
    assert_eq!(
        catalog.query_int("android/media/AudioDeviceInfo", "getType", "()I", -1),
        4
    );
}

#[test]
fn source_table_carries_the_fmod_register_reply() {
    // Ticket 76: byte-identical literal from the read-only oracle
    // (sdk_fmod/fakefmod.cpp: `bool FMOD::checkInit() { return true; }`,
    // STATIC descriptor, `()Z`). The sdk_fmod register_jni join resolves
    // this through the shared session; a wrong value or signature must
    // fail this test before it can fail a deployed kit run.
    let catalog = Catalog::from_source();
    assert_eq!(
        catalog.query_bool("org/fmod/FMOD", "checkInit", "()Z", false),
        true,
        "oracle checkInit answers true"
    );
    // The exact JNI lookup the addon join performs: a signature drift is
    // a catalog miss (GetStaticMethodID null), never a type default.
    assert!(catalog
        .query("org/fmod/FMOD", "checkInit", "()Z")
        .is_some());
    assert!(catalog
        .query("org/fmod/FMOD", "checkInit", "(I)Z")
        .is_none(),
        "wrong signature must stay a miss");
}

#[test]
fn from_path_and_load_errors() {
    let dir = std::env::temp_dir().join(format!("rudroid-catalog-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("catalog.bin");
    std::fs::write(&path, sample_blob()).unwrap();
    assert_eq!(Catalog::from_path(&path).unwrap().len(), 3);

    let bad = dir.join("bad.bin");
    std::fs::write(&bad, b"nope").unwrap();
    assert!(matches!(
        Catalog::from_path(&bad),
        Err(CatalogLoadError::Format(_))
    ));
    assert!(matches!(
        Catalog::from_path(dir.join("absent.bin")),
        Err(CatalogLoadError::Io(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
