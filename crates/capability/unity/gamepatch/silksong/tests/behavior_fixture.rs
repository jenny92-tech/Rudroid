//! INPUT:  compiled C IL2CPP fixture, pinned Host API and title configuration
//! OUTPUT:  gate, ABI rejection, native bool and real detour behavior checks
//! POS:    exercises Silksong installation and hook calls through production entry points
//! Ticket 54 acceptance: the Silksong gamepatch's REAL install chain against
//! a gcc-compiled fake IL2CPP object model (tests/fixtures/rudroid54_silksong_il2cpp.c,
//! passed via a test-local compile — gcc/cc required).
//!
//! The probe drives the production path end to end:
//! `plugin_init` (config getters + package gate + module subscription) →
//! the module-loaded callback (the kit's `register_il2cpp_post_init`
//! analogue) → `so_symbol` binding → metadata resolution → REAL inline
//! detours (ticket-51 arm64) → observable behavior changes asserted through
//! the fixture's event buffer, return values and the patch's own log lines.
//!
//! Every fixture test takes the SERIAL lock: `plugin_init` arms
//! process-global patch state (the same rule as the ticket-52/53 fixtures).
//! Fixture-vs-real boundary: the guest is synthetic C — no real Unity or
//! Silksong metadata, and no game offsets are involved.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::OnceLock;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::{RudroidModuleEvent, RudroidModuleLoadedCallback, HostStr};
use rudroid_host_config::HostConfig;
use rudroid_test_support::serial_lock as serial;
use silksong_unity6::{applies_to, plugin_init, SilksongFeatures};

// ---- fixture event kinds (mirrored by tests/fixtures/rudroid54_silksong_il2cpp.c) --

const EV_QUIT_NOARGS_BODY: i32 = 1;
const EV_QUIT_CODE_BODY: i32 = 2;
const EV_PARTICLES_ENABLE: i32 = 3;
const EV_PARTICLES_DISABLE: i32 = 4;
const EV_PRELOAD_BODY: i32 = 5;
const EV_MOSS_UPDATE_BODY: i32 = 6;
const EV_CLAMP_BODY: i32 = 7;
const EV_AUTO_BODY: i32 = 8;
const EV_MENU_QUIT_BODY: i32 = 10;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Event {
    kind: c_int,
    a: i64,
    b: i64,
}

// ---- process-global serial lock -------------------------------------------


const IL2CPP_KIND: u32 = rudroid_common::ModuleKind::Il2cpp as u32;
const READY_KIND: u32 = rudroid_common::ModuleKind::Il2cppReady as u32;

// ---- thread-local fake host plumbing --------------------------------------

thread_local! {
    static CONFIG: std::cell::RefCell<Option<HostConfig>> =
        const { std::cell::RefCell::new(None) };
    static FIXTURE_HANDLE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    // Ticket 78: the Il2cppReady subscription sits in its own slot (the
    // real registry keys by kind; the patch holds one subscription each).

}

fn config() -> HostConfig {
    CONFIG.with(|cell| {
        cell.borrow()
            .clone()
            .expect("test config installed on this thread")
    })
}

fn text(value: HostStr<'_>) -> String {
    value.to_cstr().unwrap_or_default().to_string_lossy().into_owned()
}

extern "C" fn fake_get_bool(key: HostStr<'_>, fallback: c_int) -> c_int {
    c_int::from(config().get_bool(&text(key), fallback != 0))
}

extern "C" fn fake_get_i64(key: HostStr<'_>, fallback: i64) -> i64 {
    config().get_i64(&text(key), fallback)
}

extern "C" fn fake_get_f64(key: HostStr<'_>, fallback: f64) -> f64 {
    config().get_f64(&text(key), fallback)
}

unsafe extern "C" fn fake_get_string(
    key: HostStr<'_>,
    fallback: HostStr<'_>,
    out: *mut c_char,
    out_size: u32,
) -> c_int {
    let (key, fallback) = (text(key), text(fallback));
    let hit = config().get_string(&key, &fallback);
    let found = hit.is_some();
    let value = hit.unwrap_or(fallback);
    let bytes = value.as_bytes();
    let limit = (out_size as usize - 1).min(bytes.len());
    assert!(limit < out_size as usize, "buffer contract");
    // SAFETY: the caller's buffer holds `out_size` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, limit);
        *out.add(limit) = 0;
    }
    c_int::from(found)
}

extern "C" fn fake_log(tag: HostStr<'_>, message: HostStr<'_>) {
    log_capture().push(&text(tag), &text(message));
}

extern "C" fn fake_so_symbol(module: *mut c_void, name: HostStr<'_>) -> usize {
    let handle = FIXTURE_HANDLE.with(|cell| cell.get()) as *mut c_void;
    assert_eq!(module, handle, "so_symbol must see the fixture handle");
    // SAFETY: live dlopen handle; NUL-terminated name.
    let address = unsafe { libc::dlsym(handle, name.as_ptr()) };
    address as usize
}

extern "C" fn fake_register_module_loaded(
    kind: u32,
    callback: Option<RudroidModuleLoadedCallback>,
    user: *mut c_void,
) -> c_int {
    // Ticket 81: per-kind first-write-wins lives in the shared fake.
    subscriptions().register(kind, callback, user)
}


// ---- ticket 81: shared test facilities (per-kind subscription fake + log
// capture + serial lock) backed by the rudroid-test-support crate --------

fn log_capture() -> rudroid_test_support::LogCapture {
    rudroid_test_support::LogCapture::shared()
}

fn subscriptions() -> &'static rudroid_test_support::SubscriptionFake {
    static SUBS: OnceLock<rudroid_test_support::SubscriptionFake> = OnceLock::new();
    SUBS.get_or_init(Default::default)
}


fn fake_api() -> HostApiV0 {
    HostApiV0 {
        config_get_bool: fake_get_bool,
        config_get_i64: fake_get_i64,
        config_get_f64: fake_get_f64,
        // SAFETY: the static getter bounds writes, NUL-terminates successes and retains no borrows.
        config_get_string: unsafe { rudroid_common::host_api::ConfigGetStringFn::new(fake_get_string) },
        so_symbol: fake_so_symbol,
        log: fake_log,
        register_module_loaded: fake_register_module_loaded,
        so_symbol_all: fake_so_symbol,
        ..HostApiV0::stubbed()
    }
}

fn logs() -> Vec<String> {
    log_capture().lines()
}

fn logs_contain(needle: &str) -> bool {
    logs().iter().any(|line| line.contains(needle))
}

// ---- fixture plumbing -------------------------------------------------------

