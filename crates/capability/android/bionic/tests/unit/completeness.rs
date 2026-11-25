//! INPUT:  pinned Android 15 export maps and the production Bionic query/registries
//! OUTPUT:  independent library/version conformance and an explicitly strict completeness gate
//! POS:    distinguishes resolvable ABI coverage from behavioral and real-game acceptance
use super::*;

struct Export {
    library: &'static CStr,
    name: &'static str,
    version: &'static str,
}

// The snapshots are Android 15/API 35. Follow Soong's NDK-mode filters,
// including architecture-specific introduction levels and private versions.
fn available(tags: &str) -> bool {
    let tags: Vec<_> = tags.split_whitespace().collect();
    let architectures = ["arm", "arm64", "x86", "x86_64", "riscv64"];
    if tags.iter().any(|t| architectures.contains(t)) && !tags.contains(&"arm64") {
        return false;
    }
    if tags.iter().any(|t| matches!(*t, "apex" | "llndk" | "systemapi" | "platform-only" | "future")) {
        return false;
    }
    let introduced = tags.iter().find_map(|t| t.strip_prefix("introduced-arm64="))
        .or_else(|| tags.iter().find_map(|t| t.strip_prefix("introduced=")));
    let level = introduced.map_or(0, |value| match value {
        "O" => 26, "P" => 28, "Q" => 29, "R" => 30, "S" => 31,
        "Tiramisu" => 33, "UpsideDownCake" => 34, "VanillaIceCream" => 35,
        _ => value.parse::<u32>().expect("unknown Android introduction level"),
    });
    level <= 35
}

fn parse_map(library: &'static CStr, source: &'static str) -> Vec<Export> {
    let mut exports = Vec::new();
    let (mut version, mut enabled, mut global) = ("", false, false);
    for line in source.lines() {
        let (code, tags) = line.split_once('#').unwrap_or((line, ""));
        let code = code.trim();
        if code.is_empty() { continue; }
        if let Some(name) = code.strip_suffix('{') {
            version = name.trim();
            enabled = !version.ends_with("_PRIVATE") && !version.ends_with("_PLATFORM") && available(tags);
            global = false;
        } else if code.starts_with('}') {
            version = "";
            global = false;
        } else if code == "global:" {
            global = true;
        } else if code == "local:" {
            global = false;
        } else if enabled && global && available(tags) {
            let name = code.strip_suffix(';').expect("unrecognized export-map syntax");
            assert!(!version.is_empty() && !name.contains('*'));
            exports.push(Export { library, name, version });
        }
    }
    exports
}

fn reference() -> Vec<Export> {
    [
        (c"libc.so", include_str!("../fixtures/android-15/libc.map.txt")),
        (c"libm.so", include_str!("../fixtures/android-15/libm.map.txt")),
        (c"liblog.so", include_str!("../fixtures/android-15/liblog.map.txt")),
    ].into_iter().flat_map(|(library, source)| parse_map(library, source)).collect()
}

fn resolve(export: &Export) -> usize {
    let version = std::ffi::CString::new(export.version).unwrap();
    query(export.name.as_bytes(), Some(export.library), Some(&version))
}

#[test]
fn public_reference_filters_architecture_api_levels_and_private_exports() {
    for tags in ["", "var", "arm64 x86_64", "introduced=35", "introduced=P", "introduced=36 introduced-arm64=21"] {
        assert!(available(tags), "{tags}");
    }
    for tags in ["arm x86", "riscv64", "introduced=36", "introduced=21 introduced-arm64=36", "apex", "llndk", "systemapi", "platform-only", "future"] {
        assert!(!available(tags), "{tags}");
    }
    let exports = reference();
    let unique: BTreeSet<_> = exports.iter().map(|e| (e.library, e.name)).collect();
    assert_eq!(unique.len(), exports.len(), "duplicate reference exports");
    for (library, count) in [(c"libc.so", 1434), (c"libm.so", 286), (c"liblog.so", 18)] {
        assert_eq!(exports.iter().filter(|e| e.library == library).count(), count, "reference/filter drift for {library:?}");
    }
    for (library, name, version) in [
        (c"libc.so", "malloc", "LIBC"),
        (c"libc.so", "pthread_barrier_wait", "LIBC_N"),
        (c"libc.so", "aligned_alloc", "LIBC_P"),
        (c"libc.so", "epoll_pwait2", "LIBC_V"),
        (c"libm.so", "sin", "LIBC"),
        (c"liblog.so", "__android_log_get_minimum_priority", "LIBLOG_R"),
    ] {
        assert!(exports.iter().any(|e| (e.library, e.name, e.version) == (library, name, version)), "{name}");
    }
    for name in ["__atomic_cmpxchg", "__riscv_hwprobe", "android_run_on_all_threads", "__system_properties_zygote_reload", "android_logger_list_alloc"] {
        assert!(!exports.iter().any(|e| e.name == name), "non-NDK export {name}");
    }
}

