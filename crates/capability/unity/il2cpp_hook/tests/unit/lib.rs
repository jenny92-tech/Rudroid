//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn absent_table_gives_empty_registry() {
    let config = HostConfig::parse("").unwrap();
    let registry = HookRegistry::from_config(&config);
    assert!(registry.is_empty());
    // Unknown sections elsewhere must not break anything.
    let config = HostConfig::parse("[gpu]\nglMajorVersionOverride = 3\n").unwrap();
    assert!(HookRegistry::from_config(&config).is_empty());
}

#[test]
fn hook_table_is_read_compatibly() {
    let config =
        HostConfig::parse("[il2cpp_hooks]\nalpha = \"Sym_One\"\nbeta = \"Sym_Two\"\n").unwrap();
    let registry = HookRegistry::from_config(&config);
    assert_eq!(registry.len(), 2);
    assert_eq!(registry.get("alpha").unwrap().symbol, "Sym_One");
    assert_eq!(registry.get("beta").unwrap().symbol, "Sym_Two");
    assert_eq!(registry.get("alpha").unwrap().address, 0);
}

/// Registry resolution semantics, independent of the detour backend.
#[test]
fn resolve_semantics_return_previous_and_refuse_unknown() {
    let mut registry = HookRegistry::default();
    registry.register_request("fake", "fake_symbol");
    assert!(matches!(
        registry.resolve("nope", 1),
        Err(HookError::UnknownHook(_))
    ));
    assert_eq!(registry.resolve("fake", 0x042).unwrap(), 0);
    // Re-resolving returns the previous address (contract since ticket 28).
    assert_eq!(registry.resolve("fake", 0x043).unwrap(), 0x042);
    assert_eq!(registry.get("fake").unwrap().address, 0x043);
}

/// Ticket 51: the detour is REAL — a branch encoding over live code, the
/// call reaching the replacement, the trampoline still running the
/// original, and restore returning the original behavior.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
#[test]
fn apply_detour_installs_a_real_branch_on_a_code_page() {
    // mov w0, #65; ret + two NOPs (room for a 16-byte veneer).
    let (page, _) = code_page(&[0x5280_0820, 0xD65F_03C0, 0xD503_201F, 0xD503_201F]);
    let target = page as usize;
    let destination = detour_destination as extern "C" fn() -> u32 as usize;

    let mut registry = HookRegistry::default();
    registry.register_request("fake", "synthetic_page");
    registry.resolve("fake", target).expect("resolve");

    // SAFETY: our own RX page; nothing executes the entry during install.
    let detour = unsafe { registry.apply_detour("fake", destination) }.expect("detour");
    assert_eq!(detour.address, target);
    assert_eq!(detour.replacement, destination);
    assert_eq!(detour.covered_bytes, 4, "exactly one word is overwritten");
    // The entry always holds a direct B. Near destinations are reached
    // by that B itself; far ones via the near trampoline's veneer.
    // SAFETY: one word inside our own page.
    let insn = unsafe { (target as *const u32).read_volatile() };
    assert_eq!(insn & 0xFC00_0000, 0x1400_0000, "direct B at the entry");
    // SAFETY: decode of our own page's word.
    let branch_target = arm64::decode_b(insn, target);
    match detour.shape {
        HookShape::Branch => {
            assert_eq!(branch_target, destination, "B goes straight there");
        }
        HookShape::AbsoluteJump => {
            assert_ne!(
                branch_target, destination,
                "far destinations must bounce through the near veneer"
            );
        }
    }

    // SAFETY: target returns 65 unhooked, 66 while detoured.
    let call: extern "C" fn() -> u32 = unsafe { std::mem::transmute(target) };
    assert_eq!(call(), 66, "calls reach the replacement");
    // SAFETY: the trampoline replays `mov w0, #65; ret`.
    let original: extern "C" fn() -> u32 =
        unsafe { std::mem::transmute(detour.original) };
    assert_eq!(original(), 65, "the original stays callable via trampoline");

    // SAFETY: same single-threaded contract.
    unsafe { registry.restore_detour(&detour) }.expect("restore");
    assert_eq!(call(), 65, "restore returns the original behavior");
    // SAFETY: our own page.
    let insn = unsafe { (target as *const u32).read_volatile() };
    assert_eq!(insn, 0x5280_0820, "the saved prologue word is back");
}