fn has_cc() -> bool {
    ["gcc", "cc"].iter().any(|cc| {
        std::process::Command::new(cc)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

struct Fixture {
    #[allow(dead_code)] // kept loaded for the process lifetime
    handle: *mut c_void,
}

/// Compile the fake IL2CPP fixture (unique path per call: fresh code pages
/// per test) and dlopen it. `extra_flags` builds the failure variants.
fn load_fixture(name: &str, extra_flags: &[&str]) -> Fixture {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(if extra_flags.contains(&"-DRUDROID60_LANGUAGE") { "tests/fixtures/language_il2cpp.c" }
            else if extra_flags.contains(&"-DRUDROID54_DEFERRED_QUIT") { "tests/fixtures/quit_il2cpp.c" }
            else if extra_flags.contains(&"-DRUDROID56_POSTPROCESS") { "tests/fixtures/postprocess_il2cpp.c" }
            else if extra_flags.contains(&"-DRUDROID54_LAUNCHER") { "tests/fixtures/launcher_il2cpp.c" } else { "tests/fixtures/rudroid54_silksong_il2cpp.c" });
    let out_dir = std::env::temp_dir().join(format!("rudroid54-silksong-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let so = out_dir.join(format!("{name}.so"));
    let cc = ["gcc", "cc"]
        .iter()
        .find(|cc| {
            std::process::Command::new(cc)
                .arg("--version")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
        .expect("the fixture suite needs gcc or cc");
    let mut command = std::process::Command::new(cc);
    command
        .args(["-shared", "-fPIC", "-O0", "-o"])
        .arg(&so)
        .args(extra_flags)
        .arg(&source);
    let status = command.status().expect("spawn C compiler");
    assert!(status.success(), "compiling {} failed", so.display());
    // SAFETY: RTLD_NOW|RTLD_LOCAL on a compiler-built dependency-free .so.
    let handle = unsafe {
        libc::dlopen(
            CString::new(so.to_str().unwrap().to_owned()).unwrap().as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        )
    };
    assert!(!handle.is_null(), "dlopen {} failed", so.display());
    FIXTURE_HANDLE.with(|cell| cell.set(handle as usize));
    Fixture { handle }
}

impl Fixture {
    fn symbol(&self, name: &str) -> usize {
        // SAFETY: live handle; NUL-free literal name.
        let address = unsafe {
            libc::dlsym(self.handle, CString::new(name).unwrap().as_ptr())
        };
        assert_ne!(address as usize, 0, "fixture must export {name}");
        address as usize
    }

    fn event_count(&self) -> i32 {
        let f: extern "C" fn() -> c_int =
            unsafe { std::mem::transmute(self.symbol("rudroid54_event_count")) };
        f()
    }

    fn event_at(&self, index: i32) -> Option<Event> {
        let mut out = Event { kind: 0, a: 0, b: 0 };
        let f: extern "C" fn(c_int, *mut Event) -> c_int =
            unsafe { std::mem::transmute(self.symbol("rudroid54_event_at")) };
        (f(index, &mut out) != 0).then_some(out)
    }

    fn reset_events(&self) {
        let f: extern "C" fn() = unsafe { std::mem::transmute(self.symbol("rudroid54_reset_events")) };
        f()
    }

    fn count_kind(&self, kind: i32) -> i32 {
        (0..self.event_count())
            .filter_map(|index| self.event_at(index))
            .filter(|event| event.kind == kind)
            .count() as i32
    }

    fn events(&self) -> Vec<Event> {
        (0..self.event_count())
            .filter_map(|index| self.event_at(index))
            .collect()
    }

    fn get<T>(&self, name: &str) -> T {
        let address = self.symbol(name);
        assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        // SAFETY: the export address; the caller picked T to match it.
        unsafe { std::mem::transmute_copy::<usize, T>(&address) }
    }
}

/// The test's package-arm config (mirrors the checked-in device TOML's
/// gamepatch sections plus everything the fixture models).
fn silksong_config(package: &str, patches: &str) -> HostConfig {
    HostConfig::parse(&format!(
        "[package]\npackageName = \"{package}\"\n\n[game_patches.silksong_unity6]\n{patches}\n"
    ))
    .unwrap()
}

const ALL_ON: &str = "\
disable_touch_camera = true
disable_touch_controls = true
disable_watermark_canvas = true
destroy_bootstrap_objects = true
freeze_decorative_behaviours = true
decorative_sprite_percent = 50
disable_ambient_particles = true
disable_heavy_postprocess = true
disable_audio_preloader = true
";

/// Fire the captured Il2cppReady subscription — the point where the
/// il2cpp_hook producer would announce `il2cpp_init` success (ticket 78).
fn fire_il2cpp_ready() {
    let subscription = subscriptions()
        .slot(READY_KIND)
        .map(|(c, u)| unsafe {
            let callback: RudroidModuleLoadedCallback =
                std::mem::transmute(c);
            (callback, u as *mut c_void)
        })
        .expect("plugin_init must subscribe to the Il2cppReady event");
    let (callback, user) = subscription;
    let name = c"runtime/il2cpp_ready";
    let event = RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Il2cppReady as u32,
        base: 0,
        name: rudroid_common::registry::HostStr::new(name),
    };
    callback(Some(&event), user);
}

/// Call the fixture's `il2cpp_init` — flips its metadata API from null to
/// usable, exactly what the real guest's init does.
fn run_fixture_il2cpp_init(fixture: &Fixture) {
    let il2cpp_init: extern "C" fn(*const c_char) -> c_int = fixture.get("il2cpp_init");
    assert_eq!(il2cpp_init(c"fixture".as_ptr()), 1);
}

/// Fire the captured module subscription with the fixture base — the point
/// where the loader would announce the mapped guest libil2cpp.
fn fire_module_event() {
    let subscription = subscriptions()
        .slot(IL2CPP_KIND)
        .map(|(c, u)| unsafe {
            (
                std::mem::transmute::<usize, RudroidModuleLoadedCallback>(c),
                u as *mut c_void,
            )
        })
        .expect("plugin_init must subscribe to module-loaded events");
    let (callback, user) = subscription;
    let name = c"lib/arm64-v8a/libil2cpp.so";
    let event = RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Il2cpp as u32,
        base: FIXTURE_HANDLE.with(|cell| cell.get()),
        name: rudroid_common::registry::HostStr::new(name),
    };
    callback(Some(&event), user);
}

fn was_subscribed() -> bool {
    subscriptions().has_kind(IL2CPP_KIND)
}

/// Resolve a target through the fake object model exactly as the reference
/// patch does (`*(uintptr_t*)mi`).
fn resolve_entry(fixture: &Fixture, ns: &str, class: &str, method: &str, argc: i32) -> usize {
    let domain_get: extern "C" fn() -> usize = fixture.get("il2cpp_domain_get");
    let assemblies: extern "C" fn(usize, *mut usize) -> usize =
        fixture.get("il2cpp_domain_get_assemblies");
    let image_of: extern "C" fn(usize) -> usize = fixture.get("il2cpp_assembly_get_image");
    let class_from_name: extern "C" fn(usize, usize, usize) -> usize =
        fixture.get("il2cpp_class_from_name");
    let method_of: extern "C" fn(usize, usize, i32) -> usize =
        fixture.get("il2cpp_class_get_method_from_name");
    let ns_c = CString::new(ns).unwrap();
    let class_c = CString::new(class).unwrap();
    let method_c = CString::new(method).unwrap();
    let (ns_p, class_p, method_p) = (
        ns_c.as_ptr() as usize,
        class_c.as_ptr() as usize,
        method_c.as_ptr() as usize,
    );
    let domain = domain_get();
    let mut count = 0usize;
    let list = assemblies(domain, &mut count);
    for index in 0..count {
        let image = unsafe { *(list as *const usize).add(index) };
        let image = image_of(image);
        let klass = class_from_name(image, ns_p, class_p);
        if klass != 0 {
            let mi = method_of(klass, method_p, argc);
            assert_ne!(mi, 0, "{ns}.{class}.{method}/{argc} must resolve");
            return unsafe { *(mi as *const usize) };
        }
    }
    panic!("class {ns}.{class} not found in the fake domain");
}

const F32_ASPECT_4_3: f32 = 960.0 / 720.0;

/// Ticket 78 red→green probe, kept as a regression: firing Il2cppReady
/// BEFORE `il2cpp_init` must report TargetMissing for every metadata-driven
/// behavior (the pre-78 divergence — installing against unusable
/// metadata) and leave every hook slot empty. The green half lives in the
/// happy-path test (init before ready → everything installs).
#[test]
fn il2cpp_ready_without_init_reports_missing_and_installs_nothing() {
    let _serial = serial();
    if !has_cc() {
        panic!("the ticket-54 fixture suite needs gcc or cc (no silent skip)");
    }
    let config = silksong_config("com.game.silksong", ALL_ON);
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let _fixture = load_fixture("red78-no-init", &[]);

    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    let rc = unsafe { plugin_init(&api) };
    assert_eq!(rc, 0, "plugin_init must succeed");

    fire_module_event();
    assert!(logs_contain("il2cpp module bound"));
    fire_il2cpp_ready();
    for needle in [
        "target missing: disable_ambient_particles",
        "target missing: disable_audio_preloader",
        "target missing: suppress_application_quit",
        "target missing: silksong_viewport",
    ] {
        assert!(logs_contain(needle), "missing `{needle}` in {:#?}", logs());
    }
    // Slot liveness is process-global (earlier tests may have installed);
    // this cycle's evidence is the log: nothing reports Installed.
    assert!(
        !logs_contain("hook installed"),
        "no hook may install against unusable metadata: {:#?}",
        logs()
    );
}

// ---- the acceptance tests ----------------------------------------------------

/// The full happy path: gate → arm → il2cpp module event → bind → resolve →
/// real detours → OBSERVABLE behavior changes on every modeled reference
/// behavior, with the reference-ordered event sequences.
#[test]
#[cfg(target_arch = "aarch64")]
fn silksong_patch_installs_and_changes_observable_behavior() {
    let _serial = serial();
    if !has_cc() {
        panic!("the ticket-54 fixture suite needs gcc or cc (no silent skip)");
    }
    let config = silksong_config("com.game.silksong", ALL_ON);
    assert!(applies_to(&config));
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("happy", &[]);

    // -- gate + arm ---------------------------------------------------------
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    let rc = unsafe { plugin_init(&api) };
    assert_eq!(rc, 0, "plugin_init must succeed");
    assert!(
        was_subscribed(),
        "a matching package must arm the module subscription"
    );
    assert!(logs_contain("armed for com.game.silksong"));
    assert!(logs_contain("viewport=true"));

    // -- the il2cpp module event: bind + resolve + install -------------------
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(
        logs_contain("il2cpp module bound"),
        "bind evidence: {:?}",
        logs()
    );
    // Every modeled reference hook installed with a real detour.
    for needle in [
        "hook installed: disable_ambient_particles",
        "hook installed: disable_audio_preloader",
        "hook installed: freeze_decorative_behaviours target=MossClump.Update",
        "hook installed: silksong_viewport target=ForceCameraAspect.AutoScaleViewport",
        "hook installed: suppress_application_quit target=UnityEngine.Application.Quit(0)",
        "hook installed: suppress_application_quit target=UnityEngine.Application.Quit(1)",
        "native aspect pipeline installed",
    ] {
        assert!(logs_contain(needle), "missing `{needle}` in {:#?}", logs());
    }
    // Missing decorative classes stay explicit non-installs (native kept).
    assert!(logs_contain(
        "target missing: freeze_decorative_behaviours target=GrassBehaviour.BatchedLateUpdate"
    ));
    // The resolved entries are the fixture's own bodies (model correctness).
    let quit_entry = resolve_entry(&fixture, "UnityEngine", "Application", "Quit", 0);
    assert_eq!(quit_entry, fixture.symbol("rudroid54_quit_noargs_body"));
    // The detour redirects are in place (slot liveness, no guest poking).
    let (quit0, quit1, particles, preloader, viewport) = silksong_unity6::hook_slots();
    assert!(quit0 && quit1 && particles && preloader && viewport);
    // dlopen'd fixture vs in-binary hook bodies: far-range veneer.
    assert!(logs_contain("shape=AbsoluteJump"), "{:#?}", logs());

    // -- observable behavior changes ----------------------------------------
    fixture.reset_events();

    // suppress_application_quit (default ON): Quit never reaches its body.
    let call_quit: extern "C" fn() = fixture.get("rudroid54_call_quit");
    call_quit();
    let call_quit_code: extern "C" fn(i32) = fixture.get("rudroid54_call_quit_code");
    call_quit_code(7);
    assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 0, "Quit must be swallowed");
    assert_eq!(fixture.count_kind(EV_QUIT_CODE_BODY), 0, "Quit(7) must be swallowed");
    assert!(logs_contain("SILK: Application.Quit() suppressed"));
    assert!(logs_contain("SILK: Application.Quit(7) suppressed"));

    // The verified, confirmed UI action retains its original body and reaches
    // both Application overloads. Other calls are suppressed again on return.
    fixture.reset_events();
    let menu_quit: extern "C" fn() = fixture.get("rudroid54_call_menu_quit");
    menu_quit();
    let menu_events = fixture.events();
    assert_eq!(menu_events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [EV_MENU_QUIT_BODY, EV_QUIT_NOARGS_BODY, EV_QUIT_CODE_BODY]);
    assert_ne!(menu_events[0].a, 0, "the original keeps its receiver");
    assert_ne!(menu_events[0].b, 0, "the original keeps its MethodInfo");
    assert_eq!(menu_events[2].a, 7, "the explicit exit code survives");
    call_quit();
    call_quit_code(19);
    assert_eq!(fixture.events().len(), 3, "menu scope cannot leak into later callbacks");
    assert!(logs_contain("confirmed menu quit forwarded through the original action"));

    // disable_ambient_particles: EnableParticles(false) becomes
    // orig(true) + DisableParticles — the reference event sequence.
    let call_particles: extern "C" fn(bool) = fixture.get("rudroid54_call_enable_particles");
    call_particles(false);
    call_particles(true);
    let events_all = fixture.events();
    let particles: Vec<&Event> = events_all
        .iter()
        .filter(|event| event.kind == EV_PARTICLES_ENABLE || event.kind == EV_PARTICLES_DISABLE)
        .collect();
    assert_eq!(particles.len(), 4, "controller pairs for both bool inputs: {events_all:?}");
    for pair in particles.chunks_exact(2) {
        assert_eq!(pair[0].kind, EV_PARTICLES_ENABLE);
        assert_eq!(pair[0].a, 1, "the hook forces the no-scene-particles path");
        assert_eq!(pair[1].kind, EV_PARTICLES_DISABLE);
    }
    assert!(logs_contain("ambient scene particles disabled via controller"));

    // disable_audio_preloader: PreloadClip is a no-op.
    let call_preload: extern "C" fn() = fixture.get("rudroid54_call_preload");
    call_preload();
    call_preload();
    assert_eq!(fixture.count_kind(EV_PRELOAD_BODY), 0, "preloader must be disabled");

    // freeze_decorative_behaviours: MossClump.Update is frozen.
    let call_moss: extern "C" fn() = fixture.get("rudroid54_call_moss_update");
    call_moss();
    call_moss();
    assert_eq!(fixture.count_kind(EV_MOSS_UPDATE_BODY), 0, "decorative Update frozen");

    // silksong_viewport: the native 1.4 lower bound no longer clamps the
    // 4:3 aspect INSIDE the AutoScaleViewport call chain (the scope hook).
    // A DIRECT GetClampedBetween call keeps the native behavior — the
    // reference only removes the lower bound for the scoped subscriber.
    let call_clamp: extern "C" fn(f32) -> f32 = fixture.get("rudroid54_call_clamp");
    assert!((call_clamp(0.0) - 1.4).abs() < 1e-6, "zero outside the viewport scope keeps the native lower bound");
    let direct = call_clamp(F32_ASPECT_4_3);
    assert!(
        (direct - 1.4).abs() < 1e-6,
        "out-of-scope callers keep the native clamp, got {direct}"
    );
    let call_auto: extern "C" fn() -> f32 = fixture.get("rudroid54_call_auto_scale");
    let auto = call_auto();
    assert!(
        (auto - F32_ASPECT_4_3).abs() < 1e-6,
        "AutoScaleViewport must report the unclamped aspect, got {auto}"
    );
    assert!(logs_contain("SSVIEW: native aspect lower bound 1.4000 -> 1.3333"));
    // Reference cap (`reports++ < 8`): the hot-path log stops after eight
    // reports while the unclamped aspect keeps flowing.
    for _ in 0..12 {
        assert!((call_auto() - F32_ASPECT_4_3).abs() < 1e-6);
    }
    let reports = logs()
        .iter()
        .filter(|line| line.contains("native aspect lower bound 1.4000 -> 1.3333"))
        .count();
    assert_eq!(reports, 8, "the lower-bound report is capped at eight");
    // In-range values are untouched, and the scope guard consumed exactly
    // one clamp observation per AutoScaleViewport call (no stray logs).
    let clamped = call_clamp(2.0);
    assert!((clamped - 2.0).abs() < 1e-6);
    assert!(!logs_contain("native aspect lower bound 2.0000"));
    // The event buffer saw the native bodies run underneath the hooks.
    assert!(fixture.count_kind(EV_CLAMP_BODY) >= 2);
    assert!(fixture.count_kind(EV_AUTO_BODY) >= 1);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn menu_quit_exemption_rejects_unknown_assembly_missing_and_ambiguous_or_wrong_abis() {
    let _serial = serial();
    for mode in 1..=9 {
        CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
        subscriptions().reset();
        log_capture().clear();
        let fixture = load_fixture(&format!("menu-quit-rejected-{mode}"), &[]);
        let set_bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_menu_abi");
        set_bad(mode);
        // SAFETY: pinned fixture exports and serial, idle hook installation.
        assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
        fire_module_event();
        run_fixture_il2cpp_init(&fixture);
        fire_il2cpp_ready();
        assert!(logs_contain("target missing: suppress_application_quit target=Silksong UIManager.QuitGame/0"),
            "mode {mode}: {:?}", logs());
        fixture.reset_events();
        let menu_quit: extern "C" fn() = fixture.get("rudroid54_call_menu_quit");
        menu_quit();
        assert_eq!(fixture.count_kind(EV_MENU_QUIT_BODY), 1, "mode {mode}: native action retained");
        assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 0, "mode {mode}: no unchecked exemption");
        assert_eq!(fixture.count_kind(EV_QUIT_CODE_BODY), 0, "mode {mode}: no unchecked exemption");
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn deferred_menu_quit_keeps_original_factory_and_scopes_only_iterator_steps() {
    let _serial = serial();
    for ordinal in [0, 9] {
        CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
        subscriptions().reset();
        log_capture().clear();
        let fixture = load_fixture(&format!("deferred-menu-quit-{ordinal}"), &["-DRUDROID54_DEFERRED_QUIT"]);
        let set_bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_deferred_quit");
        set_bad(ordinal);
        // SAFETY: serial installation into this pinned, compiler-built fixture.
        assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
        fire_module_event();
        run_fixture_il2cpp_init(&fixture);
        fire_il2cpp_ready();
        assert!(logs_contain("hook installed: suppress_application_quit target=GameManager.QuitGame iterator MoveNext/0"), "{:?}", logs());
        fixture.reset_events();
        let menu: extern "C" fn() = fixture.get("rudroid54_call_menu_quit");
        let step: extern "C" fn() -> bool = fixture.get("rudroid54_advance_quit");
        let background_quit: extern "C" fn() = fixture.get("rudroid54_call_quit");
        menu();
        assert_eq!(fixture.count_kind(EV_MENU_QUIT_BODY), 1);
        assert_eq!(fixture.count_kind(12), 1, "original coroutine factory retained");
        background_quit();
        assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 0, "no exemption between UI and iterator callbacks");
        assert!(!step(), "original MoveNext completion is preserved");
        assert_eq!(fixture.count_kind(11), 1);
        assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 1);
        assert_eq!(fixture.count_kind(EV_QUIT_CODE_BODY), 1);
        background_quit();
        assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 1, "no scope after iterator return");
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn deferred_menu_quit_rejects_unknown_factory_nested_types_and_step_abis() {
    let _serial = serial();
    for mode in 1..=9 {
        CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
        subscriptions().reset();
        log_capture().clear();
        let flags = if mode == 9 { vec!["-DRUDROID54_DEFERRED_QUIT", "-DRUDROID54_MISSING_NESTED_TYPES"] }
            else { vec!["-DRUDROID54_DEFERRED_QUIT"] };
        let fixture = load_fixture(&format!("deferred-menu-rejected-{mode}"), &flags);
        let set_bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_deferred_quit");
        set_bad(if mode == 9 { 0 } else { mode });
        // SAFETY: serial installation into this pinned, compiler-built fixture.
        assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
        fire_module_event();
        run_fixture_il2cpp_init(&fixture);
        fire_il2cpp_ready();
        assert!(logs_contain("target missing: suppress_application_quit target=GameManager.QuitGame iterator MoveNext/0"), "mode {mode}: {:?}", logs());
        fixture.reset_events();
        let menu: extern "C" fn() = fixture.get("rudroid54_call_menu_quit");
        let step: extern "C" fn() -> bool = fixture.get("rudroid54_advance_quit");
        menu();
        assert!(!step());
        assert_eq!(fixture.count_kind(EV_MENU_QUIT_BODY), 1);
        assert_eq!(fixture.count_kind(11), 1, "mode {mode}: original iterator retained");
        assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 0, "mode {mode}: no unchecked exemption");
        assert_eq!(fixture.count_kind(EV_QUIT_CODE_BODY), 0, "mode {mode}: no unchecked exemption");
    }
}

/// Switch-off negative contract: pruning switches off leave the native
/// behavior fully intact; the quit hook still installs (reference) but the
/// runtime switch lets the call through; viewport disable keeps the clamp.
#[test]
#[cfg(target_arch = "aarch64")]
fn switches_off_and_viewport_disabled_change_nothing() {
    let _serial = serial();
    let config = silksong_config(
        "com.game.silksong",
        "\
disable_ambient_particles = false
disable_audio_preloader = false
freeze_decorative_behaviours = false
suppress_application_quit = false
",
    );
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("switches-off", &[]);
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();

    for needle in [
        "hook skipped: disable_ambient_particles (switch off)",
        "hook skipped: disable_audio_preloader (switch off)",
        "hook skipped: freeze_decorative_behaviours (switch off)",
    ] {
        assert!(logs_contain(needle), "missing `{needle}` in {:#?}", logs());
    }
    // Viewport: the new key is unset → default on... this config does not
    // disable it, so it installed. Assert the viewport default.
    assert!(logs_contain("hook installed: silksong_viewport"));

    // Native behaviors run untouched.
    fixture.reset_events();
    let call_quit: extern "C" fn() = fixture.get("rudroid54_call_quit");
    call_quit();
    assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 1, "quit passes through");
    let call_particles: extern "C" fn(bool) = fixture.get("rudroid54_call_enable_particles");
    call_particles(false);
    call_particles(true);
    let events = fixture.events();
    let particles: Vec<&Event> = events
        .iter()
        .filter(|event| event.kind == EV_PARTICLES_ENABLE || event.kind == EV_PARTICLES_DISABLE)
        .collect();
    assert_eq!(particles.len(), 2, "one native call per bool input when off: {events:?}");
    assert_eq!(particles[0].a, 0, "native body receives false unchanged");
    assert_eq!(particles[1].a, 1, "native body receives true unchanged");
    let call_preload: extern "C" fn() = fixture.get("rudroid54_call_preload");
    call_preload();
    assert_eq!(fixture.count_kind(EV_PRELOAD_BODY), 1, "preloader native");
    let call_moss: extern "C" fn() = fixture.get("rudroid54_call_moss_update");
    call_moss();
    assert_eq!(fixture.count_kind(EV_MOSS_UPDATE_BODY), 1, "decorative native");
    // Viewport stayed on: the scoped auto call still unclamps.
    let call_auto: extern "C" fn() -> f32 = fixture.get("rudroid54_call_auto_scale");
    assert!((call_auto() - F32_ASPECT_4_3).abs() < 1e-6);
}

/// The NEW viewport key: `game_patches.silksong_viewport.enable = false`
/// keeps the native aspect lower bound (no viewport detours at all).
#[test]
#[cfg(target_arch = "aarch64")]
fn viewport_switch_off_keeps_the_native_clamp() {
    let _serial = serial();
    let config = silksong_config(
        "com.game.silksong",
        "disable_ambient_particles = false\ndisable_audio_preloader = false\nfreeze_decorative_behaviours = false\n[game_patches.silksong_viewport]\nenable = false\n",
    );
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("viewport-off", &[]);
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(logs_contain(
        "hook skipped: silksong_viewport (switch off)"
    ));
    assert!(logs_contain(
        "SSVIEW: viewport disabled by game_patches.silksong_viewport.enable=false"
    ));
    assert!(!logs_contain("native aspect pipeline installed"));
    let call_clamp: extern "C" fn(f32) -> f32 = fixture.get("rudroid54_call_clamp");
    let clamped = call_clamp(F32_ASPECT_4_3);
    assert!((clamped - 1.4).abs() < 1e-6, "native lower bound kept: {clamped}");
}

/// Package-mismatch negative contract: the gate closes BEFORE any
/// subscription exists — firing the (absent) callback is impossible and the
/// guest behavior is untouched.
#[test]
#[cfg(target_arch = "aarch64")]
fn package_mismatch_never_arms_the_patch() {
    let _serial = serial();
    let config = silksong_config("com.teamcherry.hollowknight", ALL_ON);
    assert!(!applies_to(&config), "the crate-level gate must reject");
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("mismatch", &[]);
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0, "gate close is a clean no-op");
    assert!(
        !was_subscribed(),
        "a closed gate must not subscribe to module events"
    );
    assert!(logs_contain(
        "gate closed: package 'com.teamcherry.hollowknight' does not match 'com.game.silksong'"
    ));
    assert!(!logs_contain("armed for"));
    // The old-from_config feature path still reads (old ticket-30 surface).
    let features = SilksongFeatures::from_config(&silksong_config("com.teamcherry.hollowknight", ALL_ON));
    assert!(features.disable_ambient_particles);
    // Guest behavior: with no event fired nothing is installed — prove the
    // native behavior directly (the detour set is empty anyway).
    fixture.reset_events();
    let call_quit: extern "C" fn() = fixture.get("rudroid54_call_quit");
    call_quit();
    assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 1, "native quit ran");
}

/// A guest module missing a required il2cpp export fails the bind explicitly
/// and installs nothing (reference: init returns -1 on missing exports).
#[test]
#[cfg(target_arch = "aarch64")]
fn missing_il2cpp_exports_fail_the_bind_without_side_effects() {
    let _serial = serial();
    let config = silksong_config("com.game.silksong", ALL_ON);
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("stripped", &["-DRUDROID54_MISSING_EXPORTS"]);
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(
        logs_contain("il2cpp export bind failed"),
        "{:#?}",
        logs()
    );
    assert!(logs_contain("il2cpp_string_new"));
    assert!(!logs_contain("hook installed:"));
    // Native behavior intact.
    fixture.reset_events();
    let call_quit: extern "C" fn() = fixture.get("rudroid54_call_quit");
    call_quit();
    assert_eq!(fixture.count_kind(EV_QUIT_NOARGS_BODY), 1);
}

/// Wrong package type in the config: the getter fallback keeps the gate OPEN
/// for the right package name and the behavior stays correct.
#[test]
#[cfg(target_arch = "aarch64")]
fn config_read_goes_through_the_host_api_getters() {
    let _serial = serial();
    // The device config read (ticket-50 style) through the getters: the
    // device TOML's exact gamepatch table must produce the reference view.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let device = HostConfig::from_path(format!(
        "{manifest}/../../../../../work/config.silksong.device.toml"
    ))
    .unwrap();
    assert!(applies_to(&device));
    let features = SilksongFeatures::from_config(&device);
    assert!(features.disable_ambient_particles);
    assert!(features.freeze_decorative_behaviours);
    assert!(features.disable_audio_preloader);
    assert_eq!(features.decorative_sprite_percent, 50);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn wrong_return_metadata_rejects_every_hook_before_patching() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(HostConfig::parse("[package]\npackageName='com.game.silksong'\n[game_patches.silksong_unity6]\ndisable_ambient_particles=true\nfreeze_decorative_behaviours=true\ndisable_audio_preloader=true\n").unwrap()));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("bad-abi", &[]);
    let bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_method_abi");
    bad(1);
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(!logs_contain("hook installed"), "invalid ABI must leave native entries intact: {:#?}", logs());
}

