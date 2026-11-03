//! INPUT:  compiled C IL2CPP camera fixture and a pinned Host API
//! OUTPUT:  real detour, metadata ABI, layout and viewport behavior checks
//! POS:    exercises Hollow Knight hooks through production subscription and installation
//! Ticket 54 acceptance (HK side): the hollow_knight gamepatch's REAL
//! install chain against a gcc-compiled fake IL2CPP object model
//! (tests/fixtures/rudroid54_hk_il2cpp.c). Same production path as the Silksong
//! suite: plugin_init (ungated) → module-loaded event → so_symbol binding →
//! metadata + FIELD-OFFSET resolution → real detours → observable viewport
//! behavior changes. The SERIAL lock covers the process-global patch state
//! (ticket-52/53 rule). Synthetic fixture only — no real HK metadata.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::OnceLock;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::{RudroidModuleEvent, RudroidModuleLoadedCallback, HostStr};
use rudroid_host_config::HostConfig;
use rudroid_test_support::serial_lock as serial;

const IL2CPP_KIND: u32 = rudroid_common::ModuleKind::Il2cpp as u32;
const READY_KIND: u32 = rudroid_common::ModuleKind::Il2cppReady as u32;
use hollow_knight_viewport::{applies_to, plugin_init, ViewportFeatures};

// ---- fixture event kinds (mirrored by tests/fixtures/rudroid54_hk_il2cpp.c) --------