/// W^X evidence for the install/restore patch window in-process.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
#[test]
fn detour_never_leaves_a_writable_executable_page() {
    let (page, _) = code_page(&[0x5280_0820, 0xD65F_03C0, 0xD503_201F, 0xD503_201F]);
    let target = page as usize;
    let destination = detour_destination as extern "C" fn() -> u32 as usize;

    let mut registry = HookRegistry::default();
    registry.register_request("wx", "synthetic_page");
    registry.resolve("wx", target).expect("resolve");

    // SAFETY: our own RX page.
    let detour = unsafe { registry.apply_detour("wx", destination) }.expect("detour");
    // SAFETY: symmetric restore.
    unsafe { registry.restore_detour(&detour) }.expect("restore");
    for line in std::fs::read_to_string("/proc/self/maps").unwrap().lines() {
        let Some((range, rest)) = line.split_once(' ') else {
            continue;
        };
        let perms = rest.split_whitespace().next().unwrap_or("");
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) =
            (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16))
        else {
            continue;
        };
        if start <= target && target < end {
            assert!(
                !(perms.contains('w') && perms.contains('x')),
                "the patched page must not be writable AND executable: {line}"
            );
        }
    }
}

/// Anonymous RW page written with `words`, then sealed RX (never W+X).
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn code_page(words: &[u32]) -> (*mut u8, usize) {
    let page_len = 4096;
    // SAFETY: anonymous mmap; failure is MAP_FAILED.
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap code page");
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    // SAFETY: page is 4096 bytes, words is far shorter.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), map as *mut u8, bytes.len()) };
    // SAFETY: our own page.
    let rc = unsafe { libc::mprotect(map, page_len, libc::PROT_READ | libc::PROT_EXEC) };
    assert_eq!(rc, 0, "mprotect RX failed");
    (map as *mut u8, page_len)
}

extern "C" fn detour_destination() -> u32 {
    66
}

#[test]
fn detour_refuses_unresolved_hooks() {
    let mut registry = HookRegistry::default();
    registry.register_request("pending", "Sym");
    assert!(matches!(
        unsafe { registry.apply_detour("pending", 0x10) },
        Err(HookError::Unresolved(_))
    ));
}

// ---- ticket 50: legacy `[[il2cpp_patch]]` compatibility ----

/// The legacy dialect exactly as existing device configs write it (the
/// reference plugin's `[[il2cpp_patch]]` table): assembly/namespace/
/// class/method/mode fields, no renaming, no migration.
fn legacy_patch_config() -> HostConfig {
    HostConfig::parse(
        r#"
[[il2cpp_patch]]
assembly = "Assembly-CSharp"
namespace = "HollowKnight"
class = "PlayerState"
method = "TakeDamage"
argc = 1
arg = 0
field = 44
mult = 0.5
mode = "scale"

[[il2cpp_patch]]
class = "CameraController"
method = "onEnable"
mode = "trace"
"#,
    )
    .unwrap()
}

#[test]
fn legacy_patch_table_is_not_silently_dropped() {
    // RED before ticket 50: only `[il2cpp_hooks]` was read, so a plain
    // legacy config produced an EMPTY registry — silent data loss. The
    // reference plugin loads up to eight patches from this same table.
    let registry = HookRegistry::from_config(&legacy_patch_config());
    assert!(
        !registry.is_empty(),
        "legacy [[il2cpp_patch]] entries must produce hook requests"
    );
}