fn fire_player_event(kind: rudroid_common::ModuleKind) {
    let (callback, user) = subscriptions().slot(kind as u32).expect("player subscription");
    // SAFETY: same callback type captured from the production Host API call.
    let callback: RudroidModuleLoadedCallback = unsafe { std::mem::transmute(callback) };
    callback(Some(&RudroidModuleEvent {
        kind: kind as u32, base: 0, name: HostStr::new(c"runtime/unity_frame"),
    }), user as *mut c_void);
}

fn title_frame() { fire_player_event(rudroid_common::ModuleKind::UnityFrame); }
extern "C" fn reenter_title_frame() { title_frame(); }
extern "C" fn stop_inside_scene_query() {
    fire_player_event(rudroid_common::ModuleKind::UnityStopping);
}

fn prepare_title_fixture(name: &str, flags: &[&str]) -> Fixture {
    CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong",
        "unload_assetbundles_after_title=true\nassetbundle_unload_delay_frames=60\n")));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture(name, flags);
    // SAFETY: same-kit Host API, process-pinned fixture and idle detour targets.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    title_frame(); // Metadata not ready; no managed calls permitted.
    assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    fixture
}

fn set_title_scene(fixture: &Fixture, name: &std::ffi::CStr) {
    fixture.get::<extern "C" fn(*const c_char)>("rudroid54_set_scene")(name.as_ptr());
}