const EV_RECT_BODY: i32 = 1;
const EV_ORTHO_BODY: i32 = 2;
const EV_TK2D_MATRIX_BODY: i32 = 3;
const EV_OVERSCANVP_BODY: i32 = 4;
const EV_SET_OVERSCAN_BODY: i32 = 5;
const EV_AUTO_BODY: i32 = 6;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Event {
    kind: c_int,
    a: i64,
    b: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

// ---- process-global serial lock -------------------------------------------


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

fn load_fixture(name: &str) -> Fixture {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rudroid54_hk_il2cpp.c");
    let out_dir = std::env::temp_dir().join(format!("rudroid54-hk-{}", std::process::id()));
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
    let status = std::process::Command::new(cc)
        .args(["-shared", "-fPIC", "-O0", "-o"])
        .arg(&so)
        .arg(&source)
        .status()
        .expect("spawn C compiler");
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
            unsafe { std::mem::transmute(self.symbol("rudroid54_hk_event_count")) };
        f()
    }

    fn event_at(&self, index: i32) -> Option<Event> {
        let mut out = Event { kind: 0, a: 0, b: 0 };
        let f: extern "C" fn(c_int, *mut Event) -> c_int =
            unsafe { std::mem::transmute(self.symbol("rudroid54_hk_event_at")) };
        (f(index, &mut out) != 0).then_some(out)
    }

    fn reset_events(&self) {
        let f: extern "C" fn() =
            unsafe { std::mem::transmute(self.symbol("rudroid54_hk_reset_events")) };
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

    // state getters
    fn main_rect(&self) -> Rect {
        let f: extern "C" fn(*mut Rect) = self.get("rudroid54_hk_main_rect");
        let mut rect = Rect::default();
        f(&mut rect);
        rect
    }

    fn hud_rect(&self) -> Rect {
        let f: extern "C" fn(*mut Rect) = self.get("rudroid54_hk_hud_rect");
        let mut rect = Rect::default();
        f(&mut rect);
        rect
    }

    fn main_ortho(&self) -> f32 {
        let f: extern "C" fn() -> f32 = self.get("rudroid54_hk_main_ortho");
        f()
    }

    fn hud_ortho(&self) -> f32 {
        let f: extern "C" fn() -> f32 = self.get("rudroid54_hk_hud_ortho");
        f()
    }

    fn tk2d_ortho(&self) -> f32 {
        let f: extern "C" fn() -> f32 = self.get("rudroid54_hk_tk2d_ortho");
        f()
    }

    fn tk2d_ppm(&self) -> f32 {
        let f: extern "C" fn() -> f32 = self.get("rudroid54_hk_tk2d_ppm");
        f()
    }

    fn tk2d_zoom(&self) -> f32 {
        let f: extern "C" fn() -> f32 = self.get("rudroid54_hk_tk2d_zoom");
        f()
    }
}

fn hk_config(patches: &str) -> HostConfig {
    let body = if patches.is_empty() {
        String::new()
    } else {
        format!("\n[game_patches.hollow_knight_viewport]\n{patches}\n")
    };
    HostConfig::parse(&format!(
        "[package]\npackageName = \"com.teamcherry.hollowknight\"\n{body}"
    ))
    .unwrap()
}

/// Call the fixture's `il2cpp_init` — flips its metadata API from null to
/// usable, exactly what the real guest's init does.
fn run_fixture_il2cpp_init(fixture: &Fixture) {
    let il2cpp_init: extern "C" fn(*const c_char) -> c_int = fixture.get("il2cpp_init");
    assert_eq!(il2cpp_init(c"fixture".as_ptr()), 1);
}

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

/// The fake screen is 960x720 (aspect 4:3) — the reference width-safe scale
/// is (16/9)/(4/3) = 4/3.
const SCALE: f32 = 4.0f32 / 3.0f32;

/// Regression for the real device's clipped HUD: cameras initialized through
/// native/injected paths do not necessarily call the managed ortho setter.
/// All three game callbacks must repair their cameras after the original,
/// while repeated callbacks must never compound the existing scale.
#[test]
#[cfg(target_arch = "aarch64")]
fn native_initialized_hud_is_corrected_after_all_camera_callbacks_without_rescaling() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(hk_config("")));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("native-initialized-hud");
    let api = fake_api();
    // SAFETY: the owned fixture and callbacks are pinned, entries are idle.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    let call_auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    let call_viewport: extern "C" fn(f32) = fixture.get("rudroid54_hk_call_set_overscan_viewport");
    let call_cameras: extern "C" fn(f32) = fixture.get("rudroid54_hk_call_set_overscan");
    assert_eq!(fixture.hud_ortho(), 2.0);
    call_auto();
    assert!((fixture.hud_ortho() - 2.0 * SCALE).abs() < 1e-4, "native-initialized HUD needs the post-call correction");
    for _ in 0..32 {
        call_auto();
        call_viewport(0.25);
        call_cameras(0.75);
        assert!((fixture.hud_ortho() - 2.0 * SCALE).abs() < 1e-4, "HUD correction must stay idempotent");
        assert!((fixture.main_ortho() - 3.0 * SCALE).abs() < 1e-4, "main camera correction must stay idempotent");
        assert_eq!(fixture.hud_rect(), Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
        assert_eq!(fixture.main_rect(), Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    }
}

// ---- the acceptance tests ----------------------------------------------------

/// The happy path with REFERENCE DEFAULTS (all switches on except debug):
/// ungated arm → il2cpp module event → bind → resolve (incl. tk2d field
/// offsets) → real detours → observable viewport changes.
/// Ticket 78 red→green probe, kept as a regression: firing Il2cppReady
/// BEFORE `il2cpp_init` must report TargetMissing for every hook (the
/// pre-78 divergence — installing against unusable metadata) and leave
/// every hook slot empty. The green half lives in the happy-path test.
#[test]
fn il2cpp_ready_without_init_reports_missing_and_installs_nothing() {
    let _serial = serial();
    if !has_cc() {
        panic!("the ticket-54 fixture suite needs gcc or cc (no silent skip)");
    }
    let config = hk_config("");
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let _fixture = load_fixture("red78-no-init");

    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    let rc = unsafe { plugin_init(&api) };
    assert_eq!(rc, 0, "plugin_init must succeed");

    fire_module_event();
    assert!(logs_contain("il2cpp module bound"));
    fire_il2cpp_ready();
    for needle in [
        // HK's viewport stage reports the failed image walk as one grouped
        // diagnostic (reference text: "native viewport unchanged").
        "image missing — native viewport unchanged",
        "target missing: hollow_knight_viewport target=Assembly-CSharp / UnityEngine.CoreModule images",
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

#[test]
#[cfg(target_arch = "aarch64")]
fn hk_patch_installs_and_adapts_the_viewport() {
    let _serial = serial();
    if !has_cc() {
        panic!("the ticket-54 fixture suite needs gcc or cc (no silent skip)");
    }
    let config = hk_config("");
    assert!(applies_to(&config));
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("happy");

    // Ungated: arms for ANY package (reference parity decision).
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    assert!(was_subscribed());
    assert!(logs_contain(
        "ungated reference parity: presence in the deploy dir is the activation switch"
    ));

    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    for needle in [
        "hooked ForceCameraAspect.SetOverscanViewport(1)",
        "hooked ForceCameraAspect.AutoScaleViewport(0)",
        "hooked GameCameras.SetOverscan(1)",
        "force_full_viewport=1 camera.set_rect_Injected",
        "tk2dCamera fields settings=0x20 zoom=0x40",
        "plugin armed: full_viewport=1 tk2d_width_safe=1 ui_ortho_safe=1 debug=0",
    ] {
        assert!(logs_contain(needle), "missing `{needle}` in {:#?}", logs());
    }
    let (rect, ortho, tk2d, auto, offsets) = hollow_knight_viewport::hook_slots();
    assert!(rect && ortho && tk2d && auto && offsets);
    assert!(logs_contain("shape=AbsoluteJump"), "{:#?}", logs());
    // ui_orthographic_width_safe installed as well (default on).
    assert!(
        logs().iter().any(|line| {
            line.contains("hook installed: ui_orthographic_width_safe")
        }),
        "{:#?}",
        logs()
    );

    // -- observable behavior changes ----------------------------------------

    // force_full_viewport: any rect through set_rect_Injected comes out full.
    fixture.reset_events();
    let call_rect: extern "C" fn(i32, f32, f32, f32, f32) =
        fixture.get("rudroid54_hk_call_set_rect");
    call_rect(1, 0.2, 0.2, 0.6, 0.6);
    let rect = fixture.main_rect();
    assert_eq!(
        rect,
        Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
        "the rect must be forced full, got {rect:?}"
    );
    assert_eq!(fixture.count_kind(EV_RECT_BODY), 1, "the original still ran");
    assert!(logs_contain("rect 0.200,0.200 0.600x0.600 -> full"));

    // ui_orthographic_width_safe: set_orthographicSize(3.0) lands 4.0.
    let call_ortho: extern "C" fn(i32, f32) = fixture.get("rudroid54_hk_call_set_ortho");
    call_ortho(1, 3.0);
    let ortho = fixture.main_ortho();
    assert!(
        (ortho - 3.0 * SCALE).abs() < 1e-4,
        "ortho must be width-scaled: expected {}, got {ortho}",
        3.0 * SCALE
    );
    assert_eq!(fixture.count_kind(EV_ORTHO_BODY), 1, "original saw the scaled value");

    // tk2d_width_safe: UpdateCameraMatrix rescales ortho/ppm/zoom, and the
    // slot cache keeps a SECOND pass idempotent.
    let call_matrix: extern "C" fn() = fixture.get("rudroid54_hk_call_update_matrix");
    call_matrix();
    call_matrix();
    assert!((fixture.tk2d_ortho() - 3.0 * SCALE).abs() < 1e-4, "tk2d ortho");
    assert!((fixture.tk2d_ppm() - 100.0 / SCALE).abs() < 1e-2, "tk2d ppm");
    assert!((fixture.tk2d_zoom() - 1.0 / SCALE).abs() < 1e-4, "tk2d zoom");
    // The original body ran twice and observed the ALREADY patched ortho
    // both times (no double-scaling).
    let all_events = fixture.events();
    let matrix_events: Vec<&Event> = all_events
        .iter()
        .filter(|event| event.kind == EV_TK2D_MATRIX_BODY)
        .collect();
    assert_eq!(matrix_events.len(), 2);
    for event in matrix_events {
        assert!(
            (f32::from_bits(event.a as u32) - 3.0 * SCALE).abs() < 1e-4,
            "the original must observe the patched ortho, got {}",
            f32::from_bits(event.a as u32)
        );
    }

    // force_full_viewport: AutoScaleViewport returns exactly 1.0 (native 4:3
    // value would be 4/3), and the HUD fixup pushed a full rect through the
    // original set_rect.
    let call_auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    let auto = call_auto();
    assert!((auto - 1.0).abs() < 1e-6, "AutoScaleViewport must be 1.0, got {auto}");
    assert_eq!(fixture.count_kind(EV_AUTO_BODY), 1, "native body ran under the hook");
    let hud = fixture.hud_rect();
    assert_eq!(
        hud,
        Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
        "the HUD camera must be forced full"
    );

    // Always-on pass-through group: values reach the originals unchanged.
    let call_ovp: extern "C" fn(f32) = fixture.get("rudroid54_hk_call_set_overscan_viewport");
    call_ovp(0.25);
    assert_eq!(fixture.count_kind(EV_OVERSCANVP_BODY), 1);
    assert!(logs_contain("SetOverscanViewport adjustment=0.2500"));
    let call_overscan: extern "C" fn(f32) = fixture.get("rudroid54_hk_call_set_overscan");
    call_overscan(0.75);
    assert_eq!(fixture.count_kind(EV_SET_OVERSCAN_BODY), 1);
    assert!(logs_contain("GameCameras.SetOverscan value=0.7500"));
}

/// Switch-off contract: force_full_viewport = false keeps the native rect,
/// native AutoScaleViewport value, and stops the HUD fixup; the always-on
/// pass-through group still installs (reference semantics).
#[test]
#[cfg(target_arch = "aarch64")]
fn force_full_off_keeps_the_native_viewport() {
    let _serial = serial();
    let config = hk_config("force_full_viewport = false\n");
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("force-full-off");
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(logs_contain("hook skipped: force_full_viewport (switch off)"));
    // The always-on group still hooks.
    assert!(logs_contain("hooked ForceCameraAspect.SetOverscanViewport(1)"));
    assert!(logs_contain("hooked GameCameras.SetOverscan(1)"));

    fixture.reset_events();
    // Native rect passes through untouched.
    let call_rect: extern "C" fn(i32, f32, f32, f32, f32) =
        fixture.get("rudroid54_hk_call_set_rect");
    call_rect(1, 0.2, 0.2, 0.6, 0.6);
    let rect = fixture.main_rect();
    assert_eq!(
        rect,
        Rect { x: 0.2, y: 0.2, width: 0.6, height: 0.6 },
        "native rect must survive untouched"
    );
    // Native AutoScaleViewport value (4/3 at the fake 4:3 screen).
    let call_auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    let auto = call_auto();
    assert!(
        (auto - SCALE).abs() < 1e-6,
        "native aspect fill value expected, got {auto}"
    );
    assert!(!logs_contain("full camera rect applied"));
    // The pass-through hooks still log their observations.
    let call_overscan: extern "C" fn(f32) = fixture.get("rudroid54_hk_call_set_overscan");
    call_overscan(0.5);
    assert_eq!(fixture.count_kind(EV_SET_OVERSCAN_BODY), 1);
}

/// The legacy config dialect (`hk_viewport.*`) still drives the switches,
/// and the new key wins when both exist (reference cfg_bool chain).
#[test]
#[cfg(target_arch = "aarch64")]
fn legacy_dialect_still_controls_the_switches() {
    let _serial = serial();
    let config = HostConfig::parse(
        "[package]\npackageName = \"com.teamcherry.hollowknight\"\n\n[hk_viewport]\nforce_full_viewport = false\nui_orthographic_width_safe = false\ntk2d_width_safe = false\n",
    )
    .unwrap();
    let features = ViewportFeatures::from_config(&config);
    assert!(!features.force_full_viewport);
    assert!(!features.ui_orthographic_width_safe);
    assert!(!features.tk2d_width_safe);
    assert!(!features.debug);

    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("legacy-off");
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    // All three switch-gated hooks skipped; always-on group installed.
    assert!(logs_contain("hook skipped: force_full_viewport (switch off)"));
    assert!(logs_contain("hook skipped: ui_orthographic_width_safe (switch off)"));
    assert!(logs_contain("hook skipped: tk2d_width_safe (switch off)"));
    assert!(logs_contain("hooked ForceCameraAspect.SetOverscanViewport(1)"));
    // Native behaviors intact.
    fixture.reset_events();
    let call_ortho: extern "C" fn(i32, f32) = fixture.get("rudroid54_hk_call_set_ortho");
    call_ortho(1, 3.0);
    assert!((fixture.main_ortho() - 3.0).abs() < 1e-6, "native ortho kept");
    let call_matrix: extern "C" fn() = fixture.get("rudroid54_hk_call_update_matrix");
    call_matrix();
    assert!((fixture.tk2d_ortho() - 3.0).abs() < 1e-6, "native tk2d ortho kept");
}

/// The ungated patch activates for a DIFFERENT package too — the reference
/// behavior (and the reason Titles are isolated by deployment, not code).
#[test]
#[cfg(target_arch = "aarch64")]
fn ungated_activates_for_any_package() {
    let _serial = serial();
    for package in [
        "com.teamcherry.hollowknight",
        "com.game.silksong",
        "some.other.game",
    ] {
        let config = HostConfig::parse(&format!(
            "[package]\npackageName = \"{package}\"\n"
        ))
        .unwrap();
        assert!(applies_to(&config), "{package} must apply (ungated)");
    }
    // Behavioral end-to-end proof on a non-HK package name.
    let config = HostConfig::parse("[package]\npackageName = \"com.game.silksong\"\n").unwrap();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(config));
    subscriptions().reset();
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("ungated-silksong-pkg");
    let api = fake_api();
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&api) }, 0);
    fire_module_event();
    // Ticket 78: metadata becomes usable only after il2cpp_init returns.
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(logs_contain("hooked ForceCameraAspect.SetOverscanViewport(1)"));
}

