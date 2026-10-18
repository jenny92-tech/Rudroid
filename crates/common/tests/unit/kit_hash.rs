//! INPUT:  private items of `kit_hash.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `kit_hash.rs`; declared there with `#[path]`
use super::*;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rudroid57-kit-hash-{tag}-{}-{}",
        std::process::id(),
        u64::from(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .subsec_nanos())
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp root");
    dir
}

fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (rel, bytes) in files {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }
}

fn params<'a>(
    target: &'a str,
    profile: &'a str,
    features: &'a [String],
    kit_features: &'a str,
    ci_name: Option<&'a str>,
) -> KitParams<'a> {
    KitParams {
        target,
        profile,
        opt_level: "0",
        debug: "true",
        rustflags: "",
        cargo_encoded_rustflags: "",
        release_lto: "",
        features,
        kit_features,
        ci_name,
    }
}

fn sample_tree(root: &Path) {
    write_tree(
        root,
        &[
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"version = 4\n".as_slice()),
            ("common/src/lib.rs", b"pub fn kit() {}\n".as_slice()),
            (
                "capability/log/src/lib.rs",
                b"rudroid_common::declare!(\"log\");\n".as_slice(),
            ),
            ("capability/log/Cargo.toml", b"[package]\n".as_slice()),
        ],
    );
}

#[test]
fn identical_inputs_hash_identically_in_different_directories() {
    // No-timestamp / no-path property: the same content in two different
    // roots must produce the same id, otherwise rebuilt kit members could
    // never agree.
    let a = temp_root("same-a");
    let b = temp_root("same-b");
    sample_tree(&a);
    sample_tree(&b);
    let features: Vec<String> = Vec::new();
    let walk_a = walk_workspace(&a).expect("walk a");
    let walk_b = walk_workspace(&b).expect("walk b");
    let id_a = kit_build_id(&params("t", "debug", &features, "", None), &walk_a.files);
    let id_b = kit_build_id(&params("t", "debug", &features, "", None), &walk_b.files);
    assert_eq!(id_a, id_b, "same inputs must give the same id");
    assert!(id_a.starts_with("rust-kit-"));
    assert_eq!(id_a.len(), "rust-kit-".len() + 16);
}

#[test]
fn every_hashed_input_class_changes_the_id() {
    let root = temp_root("sensitivity");
    sample_tree(&root);
    let features = vec!["CARGO_FEATURE_SDL=1".to_owned()];
    let base_walk = walk_workspace(&root).expect("walk");
    let base = kit_build_id(
        &params("aarch64-unknown-linux-gnu", "debug", &[], "", None),
        &base_walk.files,
    );

    let changed = |walk: &WorkspaceWalk, p: &KitParams| {
        let id = kit_build_id(p, &walk.files);
        assert_ne!(id, base, "id must change for the mutated input");
    };

    // Each build-parameter input.
    let mutates: [fn(&mut KitParams); 9] = [
        |p: &mut KitParams| p.target = "x86_64-unknown-linux-gnu",
        |p: &mut KitParams| p.profile = "release",
        |p: &mut KitParams| p.opt_level = "3",
        |p: &mut KitParams| p.debug = "false",
        |p: &mut KitParams| p.rustflags = "-C target-cpu=native",
        |p: &mut KitParams| p.cargo_encoded_rustflags = "-C\0link-arg=-foo",
        |p: &mut KitParams| p.release_lto = "thin",
        |p: &mut KitParams| p.kit_features = "--features sdl",
        |p: &mut KitParams| p.ci_name = Some("release-42"),
    ];
    for mutate in mutates {
        let mut p = params("aarch64-unknown-linux-gnu", "debug", &[], "", None);
        mutate(&mut p);
        changed(&base_walk, &p);
    }

    // Feature surface.
    changed(
        &base_walk,
        &params("aarch64-unknown-linux-gnu", "debug", &features, "", None),
    );

    // Source content, added file, renamed file and manifest/lockfile
    // content all feed the identity.
    let mut cases: Vec<(String, Vec<(&str, &[u8])>)> = Vec::new();
    cases.push((
        "source byte change".to_owned(),
        vec![
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"version = 4\n".as_slice()),
            ("common/src/lib.rs", b"pub fn kit() { }\n".as_slice()),
            (
                "capability/log/src/lib.rs",
                b"rudroid_common::declare!(\"log\");\n".as_slice(),
            ),
            ("capability/log/Cargo.toml", b"[package]\n".as_slice()),
        ],
    ));
    cases.push((
        "capability file added".to_owned(),
        vec![
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"version = 4\n".as_slice()),
            ("common/src/lib.rs", b"pub fn kit() {}\n".as_slice()),
            (
                "capability/log/src/lib.rs",
                b"rudroid_common::declare!(\"log\");\n".as_slice(),
            ),
            ("capability/log/Cargo.toml", b"[package]\n".as_slice()),
            ("capability/log/src/extra.rs", b"pub const X: u8 = 1;\n".as_slice()),
        ],
    ));
    cases.push((
        "manifest change".to_owned(),
        vec![
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"version = 4\n".as_slice()),
            ("common/src/lib.rs", b"pub fn kit() {}\n".as_slice()),
            (
                "capability/log/src/lib.rs",
                b"rudroid_common::declare!(\"log\");\n".as_slice(),
            ),
            (
                "capability/log/Cargo.toml",
                b"[package]\ndescription = \"x\"\n".as_slice(),
            ),
        ],
    ));
    cases.push((
        "lockfile change".to_owned(),
        vec![
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"version = 5\n".as_slice()),
            ("common/src/lib.rs", b"pub fn kit() {}\n".as_slice()),
            (
                "capability/log/src/lib.rs",
                b"rudroid_common::declare!(\"log\");\n".as_slice(),
            ),
            ("capability/log/Cargo.toml", b"[package]\n".as_slice()),
        ],
    ));
    for (what, files) in cases {
        let case_root = temp_root("case");
        write_tree(&case_root, &files);
        let walk = walk_workspace(&case_root).expect("walk case");
        let id = kit_build_id(
            &params("aarch64-unknown-linux-gnu", "debug", &[], "", None),
            &walk.files,
        );
        assert_ne!(id, base, "{what} must change the kit id");
    }
}