fn prepare_postprocess(name: &str, patches: &str, bad: i32, failure: i32) -> Fixture {
    CONFIG.with(|cell| *cell.borrow_mut()=Some(silksong_config("com.game.silksong", &format!(
        "skip_launcher_disclaimer=false\nload_main_menu_directly=false\nskip_unsupported_cinematics=false\n{patches}\n[game_patches.silksong_viewport]\nenable=false\n"))));
    subscriptions().reset();log_capture().clear();
    let fixture=load_fixture(name,&["-DRUDROID56_POSTPROCESS"]);
    fixture.get::<extern "C" fn(i32)>("rudroid56_set_bad")(bad);
    fixture.get::<extern "C" fn(i32)>("rudroid56_set_failure")(failure);
    // SAFETY: compiled process-pinned fixture and same-kit Host API, all targets idle.
    assert_eq!(unsafe {plugin_init(&fake_api())},0);
    fire_module_event();run_fixture_il2cpp_init(&fixture);fire_il2cpp_ready();
    fixture
}
fn assert_roots_released(f: &Fixture) {
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),f.get::<extern "C" fn()->i32>("rudroid56_freed")());
}

fn prepare_language(name: &str, bad: i32, failure: i32) -> Fixture {
    CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
    subscriptions().reset(); log_capture().clear();
    let fixture = load_fixture(name, &["-DRUDROID60_LANGUAGE"]);
    fixture.get::<extern "C" fn(i32)>("rudroid60_bad")(bad);
    fixture.get::<extern "C" fn(i32)>("rudroid60_failure")(failure);
    // SAFETY: same-kit API, pinned fixture and idle metadata-checked entries.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    fire_module_event(); run_fixture_il2cpp_init(&fixture); fire_il2cpp_ready();
    fixture
}