#[test]
#[cfg(target_arch = "aarch64")]
fn wrong_return_metadata_rejects_every_hook_before_patching() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(hk_config("")));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("bad-abi");
    let bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_method_abi");
    bad(1);
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(!logs_contain("hook installed"), "invalid ABI must leave native entries intact: {:#?}", logs());
    let auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    assert!((auto() - 4. / 3.).abs() < 1e-6);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn matching_type_kinds_with_wrong_classes_skip_native_layout_access() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(hk_config("")));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("bad-reference-types");
    let bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_reference_types");
    bad(1);
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(logs_contain("target missing: force_full_viewport target=Camera.set_rect_Injected"));
    assert!(logs_contain("tk2dCamera/tk2dCameraSettings field offsets"));
    let matrix: extern "C" fn() = fixture.get("rudroid54_hk_call_update_matrix");
    let auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    matrix();
    auto();
    assert_eq!(fixture.tk2d_ortho(), 3.0);
    assert_eq!(fixture.tk2d_ppm(), 100.0);
    assert_eq!(fixture.tk2d_zoom(), 1.0);
    assert_eq!(fixture.hud_rect().width, 0.8);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn matching_names_with_distinct_metadata_types_skip_native_layout_access() {
    let _serial = serial();
    CONFIG.with(|cell| *cell.borrow_mut() = Some(hk_config("")));
    subscriptions().reset();
    log_capture().clear();
    let fixture = load_fixture("shadow-reference-types");
    let bad: extern "C" fn(i32) = fixture.get("rudroid54_set_bad_reference_types");
    bad(2);
    // SAFETY: pinned fixture exports and serial, idle hook installation.
    assert_eq!(unsafe { plugin_init(&fake_api()) }, 0);
    fire_module_event();
    run_fixture_il2cpp_init(&fixture);
    fire_il2cpp_ready();
    assert!(logs_contain("target missing: force_full_viewport target=Camera.set_rect_Injected"));
    assert!(logs_contain("tk2dCamera/tk2dCameraSettings field offsets"));
    let matrix: extern "C" fn() = fixture.get("rudroid54_hk_call_update_matrix");
    let auto: extern "C" fn() -> f32 = fixture.get("rudroid54_hk_call_auto_scale");
    matrix();
    auto();
    assert_eq!(fixture.tk2d_ortho(), 3.0);
    assert_eq!(fixture.tk2d_ppm(), 100.0);
    assert_eq!(fixture.tk2d_zoom(), 1.0);
    assert_eq!(fixture.hud_rect().width, 0.8);
}
