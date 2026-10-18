//! INPUT:  every workspace `src/**/*.rs` file under `crates/`
//! OUTPUT:  failures naming any JNI table read or Host API fn-pointer call outside its wrapper
//! POS:    keeps the converged unsafe boundary converged: JNI slots only in jni_env.rs, Host API calls only via HostApiV0 methods, per-crate unsafe budgets

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "target" && name != "tests" && name != ".git" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && path.components().any(|c| c.as_os_str() == "src") {
            out.push(path);
        }
    }
}

fn workspace_sources() -> Vec<(String, String)> {
    let root = std::env::var_os("RUDROID_TEST_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf());
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            (rel, std::fs::read_to_string(&path).unwrap())
        })
        .collect()
}

/// Mentions of the `function_slot` identifier: every JNI table read goes
/// through it (directly or via a `function_slot as slot` alias).
fn slot_reads(text: &str) -> usize {
    text.match_indices("function_slot")
        .filter(|(at, _)| {
            let before = text[..*at].chars().last();
            let after = text[at + "function_slot".len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .count()
}

#[test]
fn jni_tables_are_read_only_through_jni_env() {
    // The VM itself builds the tables; jni_env.rs is the one reader.
    let allowed = ["common/src/jni_env.rs", "common/src/jni_abi.rs"];
    let offenders: Vec<String> = workspace_sources()
        .into_iter()
        .filter(|(rel, _)| !allowed.contains(&rel.as_str()) && !rel.starts_with("capability/android/jni_vm/"))
        .filter_map(|(rel, text)| {
            let reads = slot_reads(&text);
            (reads > 0).then(|| format!("{rel}: {reads}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "use rudroid_common::jni_env::{{JniEnv, JavaVm}} instead of JNI table reads: {offenders:?}"
    );
}

#[test]
fn host_api_entries_are_called_through_its_methods() {
    // Entries with a safe HostApiV0 method.
    let wrapped = [
        "config_get_bool", "config_get_i64", "config_get_f64", "config_get_string",
        "config_array_size", "log", "so_symbol", "so_symbol_all", "shared_vm", "fire_module_event",
        "register_module_loaded", "register_android_hooks", "register_unity_profile",
        "register_audio_policy", "register_catalog_provider", "register_shared_vm",
        "catalog_query_int", "catalog_query_bool", "asset_query", "enable_asset_packs",
    ];
    // host_api.rs implements the methods; the loader implements the table.
    let offenders: Vec<String> = workspace_sources()
        .into_iter()
        .filter(|(rel, _)| rel != "common/src/host_api.rs" && !rel.starts_with("project/unity_loader/"))
        .flat_map(|(rel, text)| {
            wrapped
                .iter()
                .filter(|entry| {
                    text.contains(&format!("api.{entry})(")) || text.contains(&format!("host.{entry})("))
                })
                .map(|entry| format!("{rel}: {entry}"))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "call the HostApiV0 method (config_bool, log_line, guest_symbol, java_vm, ...) instead: {offenders:?}"
    );
}

/// `unsafe` keywords in code; `//` comments (outside string literals) are skipped.
fn unsafe_keywords(text: &str) -> usize {
    text.lines()
        .map(|line| {
            let mut quoted = false;
            let mut end = line.len();
            let bytes = line.as_bytes();
            for i in 0..bytes.len() {
                match bytes[i] {
                    b'"' if i == 0 || bytes[i - 1] != b'\\' => quoted = !quoted,
                    b'/' if !quoted && bytes.get(i + 1) == Some(&b'/') => {
                        end = i;
                        break;
                    }
                    _ => {}
                }
            }
            let code = &line[..end];
            code.match_indices("unsafe")
                .filter(|(at, _)| {
                    let before = code[..*at].chars().last();
                    let after = code[at + "unsafe".len()..].chars().next();
                    !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                        && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
                .count()
        })
        .sum()
}

#[test]
fn unsafe_stays_within_each_crates_budget() {
    // `unsafe` keywords per crate in `src/`. Business code reaches the C ABI
    // through the safe tools in `common` (jni_env, host_api, handles,
    // il2cpp); what remains is boundary code. The match is exact: lower the
    // number when unsafe goes away; raising one means a new boundary and
    // needs a reason in review (or a new safe tool instead).
    const BUDGET: &[(&str, usize)] = &[
        ("capability/android/addon/after_unity_module", 0),
        ("capability/android/addon/fmod", 9),
        ("capability/android/addon/gms", 0),
        ("capability/android/addon/levelplay", 0),
        ("capability/android/addon/play_core", 0),
        ("capability/android/addon/unity_burst", 0),
        ("capability/android/addon/unity_helpers", 0),
        ("capability/android/addon/xlua", 0),
        // One added C-string/native-heap adapter; URI policy stays safe in paths.rs.
        ("capability/android/assets", 14),
        // Barrier/spin reuse native payload ownership; new boundaries read exact 32-bit attrs.
        ("capability/android/bionic", 139),
        // Native mount flags reuse the safe common filesystem snapshot.
        ("capability/android/elf_load", 101),
        ("capability/android/framework", 0),
        // Primitive arrays converge on typed POD copies and one rooted UnsafeCell lease ledger.
        // One typed scalar dispatcher macro adds exact byte/char/short/double boundaries.
        ("capability/android/jni_vm", 95),
        ("capability/android/media_fallback", 2),
        ("capability/android/native_events", 24),
        ("capability/android/reply_catalog", 9),
        ("capability/android/reply_catalog/format", 0),
        ("capability/android/zlib", 2),
        ("capability/linux/android_asset_packs", 0),
        ("capability/linux/log_stderr", 2),
        // Typed GL allocation/presentation, borrowed GL wrappers and quiescent cleanup.
        // Typed GLES identity/capability queries, audio enumeration and shader lifecycle/draw
        // boundaries; texture prototypes, checked CPU/PBO read leases and synchronous upload
        // facades. Shader/texture transformations, plans and metadata forbid unsafe.
        // Native-buffer setup has one additional pointer-free SDL request-reset
        // boundary in its creator-thread Drop owner; no guest memory or GL lifetime changes.
        // A named native display-mode struct replaces SDL2's dangling temporary wrapper.
        // Optional typed Vulkan/Wayland dispatch and a separately pinned SDL
        // pixel-size lookup; no new required SDL symbol for GLES deployments.
        // EGL surface attributes use the shared checked facade for unconditional
        // Unity imports, including explicit failure without a GLES context.
        ("capability/linux/sdl_runtime", 340),
        // One typed instance-float getter and one shared borrowed camera-owner fixup.
        ("capability/unity/gamepatch/hollow_knight", 49),
        ("capability/unity/gamepatch/terraria", 0),
        // Checked launcher/scene hooks; managed object operations live in common.
        // Typed language-switch detour and callback-local rooted managed calls.
        ("capability/unity/gamepatch/silksong", 81),
        ("capability/unity/gamepatch/vampire_survivors", 14),
        ("capability/unity/il2cpp_hook", 16),
        ("capability/unity/memory_policy", 11),
        ("capability/unity/player_lifecycle", 3),
        // Exact-image cleanup-order installation is one documented native boundary.
        ("capability/unity/profile_2021_3", 8),
        ("capability/unity/profile_6", 10),
        // Two centralized managed String/instance-reference-field boundaries.
        // Scoped UTF-8/UTF-16 views share one drop boundary, including unwind cleanup.
        // The native monotonic-clock boundary moved out of framework into common.
        // Shared statvfs/fstatvfs boundary also replaces the ELF-local syscall block.
        // One checked native-code patch entry reuses the existing rollback/cache transaction.
        // Hook/pruner callers now use one binding-owned verified GC adapter;
        // the native image evidence reads use safe, bounded kernel I/O.
        // Two scalar fd boundaries: create/adopt and seal a bounded snapshot.
        // One training-only, pointer-free LLVM profile writer per pinned image.
        ("common", 216),
        ("common/host-config", 0),
        // GBM/wsialloc native prototypes and synchronous fd/BO ownership transfer.
        ("common/vulkan-wsi-allocator", 21),
        ("native-build", 0),
        // Additional dlsym prototype for validated guest render configuration.
        // One scalar-only POSIX process termination boundary, after successful CLI cleanup.
        // The SDL adapter reuses its owned API table for failure cleanup,
        // removes the duplicate destroy-function type, and copies native
        // report strings together within one documented session borrow.
        // Both presentation paths share one exact native creation-function type.
        // A checked training export lookup and its no-argument native prototype.
        ("project/unity_loader", 123),
        ("test-support", 0),
    ];
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for (rel, text) in workspace_sources() {
        let krate = rel.split("/src/").next().unwrap().to_owned();
        *counts.entry(krate).or_default() += unsafe_keywords(&text);
    }
    let mut drift = Vec::new();
    for (krate, count) in &counts {
        let budget = BUDGET.iter().find(|(name, _)| name == krate).map(|(_, n)| *n);
        if budget != Some(*count) {
            drift.push(format!("{krate}: {count} (budget {budget:?})"));
        }
    }
    for (krate, _) in BUDGET {
        if !counts.contains_key(*krate) {
            drift.push(format!("{krate}: no sources (remove its budget)"));
        }
    }
    assert!(drift.is_empty(), "unsafe budget drift: {drift:?}");
}

#[test]
fn the_scanner_sees_every_table_read() {
    assert_eq!(slot_reads("rudroid_common::jni_abi::function_slot::<F>(env, 6)"), 1);
    assert_eq!(slot_reads("pub use rudroid_common::jni_abi::function_slot as slot;"), 1, "alias import");
    assert_eq!(slot_reads("slot(guest).load(); fn slot(&self) {} my_function_slot()"), 0, "unrelated names");
    assert_eq!(unsafe_keywords("unsafe { f() } // unsafe in a comment"), 1);
    assert_eq!(unsafe_keywords("let s = \"// not a comment\"; unsafe fn g() {}"), 1);
    assert_eq!(unsafe_keywords("//! unsafe docs\nfn unsafe_name() {}\n#![forbid(unsafe_code)]"), 0);
}