#[test]
#[cfg(target_arch = "aarch64")]
fn selected_language_saves_the_restored_string_after_success_without_frame_work() {
    let _serial = serial();
    let f = prepare_language("language-save", 0, 0);
    assert!(logs_contain("hook installed: persist_selected_language"), "{:?}", logs());
    let switch = f.get::<extern "C" fn(i32) -> bool>("rudroid60_switch");
    let count = f.get::<extern "C" fn(i32) -> i32>("rudroid60_count");
    let saved = f.get::<extern "C" fn() -> *const c_char>("rudroid60_saved");
    for (value, expected) in [(202, c"ZH"), (31, c"DE")] {
        assert!(switch(value));
        // SAFETY: fixture owns a terminated immutable name until its next switch.
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(saved()) }, expected);
        assert_eq!(count(3), count(4), "all callback-local roots released");
    }
    assert!(!switch(-1), "native rejection preserved");
    assert!(!subscriptions().has_kind(rudroid_common::ModuleKind::UnityFrame as u32),
        "language persistence must not subscribe to frames");
    assert_eq!((count(0), count(1), count(2)), (3, 2, 2));
    fire_player_event(rudroid_common::ModuleKind::UnityStopping);
    assert!(switch(44));
    assert_eq!((count(1), count(2)), (2, 2), "stopping forwards original without saving");
}