#[test]
fn legacy_patch_table_builds_reference_requests() {
    let registry = HookRegistry::from_config(&legacy_patch_config());
    let patches = registry.patches();
    assert_eq!(patches.len(), 2);
    let p0 = &patches[0];
    assert_eq!(p0.id, "il2cpp_patch.0");
    assert_eq!(p0.assembly, "Assembly-CSharp");
    assert_eq!(p0.namespace, "HollowKnight");
    assert_eq!(p0.class, "PlayerState");
    assert_eq!(p0.method, "TakeDamage");
    assert_eq!(p0.argument_count, 1);
    assert_eq!(p0.object_argument, 0);
    assert_eq!(p0.field, 44);
    assert_eq!(p0.multiplier, 0.5);
    assert_eq!(p0.mode, PatchMode::Scale);
    assert_eq!(p0.source_field, -1);
    assert_eq!(p0.value, 0.0);
    assert!(!p0.integer_field);
    // Entry 1: unset keys carry the reference getter defaults.
    let p1 = &patches[1];
    assert_eq!(p1.id, "il2cpp_patch.1");
    assert_eq!(p1.assembly, "Assembly-CSharp");
    assert_eq!(p1.namespace, "");
    assert_eq!(p1.class, "CameraController");
    assert_eq!(p1.method, "onEnable");
    assert_eq!(p1.argument_count, 0);
    assert_eq!(p1.field, -1);
    assert_eq!(p1.mode, PatchMode::Trace);
    assert!(!p1.integer_field);
}

#[test]
fn patch_filter_matches_reference() {
    // Each entry below is a complete `[[il2cpp_patch]]` body (full
    // fields, no key overrides — TOML forbids duplicate keys). The
    // baseline scale entry is valid: class/method set, a field offset
    // and a non-identity multiplier (the reference drops scale entries
    // without either).
    let entry = |body: &str| {
        let config =
            HostConfig::parse(&format!("[[il2cpp_patch]]\n{body}\n")).unwrap();
        HookRegistry::from_config(&config)
    };
    let base = "class = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\n";
    assert_eq!(entry(base).patches().len(), 1);
    // Disabled entries and empty class/method drop silently.
    assert_eq!(entry(&format!("{base}enabled = false")).patches().len(), 0);
    let no_class = "class = \"\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\n";
    assert_eq!(entry(no_class).patches().len(), 0);
    let no_method = "class = \"C\"\nmethod = \"\"\nfield = 3\nmult = 0.5\n";
    assert_eq!(entry(no_method).patches().len(), 0);
    // Out-of-range argc (valid: 0..=6) and arg (valid: -1..=6) drop.
    assert_eq!(entry(&format!("{base}argc = -1")).patches().len(), 0);
    assert_eq!(entry(&format!("{base}argc = 7")).patches().len(), 0);
    assert_eq!(entry(&format!("{base}argc = 6")).patches().len(), 1);
    assert_eq!(entry(&format!("{base}arg = -2")).patches().len(), 0);
    assert_eq!(entry(&format!("{base}arg = 7")).patches().len(), 0);
    assert_eq!(entry(&format!("{base}arg = -1")).patches().len(), 1);
    assert_eq!(entry(&format!("{base}arg = 6")).patches().len(), 1);
    // scale (getter default) and freeze require a field offset; their
    // no-field entries fall back to field = -1 and drop.
    let no_field = "class = \"C\"\nmethod = \"M\"\nmult = 0.5\n";
    assert_eq!(entry(no_field).patches().len(), 0);
    let no_field_freeze =
        "class = \"C\"\nmethod = \"M\"\nmode = \"freeze\"\n";
    assert_eq!(entry(no_field_freeze).patches().len(), 0);
    // trace/bypass/force_return do not need a field offset.
    let no_field_trace =
        "class = \"C\"\nmethod = \"M\"\nmode = \"trace\"\n";
    assert_eq!(entry(no_field_trace).patches().len(), 1);
    let no_field_bypass =
        "class = \"C\"\nmethod = \"M\"\nmode = \"bypass\"\n";
    assert_eq!(entry(no_field_bypass).patches().len(), 1);
    let no_field_force =
        "class = \"C\"\nmethod = \"M\"\nmode = \"force_return\"\n";
    assert_eq!(entry(no_field_force).patches().len(), 1);
    // A scale patch with the identity multiplier is a no-op drop — both
    // written and via the getter default (1.0) — while freeze keeps it
    // and a real multiplier passes.
    let identity_scale =
        "class = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 1.0\n";
    assert_eq!(entry(identity_scale).patches().len(), 0);
    let plain_scale = "class = \"C\"\nmethod = \"M\"\nfield = 3\n";
    assert_eq!(entry(plain_scale).patches().len(), 0);
    let identity_freeze =
        "class = \"C\"\nmethod = \"M\"\nfield = 3\nmode = \"freeze\"\n";
    assert_eq!(entry(identity_freeze).patches().len(), 1);
    let real_mult =
        "class = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 2.0\n";
    assert_eq!(entry(real_mult).patches().len(), 1);
}