#[test]
fn implemented_exports_match_independent_android_versions() {
    let exports = reference();
    let mut failures = Vec::new();
    for (library, names) in [(c"libc.so", libc_names().keys().copied().collect::<Vec<_>>()),
        (c"libm.so", (MATH.names)())] {
        for name in names {
            // Extra legacy compatibility aliases are reported separately by
            // the audit; only a real AOSP record can attest a version node.
            let Some(export) = exports.iter().find(|e| e.library == library && e.name.as_bytes() == name) else { continue; };
            if resolve(export) == 0 {
                failures.push(format!("{}:{} needs {}", library.to_string_lossy(), export.name, export.version));
            }
        }
    }
    assert!(failures.is_empty(), "Android version mismatches:\n{}", failures.join("\n"));
}

#[test]
fn all_implemented_queries_reject_foreign_libraries_and_version_nodes() {
    let names = names_of(&LIBC.iter().chain([&MATH, &LOGGING]).collect::<Vec<_>>());
    let libc = libc_names();
    let math: BTreeSet<_> = (MATH.names)().into_iter().collect();
    let logging: BTreeSet<_> = (LOGGING.names)().into_iter().collect();
    let exports = reference();
    let versions: BTreeSet<_> = exports.iter().map(|e| e.version).collect();
    for name in names.keys() {
        for library in [c"libc.so", c"libm.so", c"liblog.so", c"libc.so.6", c"libdl.so"] {
            let owns_name = match library.to_bytes() {
                b"libc.so" => libc.contains_key(name) || *name == b"ldexp",
                b"libm.so" => math.contains(name),
                b"liblog.so" => logging.contains(name),
                _ => false,
            };
            if !owns_name {
                for version in &versions {
                    let version = std::ffi::CString::new(*version).unwrap();
                    assert_eq!(query(name, Some(library), Some(&version)), 0, "{} via foreign {library:?}/{version:?}", lossy(name));
                }
            }
            for version in [c"LIBC_PRIVATE", c"LIBC_PLATFORM", c"GLIBC_2.17", c"UNKNOWN"] {
                assert_eq!(query(name, Some(library), Some(version)), 0, "{} via {library:?}/{version:?}", lossy(name));
            }
        }
        assert_eq!(query(name, Some(c"libc.so"), None), 0);
        assert_eq!(query(name, None, Some(c"LIBC")), 0);
    }
}

fn audit() -> usize {
    let exports = reference();
    let mut report = String::from("library\tversion\tsymbol\tstatus\n");
    let mut missing = 0;
    for library in [c"libc.so", c"libm.so", c"liblog.so"] {
        let mut total = 0;
        let mut resolved = 0;
        let mut bare_only = 0;
        for export in exports.iter().filter(|e| e.library == library) {
            total += 1;
            let status = if resolve(export) != 0 {
                resolved += 1;
                "resolved"
            } else {
                missing += 1;
                if query(export.name.as_bytes(), None, None) != 0 { bare_only += 1; "bare-only-or-wrong-version" }
                else { "missing" }
            };
            report.push_str(&format!("{}\t{}\t{}\t{status}\n", library.to_string_lossy(), export.version, export.name));
        }
        println!("BIONIC_API35 {} total={total} resolved={resolved} bare_only_or_wrong_version={bare_only} missing={}",
            library.to_string_lossy(), total - resolved - bare_only);
    }
    for (library, names) in [(c"libc.so", libc_names().keys().copied().collect::<Vec<_>>()),
        (c"libm.so", (MATH.names)()), (c"liblog.so", (LOGGING.names)())] {
        for name in names {
            if !exports.iter().any(|e| e.library == library && e.name.as_bytes() == name) {
                report.push_str(&format!("{}\t-\t{}\textra-compatibility-export\n", library.to_string_lossy(), lossy(name)));
            }
        }
    }
    let directory = std::path::PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("run via docker-rust.sh"))
        .join("bionic-completeness");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("android-15-arm64.tsv"), report).unwrap();
    println!("BIONIC_API35 report={}/android-15-arm64.tsv; resolution is not behavioral conformance", directory.display());
    missing
}

#[test]
fn android_15_coverage_report() { audit(); }

#[test]
#[ignore = "strict full Android API 35 ABI gate; bounded Unity compatibility has known missing APIs"]
fn full_android_15_abi_completeness() {
    let missing = audit();
    assert_eq!(missing, 0, "Bionic is NOT a complete Android API 35 ABI; inspect the TSV report");
}