#[test]
#[cfg(target_arch = "aarch64")]
fn language_rejects_unknown_images_enum_width_byref_and_wrong_method_signatures() {
    let _serial = serial();
    for bad in 1..=7 {
        let f = prepare_language(&format!("language-abi-{bad}"), bad, 0);
        assert!(logs_contain("target missing: persist_selected_language"), "bad={bad}: {:?}", logs());
        assert!(!logs_contain("hook installed: persist_selected_language"));
        assert!(f.get::<extern "C" fn(i32) -> bool>("rudroid60_switch")(202));
        assert_eq!(f.get::<extern "C" fn(i32) -> i32>("rudroid60_count")(1), 0);
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn language_failures_release_roots_keep_original_result_and_do_not_report_saved() {
    let _serial = serial();
    for failure in 1..=8 {
        let f = prepare_language(&format!("language-failure-{failure}"), 0, failure);
        assert!(logs_contain("hook installed: persist_selected_language"), "{:?}", logs());
        assert!(f.get::<extern "C" fn(i32) -> bool>("rudroid60_switch")(202));
        let count = f.get::<extern "C" fn(i32) -> i32>("rudroid60_count");
        assert_eq!(count(2), 0, "failure {failure}: save must not succeed");
        assert_eq!(count(3), count(4), "failure {failure}: no roots retained");
        assert!(!logs_contain("selected language saved:"));
        assert!(logs_contain("language persistence stopped:"));
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn language_stop_during_managed_name_query_cancels_write_and_releases_roots() {
    let _serial = serial();
    let f = prepare_language("language-reentrant-stop", 0, 0);
    f.get::<extern "C" fn(extern "C" fn())>("rudroid60_callback")(stop_inside_scene_query);
    assert!(f.get::<extern "C" fn(i32) -> bool>("rudroid60_switch")(202));
    let count = f.get::<extern "C" fn(i32) -> i32>("rudroid60_count");
    assert_eq!((count(1), count(2)), (0, 0));
    assert_eq!(count(3), count(4));
}
fn post_counts(f: &Fixture) -> Vec<i32> {
    (0..5).map(|i|f.get::<extern "C" fn(i32)->i32>("rudroid56_disabled")(i)).collect()
}

#[test]
#[cfg(target_arch="aarch64")]
fn postprocess_uses_scene_delay_exact_overload_and_releases_all_roots() {
    let _serial=serial();let f=prepare_postprocess("post-delay","disable_heavy_postprocess=true",0,0);
    for _ in 0..40 {title_frame();}
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),0);
    f.get::<extern "C" fn()>("rudroid56_start")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_original_starts")(),1);
    for _ in 0..29 {title_frame();}
    assert_eq!(post_counts(&f),vec![0;5]);
    f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid56_set_callback")(1,reenter_title_frame);
    title_frame();assert_eq!(post_counts(&f),vec![2,1,1,0,0]);assert_roots_released(&f);
    for _ in 0..80 {title_frame();}
    assert_eq!(post_counts(&f),vec![2,1,1,0,0]);
    f.get::<extern "C" fn()>("rudroid56_start")();
    for _ in 0..30 {title_frame();}
    assert_eq!(post_counts(&f),vec![4,2,2,0,0]);assert_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn postprocess_honours_independent_switch_and_ignores_foreign_frame_thread() {
    let _serial=serial();let f=prepare_postprocess("post-background","disable_light_blurred_background=true",0,0);
    f.get::<extern "C" fn()>("rudroid56_start")();title_frame();
    std::thread::spawn(||for _ in 0..80 {title_frame();}).join().unwrap();
    assert_eq!(post_counts(&f),vec![0;5]);
    for _ in 0..29 {title_frame();}
    assert_eq!(post_counts(&f),vec![0,0,0,1,0]);assert_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn postprocess_stopping_during_each_managed_step_cancels_remaining_calls() {
    let _serial=serial();
    for phase in [1,2,3] {
        let f=prepare_postprocess(&format!("post-stop-{phase}"),"disable_heavy_postprocess=true",0,0);
        f.get::<extern "C" fn()>("rudroid56_start")();
        f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid56_set_callback")(phase,stop_inside_scene_query);
        for _ in 0..80 {title_frame();}
        assert_eq!(post_counts(&f),if phase==3 {vec![1,0,0,0,0]} else {vec![0;5]});
        assert_roots_released(&f);
        let created=f.get::<extern "C" fn()->i32>("rudroid56_created")();
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..80 {title_frame();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),created);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn postprocess_rejects_bad_metadata_wrong_objects_and_exceptions_without_retry() {
    let _serial=serial();
    for (bad,failure) in [(1,0),(2,0),(3,0),(4,0),(5,0),(0,1),(0,2),(0,3),(0,4),(0,6),(0,7),(0,8)] {
        let f=prepare_postprocess(&format!("post-fail-{bad}-{failure}"),"disable_heavy_postprocess=true",bad,failure);
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..40 {title_frame();}
        assert_eq!(post_counts(&f),if failure==4 {vec![2,0,0,0,0]} else {vec![0;5]});
        assert!(logs_contain("postprocess runtime unavailable")||logs_contain("postprocess runtime failed"));
        assert_roots_released(&f);
        let created=f.get::<extern "C" fn()->i32>("rudroid56_created")();
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..80 {title_frame();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),created);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn postprocess_off_leaves_scene_entry_and_frame_subscription_untouched() {
    let _serial=serial();let f=prepare_postprocess("post-off","",0,0);
    assert!(subscriptions().slot(rudroid_common::ModuleKind::UnityFrame as u32).is_none());
    f.get::<extern "C" fn()>("rudroid56_start")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_original_starts")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),0);
    assert_eq!(post_counts(&f),vec![0;5]);
}

fn prepare_sprites(name: &str, percent: i32, case: i32, bad: i32, failure: i32) -> Fixture {
    let f=prepare_postprocess(name,&format!("decorative_sprite_percent={percent}"),bad,failure);
    f.get::<extern "C" fn(i32)>("rudroid57_setup")(case);
    f
}
fn sprite_count(f: &Fixture) -> i32 {f.get::<extern "C" fn()->i32>("rudroid57_disabled")()}

#[test]
#[cfg(target_arch="aarch64")]
fn sprites_match_reference_percentage_and_never_unload_or_destroy_resources() {
    let _serial=serial();
    for (percent,expected) in [(25,75),(50,50),(75,25)] {
        let f=prepare_sprites(&format!("sprite-percent-{percent}"),percent,0,0,0);
        f.get::<extern "C" fn()>("rudroid56_start")();
        for _ in 0..29 {title_frame();}assert_eq!(sprite_count(&f),0);
        title_frame();assert_eq!(sprite_count(&f),expected);
        for index in 0..120 {
            assert_eq!(f.get::<extern "C" fn(i32)->i32>("rudroid57_disabled_index")(index),i32::from(index%100>=percent as i32));
        }
        for _ in 0..100 {title_frame();}assert_eq!(sprite_count(&f),expected);
        assert_eq!(f.count_kind(9),0);assert_eq!(post_counts(&f),vec![0;5]);assert_roots_released(&f);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn sprites_protect_gameplay_components_names_and_invalid_component_pairs() {
    let _serial=serial();
    for (case,percent,matched,disabled) in [(1,25,40,15),(2,50,0,0),(3,50,0,0),(4,50,120,50),(5,50,0,0),(6,50,120,50),(7,50,0,0),(8,50,0,0)] {
        let f=prepare_sprites(&format!("sprite-protect-{case}"),percent,case,0,0);
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..30 {title_frame();}
        assert_eq!(sprite_count(&f),disabled);
        assert!(logs_contain(&format!("static decorative sprites matched={matched} disabled={disabled}")));
        if case==1 {
            for i in 0..120 {if i%6!=0 && i%6!=5 {assert_eq!(f.get::<extern "C" fn(i32)->i32>("rudroid57_disabled_index")(i),0);}}
        }
        assert_roots_released(&f);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn sprites_stop_during_nested_queries_and_setter_reentry_without_leaking_roots() {
    let _serial=serial();
    for phase in [1,2,4,5,6,7] {
        let f=prepare_sprites(&format!("sprite-stop-{phase}"),50,0,0,0);
        f.get::<extern "C" fn()>("rudroid56_start")();
        f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid56_set_callback")(phase,stop_inside_scene_query);
        for _ in 0..40 {title_frame();}
        assert_eq!(sprite_count(&f),i32::from(phase==7));assert_roots_released(&f);
        let created=f.get::<extern "C" fn()->i32>("rudroid56_created")();
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..80 {title_frame();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),created);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn sprites_reject_metadata_wrong_objects_and_each_nested_managed_exception() {
    let _serial=serial();
    for (bad,failure) in [(9,0),(0,9),(0,10),(0,11),(0,12),(0,13),(0,14)] {
        let f=prepare_sprites(&format!("sprite-fail-{bad}-{failure}"),50,0,bad,failure);
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..40 {title_frame();}
        assert_eq!(sprite_count(&f),0);assert_roots_released(&f);
        assert!(logs_contain("static Sprite runtime unavailable")||logs_contain("static Sprite runtime failed"));
        let created=f.get::<extern "C" fn()->i32>("rudroid56_created")();
        f.get::<extern "C" fn()>("rudroid56_start")();for _ in 0..80 {title_frame();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_created")(),created);
    }
}

extern "C" fn begin_scene_during_sprite_query() {
    let address=fake_so_symbol(FIXTURE_HANDLE.with(|v|v.get()) as *mut c_void,HostStr::new(c"rudroid56_start"));
    // SAFETY: fixture exports this exact zero-argument C function, process-pinned through the test.
    let start:extern "C" fn()=unsafe {std::mem::transmute(address)};start();
}

#[test]
#[cfg(target_arch="aarch64")]
fn sprites_cancel_old_scene_generation_then_prune_new_scene_once_despite_reentry() {
    let _serial=serial();let f=prepare_sprites("sprite-new-scene",50,0,0,0);
    f.get::<extern "C" fn()>("rudroid56_start")();
    f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid56_set_callback")(5,begin_scene_during_sprite_query);
    for _ in 0..30 {title_frame();}
    assert_eq!(sprite_count(&f),0);assert_roots_released(&f);
    f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid56_set_callback")(5,reenter_title_frame);
    for _ in 0..30 {title_frame();}
    assert_eq!(sprite_count(&f),50);assert_roots_released(&f);
    for _ in 0..80 {title_frame();}assert_eq!(sprite_count(&f),50);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid56_original_starts")(),2);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn title_unload_counts_only_title_frames_once_and_keeps_loaded_objects() {
    let _serial = serial();
    let fixture = prepare_title_fixture("title-once", &[]);
    set_title_scene(&fixture, c"Pre_Menu");
    for _ in 0..100 { title_frame(); }
    assert_eq!(fixture.count_kind(9), 0);
    set_title_scene(&fixture, c"Menu_Title");
    for _ in 0..59 { title_frame(); }
    assert_eq!(fixture.count_kind(9), 0);
    // Recursive frame delivery must not count a second frame or borrow the active engine.
    fixture.get::<extern "C" fn(extern "C" fn())>("rudroid54_set_title_callback")(reenter_title_frame);
    title_frame();
    assert_eq!(fixture.count_kind(9), 1);
    assert_eq!(fixture.events().iter().find(|e| e.kind == 9).unwrap().a, 0);
    let queries = fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")();
    set_title_scene(&fixture, c"Tut_01");
    for _ in 0..100 { title_frame(); }
    set_title_scene(&fixture, c"Menu_Title");
    for _ in 0..100 { title_frame(); }
    assert_eq!(fixture.count_kind(9), 1);
    assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), queries);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn title_unload_cancels_on_scene_exit_and_ignores_foreign_player_thread() {
    let _serial = serial();
    let fixture = prepare_title_fixture("title-cancel", &[]);
    set_title_scene(&fixture, c"Menu_Title");
    title_frame(); // Establish owner.
    std::thread::spawn(title_frame).join().unwrap();
    assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), 1);
    set_title_scene(&fixture, c"Tut_01");
    title_frame();
    set_title_scene(&fixture, c"Menu_Title");
    for _ in 0..100 { title_frame(); }
    assert_eq!(fixture.count_kind(9), 0);
    assert!(logs_contain("cancelled: left Menu_Title"));
}

#[test]
#[cfg(target_arch = "aarch64")]
fn title_unload_never_calls_guest_after_stop_even_when_stop_reenters_query() {
    let _serial = serial();
    for reentrant in [false, true] {
        let fixture = prepare_title_fixture(if reentrant {"title-stop-reentry"} else {"title-stop"}, &[]);
        set_title_scene(&fixture, c"Menu_Title");
        for _ in 0..59 { title_frame(); }
        if reentrant {
            fixture.get::<extern "C" fn(extern "C" fn())>("rudroid54_set_title_callback")(stop_inside_scene_query);
            title_frame(); // Would unload on this frame without the stop check.
        } else {
            fire_player_event(rudroid_common::ModuleKind::UnityStopping);
        }
        let queries = fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")();
        for _ in 0..100 { title_frame(); }
        assert_eq!(fixture.count_kind(9), 0);
        assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), queries);
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn title_unload_managed_exceptions_and_null_results_are_terminal() {
    let _serial = serial();
    for error in [1, 2, 3] {
        let fixture = prepare_title_fixture(&format!("title-error-{error}"), &[]);
        set_title_scene(&fixture, c"Menu_Title");
        fixture.get::<extern "C" fn(i32)>("rudroid54_set_title_error")(error);
        for _ in 0..60 { title_frame(); }
        let queries = fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")();
        fixture.get::<extern "C" fn(i32)>("rudroid54_set_title_error")(0);
        for _ in 0..100 { title_frame(); }
        assert_eq!(fixture.count_kind(9), 0);
        assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), queries);
        assert!(logs_contain("stopped without retry"));
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn title_unload_rejects_missing_string_api_and_incompatible_method_abi() {
    let _serial = serial();
    for bad in 0..=3 {
        let flags = if bad == 0 { vec!["-DRUDROID54_MISSING_TITLE_API"] } else { vec![] };
        let fixture = prepare_title_fixture(&format!("title-abi-{bad}"), &flags);
        fixture.get::<extern "C" fn(i32)>("rudroid54_set_bad_title_abi")(bad);
        set_title_scene(&fixture, c"Menu_Title");
        for _ in 0..100 { title_frame(); }
        assert_eq!(fixture.count_kind(9), 0);
        assert_eq!(fixture.get::<extern "C" fn() -> i32>("rudroid54_title_queries")(), 0);
        assert!(logs_contain("after-title bundle unload unavailable"));
    }
}

#[test]
fn title_unload_default_does_not_subscribe_to_per_frame_events() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
    subscriptions().reset();
    // SAFETY: same-kit pinned Host API; no guest install happens during init.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    assert!(!subscriptions().has_kind(rudroid_common::ModuleKind::UnityFrame as u32));
    assert!(subscriptions().has_kind(rudroid_common::ModuleKind::UnityStopping as u32), "launcher also cancels on stop");
}

fn prepare_launcher_fixture(name: &str, bad: i32, error: i32) -> Fixture {
    prepare_launcher_prompt_fixture(name, bad, error, 0)
}

fn prepare_launcher_prompt_fixture(name: &str, bad: i32, error: i32, prompt: i32) -> Fixture {
    CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", "")));
    subscriptions().reset(); log_capture().clear();
    let fixture=load_fixture(name, &["-DRUDROID54_LAUNCHER"]);
    fixture.get::<extern "C" fn(i32)>("rudroid55_set_bad")(bad);
    fixture.get::<extern "C" fn(i32)>("rudroid55_set_error")(error);
    fixture.get::<extern "C" fn(i32)>("rudroid59_set_mode")(prompt);
    // SAFETY: compiler-built fixture and process-pinned same-kit Host API;
    // metadata installation occurs before any fake managed callback executes.
    assert_eq!(unsafe { plugin_init(&fake_api()) },0);
    fire_module_event(); run_fixture_il2cpp_init(&fixture); fire_il2cpp_ready();
    fixture
}

#[test]
#[cfg(target_arch="aarch64")]
fn launcher_hides_child_panel_before_acceptance_and_keeps_startup_running() {
    let _serial=serial();let f=prepare_launcher_prompt_fixture("launcher-panel",0,0,1);
    f.get::<extern "C" fn()>("rudroid55_call_start")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_visible")(),0);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_hidden")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),1);
    assert!(f.get::<extern "C" fn()->i32>("rudroid59_hide_order")()
        <f.get::<extern "C" fn()->i32>("rudroid59_click_order")());
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),0,
        "hide the panel, keeping the launcher active for its deferred update");
    for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_queries")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_hidden")(),1);
    assert!(logs_contain("disclaimer panel hidden before first frame"));
    bootstrap_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn launcher_panel_failures_and_unknown_field_shapes_keep_native_startup() {
    let _serial=serial();
    // Missing panel, wrong runtime class, find/set exceptions, then invalid
    // scalar/static/byref MsgBox fields. None may disable the launcher or retry.
    for mode in 2..=8 {
        let f=prepare_launcher_prompt_fixture(&format!("launcher-panel-fail-{mode}"),0,0,mode);
        f.get::<extern "C" fn()>("rudroid55_call_start")();
        for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_hidden")(),0);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_visible")(),1);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_queries")(),i32::from(mode<=5));
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),1);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);
        assert!(!logs_contain("disclaimer panel hidden before first frame"));
        bootstrap_roots_released(&f);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn launcher_panel_reentrant_stop_releases_roots_and_cancels_acceptance() {
    let _serial=serial();let f=prepare_launcher_prompt_fixture("launcher-panel-stop",0,0,1);
    f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid58_set_callback")(3,stop_inside_scene_query);
    f.get::<extern "C" fn()>("rudroid55_call_start")();
    for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_hidden")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid59_queries")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),0);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),0);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),0);
    bootstrap_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn launcher_preserves_click_then_requests_real_initialization_scene_once() {
    let _serial=serial(); let f=prepare_launcher_fixture("launcher-normal",0,0);
    f.get::<extern "C" fn()>("rudroid55_call_start")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),1);
    for _ in 0..29 { f.get::<extern "C" fn()>("rudroid55_call_update")(); }
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),0);
    for _ in 0..100 { f.get::<extern "C" fn()>("rudroid55_call_update")(); }
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),1);
}