#[test]
fn patch_mode_and_type_defaults_match_reference() {
    let cases = [
        ("", PatchMode::Scale),
        ("mode = \"scale\"", PatchMode::Scale),
        ("mode = \"freeze\"", PatchMode::Freeze),
        ("mode = \"force_return\"", PatchMode::ForceReturn),
        ("mode = \"trace\"", PatchMode::Trace),
        ("mode = \"bypass\"", PatchMode::Bypass),
        // Unknown values fall back to scale, like parse_mode's default.
        ("mode = \"mystery\"", PatchMode::Scale),
        // A wrong-typed mode leaf keeps the getter fallback ("scale").
        ("mode = 7", PatchMode::Scale),
    ];
    for (body, mode) in cases {
        let config = HostConfig::parse(&format!(
            "[[il2cpp_patch]]\nclass = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\n{body}\n"
        ))
        .unwrap();
        let registry = HookRegistry::from_config(&config);
        assert_eq!(registry.patches()[0].mode, mode, "body: {body}");
    }
    // `type = "i32"` flips integer_field; anything else stays float.
    let i32_config = HostConfig::parse(
        "[[il2cpp_patch]]\nclass = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\ntype = \"i32\"\n",
    )
    .unwrap();
    assert!(HookRegistry::from_config(&i32_config).patches()[0].integer_field);
    let f32_config = HostConfig::parse(
        "[[il2cpp_patch]]\nclass = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\ntype = \"f64\"\n",
    )
    .unwrap();
    assert!(!HookRegistry::from_config(&f32_config).patches()[0].integer_field);
}

#[test]
fn legacy_and_new_tables_coexist() {
    // Compat rule (ticket 50): both dialects are read and BOTH take
    // effect — neither table shadows or empties the other.
    let config = HostConfig::parse(
        r#"
[[il2cpp_patch]]
class = "PlayerState"
method = "TakeDamage"
field = 44
mult = 0.5

[il2cpp_hooks]
extra_probe = "Sym_Extra"
"#,
    )
    .unwrap();
    let registry = HookRegistry::from_config(&config);
    assert_eq!(registry.patches().len(), 1);
    assert_eq!(registry.patches()[0].class, "PlayerState");
    assert_eq!(
        registry.get("extra_probe").map(|h| h.symbol.as_str()),
        Some("Sym_Extra")
    );
    assert!(!registry.is_empty());
}

#[test]
fn patch_cap_matches_reference_eight() {
    let mut text = String::new();
    for _ in 0..10 {
        text.push_str(
            "[[il2cpp_patch]]\nclass = \"C\"\nmethod = \"M\"\nfield = 3\nmult = 0.5\n",
        );
    }
    let registry = HookRegistry::from_config(&HostConfig::parse(&text).unwrap());
    assert_eq!(registry.patches().len(), MAX_PATCHES);
    assert_eq!(registry.configured_patches(), 10);
}

