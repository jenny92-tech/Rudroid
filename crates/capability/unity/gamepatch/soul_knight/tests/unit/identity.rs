use super::*;

#[test]
fn build_hash_mismatch_is_rejected() {
    let root = std::env::temp_dir().join(format!("rudroid-sk-identity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("libanogs.so");
    std::fs::write(&file, b"not the original image").unwrap();
    let error = verify_hash(&file, ANOGS_SHA256, "original libanogs.so")
        .expect_err("different bytes must fail the build gate");
    assert!(error.contains("does not match the original 8.7.0 build"));
    let actual = sha256::hex(&sha256::digest(b"not the original image"));
    verify_hash(&file, &actual, "test fixture").expect("the hash primitive is deterministic");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_library_dir_matches_the_verified_sdk_parent() {
    let root = std::env::temp_dir().join(format!(
        "rudroid-sk-native-dir-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let sdk = root.join("lib/arm64-v8a");
    let wrong = root.join("wrong-native-dir");
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::create_dir_all(&wrong).unwrap();
    let libraries = [
        sdk.join("libanogs.so"),
        sdk.join("libanort.so"),
        sdk.join("libu8t.so"),
    ];
    let references = libraries.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    verify_native_library_dir(&sdk, &references).expect("the verified SDK parent is accepted");
    let error = verify_native_library_dir(&wrong, &references)
        .expect_err("an unrelated existing directory must be rejected");
    assert!(error.contains("does not match the canonical parent"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}