fn prepare_bootstrap_fixture(name: &str, switches: &str, bad: i32, error: i32) -> Fixture {
    CONFIG.with(|cell| *cell.borrow_mut() = Some(silksong_config("com.game.silksong", switches)));
    subscriptions().reset();log_capture().clear();
    let f=load_fixture(name,&["-DRUDROID54_LAUNCHER"]);
    f.get::<extern "C" fn(i32)>("rudroid58_set_bad")(bad);
    f.get::<extern "C" fn(i32)>("rudroid58_set_error")(error);
    // SAFETY: pinned compiler-built fixture and idle, same-kit installation window.
    assert_eq!(unsafe {plugin_init(&fake_api())},0);
    fire_module_event();run_fixture_il2cpp_init(&f);fire_il2cpp_ready();f
}
fn bootstrap_counts(f: &Fixture) -> Vec<i32> {
    let count=f.get::<extern "C" fn(i32)->i32>("rudroid58_disabled");(0..15).map(|i|count(i)).collect()
}
fn bootstrap_roots_released(f: &Fixture) {
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_created")(),f.get::<extern "C" fn()->i32>("rudroid58_freed")());
}
fn bootstrap_start_and_updates(f: &Fixture) {
    f.get::<extern "C" fn()>("rudroid55_call_start")();
    for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
}
const BOOTSTRAP_ON: &str="disable_touch_camera=true\ndisable_touch_controls=true\ndisable_watermark_canvas=true\n";

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_disables_exact_reference_names_once_after_scene_acceptance() {
    let _serial=serial();let f=prepare_bootstrap_fixture("bootstrap-once",BOOTSTRAP_ON,0,0);
    f.get::<extern "C" fn()>("rudroid55_call_start")();
    for _ in 0..29 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),0);
    for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
    assert_eq!(bootstrap_counts(&f),vec![1;15]);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),15);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),1);
    assert!(logs_contain("disabled bootstrap game objects=15 destroyed=0"));bootstrap_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_switches_select_only_mobile_groups_and_off_makes_no_managed_queries() {
    let _serial=serial();
    for (name,switches,indices) in [("off","",vec![]),("camera","disable_touch_camera=true",vec![0]),
        ("controls","disable_touch_controls=true",(1..14).collect()),("watermark","disable_watermark_canvas=true",vec![14])] {
        let f=prepare_bootstrap_fixture(&format!("bootstrap-switch-{name}"),switches,0,0);bootstrap_start_and_updates(&f);
        for (index,count) in bootstrap_counts(&f).into_iter().enumerate() {assert_eq!(count,i32::from(indices.contains(&index)));}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),indices.len() as i32);
        if indices.is_empty() {assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_created")(),0);}
        bootstrap_roots_released(&f);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_skips_missing_names_without_destroying_objects_or_repeating_work() {
    let _serial=serial();let f=prepare_bootstrap_fixture("bootstrap-missing",&format!("{BOOTSTRAP_ON}destroy_bootstrap_objects=true"),0,1);
    bootstrap_start_and_updates(&f);let counts=bootstrap_counts(&f);
    for (index,count) in counts.iter().enumerate() {assert_eq!(*count,i32::from(index!=13));}
    assert!(logs_contain("destruction remains unavailable"));assert!(logs_contain("objects=14 destroyed=0"));
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),15);bootstrap_roots_released(&f);
}

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_invalid_metadata_wrong_objects_and_managed_failures_release_roots_without_retry() {
    let _serial=serial();
    for (bad,error) in [(1,0),(2,0),(3,0),(4,0),(0,2),(0,3),(0,4),(0,5),(0,6),(0,7),(0,8)] {
        let f=prepare_bootstrap_fixture(&format!("bootstrap-fail-{bad}-{error}"),BOOTSTRAP_ON,bad,error);
        bootstrap_start_and_updates(&f);assert_eq!(bootstrap_counts(&f),vec![0;15]);bootstrap_roots_released(&f);
        assert!(logs_contain("bootstrap deactivation stopped without retry"),"{bad}/{error}: {:?}",logs());
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),1);
        let queries=f.get::<extern "C" fn()->i32>("rudroid58_find_calls")();
        for _ in 0..100 {f.get::<extern "C" fn()>("rudroid55_call_update")();}
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),queries);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_stop_during_string_find_and_setter_cancels_remaining_calls_and_keeps_root_lifetimes() {
    let _serial=serial();
    for phase in [1,2,3] {
        let f=prepare_bootstrap_fixture(&format!("bootstrap-stop-{phase}"),BOOTSTRAP_ON,0,0);
        f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid58_set_callback")(phase,stop_inside_scene_query);
        bootstrap_start_and_updates(&f);
        assert_eq!(bootstrap_counts(&f).iter().sum::<i32>(),i32::from(phase==3));
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),i32::from(phase!=1));
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),0);bootstrap_roots_released(&f);
    }
}