#[test]
fn embedded_rgba_bytes_change_identity_and_are_watched() {
    let root = temp_root("rgba");
    sample_tree(&root);
    let path = "capability/sdl/assets/mascot.rgba";
    write_tree(&root, &[(path, &[1, 2, 3, 255])]);
    let before = walk_workspace(&root).unwrap();
    assert!(before.files.iter().any(|f| f.rel_path == path));
    assert!(before.dirs.contains(&root.join("capability/sdl/assets")));
    write_tree(&root, &[(path, &[1, 2, 4, 255])]);
    let after = walk_workspace(&root).unwrap();
    let p = params("t", "release", &[], "", None);
    assert_ne!(kit_build_id(&p, &before.files), kit_build_id(&p, &after.files));
}

#[test]
fn walk_collects_sources_and_skips_target_and_nested_lockfiles() {
    let root = temp_root("walk");
    write_tree(
        &root,
        &[
            ("Cargo.toml", b"[workspace]\n".as_slice()),
            ("Cargo.lock", b"workspace lock\n".as_slice()),
            ("common/src/lib.rs", b"pub fn f() {}\n".as_slice()),
            (
                "capability/bionic/src/bridge.c",
                b"int bridge(void);".as_slice(),
            ),
            ("capability/bionic/src/bridge.h", b"#pragma once".as_slice()),
            ("capability/bionic/src/bridge.S", b"ret".as_slice()),
            ("target/debug/lib.rs", b"build output".as_slice()),
            (
                "project/testdata/fixture/Cargo.lock",
                b"fixture lock".as_slice(),
            ),
            ("project/src/lib.rs", b"pub fn g() {}\n".as_slice()),
            ("notes.md", b"docs are not identity".as_slice()),
        ],
    );
    let walk = walk_workspace(&root).expect("walk");
    let rels: Vec<&str> = walk
        .files
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        rels,
        [
            "Cargo.lock",
            "Cargo.toml",
            "capability/bionic/src/bridge.S",
            "capability/bionic/src/bridge.c",
            "capability/bionic/src/bridge.h",
            "common/src/lib.rs",
            "project/src/lib.rs",
        ],
        "target/ content, nested fixture lockfiles and non-source files are excluded, root lockfile included, sorted"
    );
    let parameters = params("t", "debug", &[], "", None);
    let original = kit_build_id(&parameters, &walk.files);
    for file in ["bridge.c", "bridge.h", "bridge.S"] {
        let path = root.join("capability/bionic/src").join(file);
        let original_bytes = fs::read(&path).unwrap();
        fs::write(&path, b"changed native source").unwrap();
        let changed = walk_workspace(&root).unwrap();
        assert_ne!(original, kit_build_id(&parameters, &changed.files));
        fs::write(path, original_bytes).unwrap();
    }
    assert!(
        walk.dirs.iter().any(|d| d.ends_with("project/testdata/fixture")),
        "walked directories are watched so later added sources trigger reruns"
    );
    assert!(
        !walk.dirs.iter().any(|d| d.ends_with("target")),
        "build output subtrees are neither hashed nor watched"
    );
}

#[test]
fn ci_name_validation_and_id_shape() {
    assert!(validate_ci_name("release-2026.09_17-RC1").is_ok());
    assert!(validate_ci_name("").is_err());
    assert!(validate_ci_name("has space").is_err());
    assert!(validate_ci_name("has/slash").is_err());
    assert!(validate_ci_name(&"x".repeat(65)).is_err());

    let root = temp_root("ci-shape");
    sample_tree(&root);
    let walk = walk_workspace(&root).expect("walk");
    let plain = kit_build_id(
        &params("t", "debug", &[], "", None),
        &walk.files,
    );
    let named = kit_build_id(
        &params("t", "debug", &[], "", Some("ci-kit")),
        &walk.files,
    );
    assert_eq!(named, format!("{plain}-ci-ci-kit"));
    // Same content + same CI name => same id (the uniqueness direction).
    let named_again = kit_build_id(
        &params("t", "debug", &[], "", Some("ci-kit")),
        &walk.files,
    );
    assert_eq!(named, named_again);
}