#[test]
fn invalid_patch_entries_do_not_break_the_loop() {
    // A filtered-out entry in the middle: valid entries around it still
    // load, and ids keep the CONFIG index (traceability over compaction,
    // like the reference's fixed per-index hooks).
    let config = HostConfig::parse(
        r#"
[[il2cpp_patch]]
class = "C0"
method = "M0"
field = 3
mult = 0.5

[[il2cpp_patch]]
enabled = false
class = "C1"
method = "M1"
field = 3
mult = 0.5

[[il2cpp_patch]]
class = "C2"
method = "M2"
field = 3
mult = 0.5
"#,
    )
    .unwrap();
    let registry = HookRegistry::from_config(&config);
    let patches = registry.patches();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].id, "il2cpp_patch.0");
    assert_eq!(patches[0].class, "C0");
    assert_eq!(patches[1].id, "il2cpp_patch.2");
    assert_eq!(patches[1].class, "C2");
    assert_eq!(registry.configured_patches(), 3);
}

// ------------------------------------------------------------------
// Ticket 82: Il2cppReady detour lifecycle — one-shot broadcast, no
// producer lock held across guest code / broadcast / logging, explicit
// NULL-domain and restore-failure semantics.
// ------------------------------------------------------------------

mod lifecycle {
    use super::*;
    use rudroid_common::registry::HostStr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    static SERIAL: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    static FIRE_COUNT: AtomicUsize = AtomicUsize::new(0);
    static ORIG_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// false = the fake original returns 0 (init failed).
    static ORIG_SUCCEEDS: AtomicBool = AtomicBool::new(true);
    static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Watchdog completion flag: the reentrancy test must finish long
    /// before this fires; otherwise the process aborts (red = hang).
    static DONE: AtomicBool = AtomicBool::new(false);

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        let mutex = SERIAL.get_or_init(|| Mutex::new(()));
        mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn logs() -> Vec<String> {
        lock(&LOG_LINES).clone()
    }

    unsafe extern "C" fn fake_original(_domain_name: *const c_char) -> c_int {
        ORIG_CALLS.fetch_add(1, Ordering::AcqRel);
        if ORIG_SUCCEEDS.load(Ordering::Acquire) {
            0x1234
        } else {
            0
        }
    }

    extern "C" fn fake_fire(kind: u32) -> c_int {
        assert_eq!(kind, ModuleKind::Il2cppReady as u32);
        FIRE_COUNT.fetch_add(1, Ordering::AcqRel);
        // Re-entrancy probe (ticket 82 red): pre-82 this ran while the
        // producer mutex was held — logging here deadlocked the guest
        // thread. The lock-free hot path must let it through.
        producer_log("broadcast dispatched (reentrant logger probe)");
        0
    }

    extern "C" fn fake_log(tag: HostStr<'_>, message: HostStr<'_>) {
        let line = format!(
            "{}: {}",
            tag.to_cstr().unwrap_or_default().to_string_lossy(),
            message.to_cstr().unwrap_or_default().to_string_lossy()
        );
        lock(&LOG_LINES).push(line);
    }

    extern "C" fn stub_symbol0(_module: *mut c_void, _name: HostStr<'_>) -> usize {
        0
    }

    /// Install the ONE process-wide fake producer surface. HostApiV0 has
    /// only function-pointer fields, so this value is `Sync` and the
    /// OnceLock never needs resetting; per-test state lives in atomics.
    fn ensure_producer_armed() {
        let api = HostApiV0 {
            so_symbol: stub_symbol0,
            log: fake_log,
            fire_module_event: fake_fire,
            so_symbol_all: stub_symbol0,
            ..HostApiV0::stubbed()
        };
        let _ = PRODUCER_API.set(api);
    }

    fn reset_lifecycle() {
        FIRE_COUNT.store(0, Ordering::Release);
        ORIG_CALLS.store(0, Ordering::Release);
        ORIG_SUCCEEDS.store(true, Ordering::Release);
        READY_FIRED.store(false, Ordering::Release);
        // SAFETY: 0 marks the detour not installed.
        unsafe { INIT_ORIG.set(0) };
        lock(&LOG_LINES).clear();
        DONE.store(false, Ordering::Release);
    }