extern "C" fn bootstrap_reenter_launcher_update() {
    let address=fake_so_symbol(FIXTURE_HANDLE.with(|v|v.get()) as *mut c_void,HostStr::new(c"rudroid55_call_update"));
    // SAFETY: exact process-pinned fixture zero-argument export.
    let update:extern "C" fn()=unsafe {std::mem::transmute(address)};update();
}

#[test]
#[cfg(target_arch="aarch64")]
fn bootstrap_callback_reentry_and_failed_scene_request_never_repeat_or_start_pruning() {
    let _serial=serial();let f=prepare_bootstrap_fixture("bootstrap-reenter",BOOTSTRAP_ON,0,0);
    f.get::<extern "C" fn(i32,extern "C" fn())>("rudroid58_set_callback")(2,bootstrap_reenter_launcher_update);
    bootstrap_start_and_updates(&f);assert_eq!(bootstrap_counts(&f),vec![1;15]);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),1);bootstrap_roots_released(&f);
    let f=prepare_bootstrap_fixture("bootstrap-load-fail",BOOTSTRAP_ON,0,0);
    f.get::<extern "C" fn(i32)>("rudroid55_set_error")(2);bootstrap_start_and_updates(&f);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_find_calls")(),0);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid58_created")(),0);
}
#[test]
#[cfg(target_arch="aarch64")]
fn launcher_rejects_scalar_byref_static_fields_and_wrong_scene_abi() {
    let _serial=serial();
    for bad in 1..=6 {
        let f=prepare_launcher_fixture(&format!("launcher-bad-{bad}"),bad,0);
        assert!(!logs_contain("target=GameLauncher.Start shape="));
        f.get::<extern "C" fn()>("rudroid55_call_start")();
        for _ in 0..100 { f.get::<extern "C" fn()>("rudroid55_call_update")(); }
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),0);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),0);
    }
}
#[test]
#[cfg(target_arch="aarch64")]
fn launcher_exceptions_and_reentrant_stop_are_terminal() {
    let _serial=serial();
    for error in 1..=3 {
        let f=prepare_launcher_fixture(&format!("launcher-error-{error}"),0,error);
        if error==3 { f.get::<extern "C" fn(extern "C" fn())>("rudroid55_set_callback")(stop_inside_scene_query); }
        f.get::<extern "C" fn()>("rudroid55_call_start")();
        for _ in 0..100 { f.get::<extern "C" fn()>("rudroid55_call_update")(); }
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_clicks")(),1);
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_loads")(),i32::from(error==2));
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_deactivations")(),0);
    }
}

#[test]
#[cfg(target_arch="aarch64")]
fn unsupported_cinematic_uses_normal_skip_and_cancels_after_stop() {
    let _serial=serial(); let f=prepare_launcher_fixture("cinematic-normal",0,0);
    f.get::<extern "C" fn()>("rudroid55_call_cinematic")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_cinematic_begins")(),1);
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_cinematic_skips")(),1);
    fire_player_event(rudroid_common::ModuleKind::UnityStopping);
    f.get::<extern "C" fn()>("rudroid55_call_cinematic")();
    assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_cinematic_skips")(),1);
}
#[test]
#[cfg(target_arch="aarch64")]
fn unsupported_cinematic_rejects_wrong_abi_and_stops_on_exception() {
    let _serial=serial();
    for (bad,error) in [(7,0),(0,4)] {
        let f=prepare_launcher_fixture(&format!("cinematic-failure-{bad}-{error}"),bad,error);
        for _ in 0..3 { f.get::<extern "C" fn()>("rudroid55_call_cinematic")(); }
        assert_eq!(f.get::<extern "C" fn()->i32>("rudroid55_cinematic_skips")(),i32::from(bad==0));
    }
}