    #[test]
    fn broadcast_fires_once_and_only_on_success() {
        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();
        // SAFETY: `fake_original` has the il2cpp_init signature.
        unsafe { INIT_ORIG.set(fake_original as *const () as usize) };

        let first = unsafe { il2cpp_init_detour(std::ptr::null()) };
        assert_eq!(first, 0x1234, "original result forwarded");
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1, "first success broadcasts");
        assert_eq!(ORIG_CALLS.load(Ordering::Acquire), 1);

        let second = unsafe { il2cpp_init_detour(std::ptr::null()) };
        assert_eq!(second, 0x1234);
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1, "one-shot: no second broadcast");
        assert_eq!(ORIG_CALLS.load(Ordering::Acquire), 2, "original still runs");
    }

    #[test]
    fn zero_status_stays_silent_and_keeps_the_detour_installed() {
        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();
        ORIG_SUCCEEDS.store(false, Ordering::Release);
        // SAFETY: `fake_original` has the il2cpp_init signature.
        unsafe { INIT_ORIG.set(fake_original as *const () as usize) };

        let domain = unsafe { il2cpp_init_detour(std::ptr::null()) };
        assert_eq!(domain, 0, "zero status forwards as-is");
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 0, "zero status never broadcasts");
        assert!(!READY_FIRED.load(Ordering::Acquire));

        // Retry succeeds: broadcast happens exactly once; the entry
        // stays redirected by design (no runtime restore, ticket 82).
        ORIG_SUCCEEDS.store(true, Ordering::Release);
        let retry = unsafe { il2cpp_init_detour(std::ptr::null()) };
        assert_eq!(retry, 0x1234);
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1);
        assert!(
            logs().iter().any(|l| l.contains("entry stays redirected")),
            "the no-restore lifecycle is the logged design point: {:#?}",
            logs()
        );
    }

    #[test]
    fn subscriber_reentry_during_broadcast_completes_without_deadlock() {
        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();
        // SAFETY: `fake_original` has the il2cpp_init signature.
        unsafe { INIT_ORIG.set(fake_original as *const () as usize) };

        // Watchdog: a held producer lock inside the broadcast (pre-82
        // behavior) deadlocks the reentrant logger probe; abort loudly
        // instead of hanging the suite.
        let done = &DONE;
        std::thread::spawn(move || {
            for _ in 0..50 {
                if done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !done.load(Ordering::Acquire) {
                eprintln!("T82 WATCHDOG: broadcast reentry deadlocked — aborting");
                std::process::abort();
            }
        });

        let domain = unsafe { il2cpp_init_detour(std::ptr::null()) };
        assert_eq!(domain, 0x1234);
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1);
        assert!(
            logs().iter().any(|l| l.contains("broadcast dispatched")),
            "the reentrant subscriber logger must run inside the broadcast: {:#?}",
            logs()
        );
        DONE.store(true, Ordering::Release);
    }

    /// Ticket 82 rework: there is no runtime restore any more, so the
    /// old `restore_failure_is_diagnosed_not_silent` probe is retired
    /// with it. The failure surface it covered (a restore that cannot
    /// complete) no longer exists on the lifecycle: the post-broadcast
    /// path performs no code-page write at all, and the entry staying
    /// redirected is the DESIGNED state — asserted in
    /// `zero_status_stays_silent_and_keeps_the_detour_installed` and,
    /// on real code pages, in the same-page concurrency regression
    /// below. `HookRegistry::restore_detour` keeps its diagnosed
    /// failure path for pre-guest machinery use.

    /// The mapped callback with a missing `il2cpp_init` export stays a
    /// structured diagnostic and never fires (portable: the stub symbol
    /// resolver always misses).
    #[test]
    fn mapped_missing_export_diagnosed_and_never_fires() {
        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();
        let event = rudroid_common::registry::RudroidModuleEvent {
            kind: ModuleKind::Il2cpp as u32,
            base: 0x1234,
            name: rudroid_common::registry::HostStr::null(),
        };
        // The stub resolver misses, which is the diagnosed path.
        on_il2cpp_mapped(Some(&event), std::ptr::null_mut());
        assert!(
            logs().iter().any(|l| l.contains("il2cpp_init export missing")),
            "{:#?}",
            logs()
        );
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 0);
    }

    /// Real end-to-end on ARM64 Linux: synthetic guest page detoured,
    /// broadcast through the live trampoline, and the entry STAYS
    /// REDIRECTED (ticket 82 rework — the runtime restore is gone).
    /// The second call runs the original through the still-mapped
    /// trampoline with no second broadcast and no page rewrite.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    #[test]
    fn real_detour_broadcasts_once_and_entry_stays_redirected() {
        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();

        // mov w0, #65; ret — the "original il2cpp_init" returns a
        // nonzero success status (65) so the broadcast path runs.
        let (page, _) = code_page(&[0x5280_0820, 0xD65F_03C0]);
        let entry_address = page as usize;
        let mut registry = HookRegistry::default();
        registry.register_request("il2cpp_init", "guest_page");
        registry.resolve("il2cpp_init", entry_address).expect("resolve");
        // SAFETY: our own RX page; nothing executes the entry during install.
        let detour = unsafe { registry.apply_detour("il2cpp_init", il2cpp_init_detour as usize) }
            .expect("detour install");
        // SAFETY: the trampoline of the entry just detoured.
        unsafe { INIT_ORIG.set(detour.original) };

        // SAFETY: patched entry on our own page, called once at a time.
        let entry: unsafe extern "C" fn(*const c_char) -> c_int =
            unsafe { std::mem::transmute(entry_address) };
        let domain = unsafe { entry(std::ptr::null()) };
        assert_eq!(domain as usize, 65, "guest result forwarded through trampoline");
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1);

        // The entry KEEPS the redirect branch (ticket 82 rework: no
        // runtime restore): a second call still answers through the
        // trampoline — the original keeps working — with no second
        // broadcast and no code-page write.
        // SAFETY: our own page.
        let insn = unsafe { (entry_address as *const u32).read_volatile() };
        assert_ne!(insn, 0x5280_0820, "entry stays redirected (no runtime restore)");
        let again = unsafe { entry(std::ptr::null()) };
        assert_eq!(again as usize, 65, "redirected entry still runs the original");
        assert_eq!(FIRE_COUNT.load(Ordering::Acquire), 1, "one-shot holds");
        assert!(
            page_has_exec(entry_address),
            "the guest page stays executable after the whole lifecycle"
        );
    }

    /// True when /proc/self/maps currently marks the page containing
    /// `address` executable; the ticket 82 regression samples this from
    /// a concurrent thread for the whole lifecycle.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    fn page_has_exec(address: usize) -> bool {
        let maps = std::fs::read_to_string("/proc/self/maps").expect("read maps");
        for line in maps.lines() {
            let Some((range, rest)) = line.split_once(' ') else {
                continue;
            };
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            let (Ok(start), Ok(end)) = (
                usize::from_str_radix(start, 16),
                usize::from_str_radix(end, 16),
            ) else {
                continue;
            };
            if address >= start && address < end {
                let perms = rest.split_whitespace().next().unwrap_or("");
                return perms.contains('x');
            }
        }
        false
    }

    /// Ticket 82 rework regression (review Spec §2): another thread
    /// executes a function on the SAME PAGE as the hooked entry through
    /// the whole Il2cppReady lifecycle. The previous design's one-shot
    /// restore opened a page-wide RW (execute-removed) mprotect window
    /// on live guest code — the sibling's instruction fetch faulted
    /// (red evidence: red82-restore-nx-fault.log, SIGSEGV on the
    /// pre-rework tree; a resident trampoline did not close that
    /// window). The reworked lifecycle writes no guest code page and
    /// calls no mprotect after install: the sibling keeps executing,
    /// the page never loses +x (sampled from /proc/self/maps every
    /// iteration), the broadcast stays one-shot, and the entry stays
    /// redirected.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    #[test]
    fn same_page_executor_never_loses_execute_during_lifecycle() {
        static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static SAW_NO_EXEC: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        static SAMPLES: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);

        let _serial = serial();
        ensure_producer_armed();
        reset_lifecycle();
        STOP.store(false, Ordering::Release);
        SAW_NO_EXEC.store(false, Ordering::Release);
        SAMPLES.store(0, Ordering::Release);

        // Two functions on ONE page: the hooked entry (+0) and the
        // sibling the second thread hammers (+8). One mprotect window
        // covers both — that identity is the point of the regression.
        let (page, _) = code_page(&[
            0x5280_0820, // mov w0, #65 (entry/"init")
            0xD65F_03C0, // ret
            0x5280_0840, // mov w0, #66 (sibling)
            0xD65F_03C0, // ret
        ]);
        let entry_address = page as usize;
        let sibling_address = page as usize + 8;
        let mut registry = HookRegistry::default();
        registry.register_request("il2cpp_init", "guest_page");
        registry.resolve("il2cpp_init", entry_address).expect("resolve");
        // SAFETY: our own RX page; the sibling never runs the entry — the ONLY
        // guest-page write of the lifecycle (pre-guest window).
        let detour = unsafe { registry.apply_detour("il2cpp_init", il2cpp_init_detour as usize) }
            .expect("detour install");
        // SAFETY: the trampoline of the entry just detoured.
        unsafe { INIT_ORIG.set(detour.original) };

        // Sibling hammer: execute + /proc/self/maps +x sample every
        // iteration. A fetch during an execute-removed window faults
        // the process (the red direction); a sample without +x fails
        // the assertion (the observable window).
        let saw = &SAW_NO_EXEC;
        let stop = &STOP;
        let samples = &SAMPLES;
        let hammer = std::thread::spawn(move || {
            // SAFETY: the sibling lives on our own executable page.
            let sibling: unsafe extern "C" fn() -> u32 =
                unsafe { std::mem::transmute(sibling_address) };
            let mut iterations = 0u64;
            while !stop.load(Ordering::Acquire) {
                // SAFETY: our own page's probe.
                let value = unsafe { sibling() };
                assert_eq!(value, 66, "the sibling must keep executing");
                if !page_has_exec(sibling_address) {
                    saw.store(true, Ordering::Release);
                }
                iterations += 1;
                samples.store(iterations, Ordering::Release);
            }
            iterations
        });

        // Overlap is the point: wait until the sibling is REALLY
        // running before driving the lifecycle (a too-fast main thread
        // would stop it before its first iteration).
        let started = std::time::Instant::now();
        while SAMPLES.load(Ordering::Acquire) < 64 {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "sibling thread never started executing"
            );
            std::thread::yield_now();
        }

        // Drive the lifecycle concurrently: the first call broadcasts,
        // the rest run one-shot through the trampoline. The old design
        // restored (and thus mprotected) right after the broadcast.
        // SAFETY: patched entry on our own page.
        let entry: unsafe extern "C" fn(*const c_char) -> c_int =
            unsafe { std::mem::transmute(entry_address) };
        for _ in 0..64 {
            let domain = unsafe { entry(std::ptr::null()) };
            assert_eq!(domain as usize, 65, "entry answers throughout the lifecycle");
        }
        assert_eq!(
            FIRE_COUNT.load(Ordering::Acquire),
            1,
            "one-shot holds under a same-page executor"
        );

        // Keep both sides running a moment longer so the concurrent
        // window is not just the call loop.
        let end = std::time::Instant::now() + std::time::Duration::from_millis(50);
        while std::time::Instant::now() < end {
            let domain = unsafe { entry(std::ptr::null()) };
            assert_eq!(domain as usize, 65);
            std::thread::yield_now();
        }

        stop.store(true, Ordering::Release);
        let total = hammer.join().expect("sibling thread survives the lifecycle");
        assert!(
            total > 0,
            "the sibling actually executed during the lifecycle"
        );
        assert!(
            !SAW_NO_EXEC.load(Ordering::Acquire),
            "the page never lost +x while guest code ran (the 82 NX window)"
        );
        assert!(
            page_has_exec(entry_address),
            "permissions are back to (and stay at) r-x after the lifecycle"
        );
    }
}
