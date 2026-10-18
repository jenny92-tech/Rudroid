//! INPUT:  live GameLauncher callbacks, checked IL2CPP metadata and typed detours
//! OUTPUT:  hidden startup panel, original button acceptance, deferred initialization and mobile deactivation
//! POS:    startup transition boundary; retains metadata only, never managed objects across frames
#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::Cell;
use std::ffi::{c_void, CStr};
use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};
use rudroid_common::arm64_detour::Original;
use rudroid_common::gamepatch_support::{install_detour, InstallEvent};
use rudroid_common::il2cpp::{Class, Field, Il2cpp, Method, Type, TYPE_BOOLEAN, TYPE_VOID};
use super::{SilksongFeatures, UpdateFn};

const BEHAVIOR: &str = "skip_launcher_disclaimer + load_main_menu_directly";
static START: Original<UpdateFn> = Original::new();
static UPDATE: Original<UpdateFn> = Original::new();
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static STOPPED: AtomicBool = AtomicBool::new(true);
static OBSERVATIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Clone, Copy, Default)]
struct Progress { updates: u32, accepted: bool, attempted: bool }
thread_local! {
    static PROGRESS: Cell<(usize, Progress)> = const { Cell::new((0, Progress { updates: 0, accepted: false, attempted: false })) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}
struct BusyGuard;
impl Drop for BusyGuard { fn drop(&mut self) { BUSY.with(|v| v.set(false)); } }

#[derive(Clone, Copy)]
struct Engine {
    api: Il2cpp, button: Field, get_click: Method, invoke_click: Method,
    load: Option<Method>, get_game_object: Method, set_active: Method,
    hide_prompt_panel: bool,
    bootstrap: super::bootstrap::Selection,
}

fn named(api: Il2cpp, ty: Type, namespace: &str, name: &str) -> bool {
    api.class_from_type(ty).is_some_and(|c| api.class_namespace(c).as_deref() == Some(namespace)
        && api.class_name(c).as_deref() == Some(name))
}
fn method(api: Il2cpp, class: Class, name: &CStr, count: i32) -> Result<Method, &'static str> {
    api.method(class, name, count).ok_or("launcher method metadata unavailable")
}
fn reference_return(api: Il2cpp, m: Method) -> Result<Class, &'static str> {
    let ty = api.method_return_type(m).ok_or("reference return metadata unavailable")?;
    if api.type_kind(ty) != Some(0x12) || api.type_byref(ty) != Some(false) {
        return Err("launcher reference return signature mismatch");
    }
    api.class_from_type(ty).ok_or("reference return class unavailable")
}

impl Engine {
    fn discover(api: Il2cpp, launcher: Class, features: &SilksongFeatures) -> Result<Self, &'static str> {
        for name in ["il2cpp_field_get_value", "il2cpp_runtime_invoke", "il2cpp_string_new"] {
            if !api.has(name) { return Err("launcher runtime export unavailable"); }
        }
        let button = api.field(launcher, c"BtnOk").ok_or("GameLauncher.BtnOk unavailable")?;
        let ty = api.field_type(button).ok_or("BtnOk type unavailable")?;
        if api.field_flags(button).is_none_or(|f| f & 0x10 != 0)
            || api.type_kind(ty) != Some(0x12) || api.type_byref(ty) != Some(false)
            || !named(api, ty, "UnityEngine.UI", "Button") { return Err("BtnOk field signature mismatch"); }
        let button_class = api.class_from_type(ty).ok_or("Button class unavailable")?;
        let get_click = method(api, button_class, c"get_onClick", 0)?;
        if !api.method_shape(get_click, 0, false) { return Err("Button.get_onClick shape mismatch"); }
        let event = reference_return(api, get_click)?;
        let invoke_click = method(api, event, c"Invoke", 0)?;
        if !api.method_signature(invoke_click, false, &[], TYPE_VOID) { return Err("button event Invoke signature mismatch"); }
        let component = api.find_class(c"UnityEngine", c"Component").ok_or("Component unavailable")?;
        let get_game_object = method(api, component, c"get_gameObject", 0)?;
        if !api.method_shape(get_game_object, 0, false) { return Err("Component getter shape mismatch"); }
        let game_object = reference_return(api, get_game_object)?;
        if api.class_namespace(game_object).as_deref() != Some("UnityEngine")
            || api.class_name(game_object).as_deref() != Some("GameObject") { return Err("Component getter type mismatch"); }
        let set_active = method(api, game_object, c"SetActive", 1)?;
        if !api.method_signature(set_active, false, &[(TYPE_BOOLEAN, false)], TYPE_VOID) { return Err("GameObject.SetActive signature mismatch"); }
        let load = if features.load_main_menu_directly {
            let addressables = api.find_class(c"UnityEngine.AddressableAssets", c"Addressables").ok_or("Addressables unavailable")?;
            let load = method(api, addressables, c"LoadSceneAsync", 5)?;
            if !api.method_shape(load, 5, true) { return Err("LoadSceneAsync shape mismatch"); }
            for (index, kind) in [(0, 0x1c), (1, 0x11), (2, 2), (3, 8), (4, 0x11)] {
                let ty = api.method_param(load, index).ok_or("scene argument type unavailable")?;
                if api.type_kind(ty) != Some(kind) || api.type_byref(ty) != Some(false) { return Err("LoadSceneAsync argument signature mismatch"); }
                if kind == 0x11 {
                    let class = api.class_from_type(ty).ok_or("scene enum class unavailable")?;
                    let base = api.enum_base_type(class).ok_or("scene enum underlying type unavailable")?;
                    if api.type_kind(base) != Some(8) || api.type_byref(base) != Some(false) { return Err("scene enum is not Int32"); }
                    let expected = if index == 1 { ("UnityEngine.SceneManagement", "LoadSceneMode") }
                        else { ("UnityEngine.ResourceManagement.ResourceProviders", "SceneReleaseMode") };
                    if !named(api, ty, expected.0, expected.1) { return Err("scene enum identity mismatch"); }
                }
            }
            let ret = api.method_return_type(load).ok_or("scene return metadata unavailable")?;
            if api.type_kind(ret) != Some(0x15) || api.type_byref(ret) != Some(false)
                || !named(api, ret, "UnityEngine.ResourceManagement.AsyncOperations", "AsyncOperationHandle`1") {
                return Err("LoadSceneAsync return signature mismatch");
            }
            Some(load)
        } else { None };
        // Only the Android launcher layout with an instance MsgBox reference
        // owns /Launcher/MsgBox/Panel. Unknown layouts keep their native UI.
        let hide_prompt_panel = api.field(launcher, c"MsgBox").is_some_and(|field| {
            api.field_flags(field).is_some_and(|flags| flags & 0x10 == 0)
                && api.field_type(field).is_some_and(|ty|
                    api.type_kind(ty) == Some(0x12) && api.type_byref(ty) == Some(false))
        });
        Ok(Self { api, button, get_click, invoke_click, load, get_game_object, set_active, hide_prompt_panel,
            bootstrap:super::bootstrap::Selection::from_features(features) })
    }

    /// # Safety
    /// Live GameLauncher receiver of the current managed callback, on its
    /// attached Unity thread. Every reference is local to this call; the
    /// native stack keeps owners visible until their final managed use.
    unsafe fn accept(self, receiver: *mut c_void) -> Result<bool, &'static str> {
        // SAFETY: callback-owned receiver and metadata-checked Button field,
        // getter and event method. No locks or Rust borrows span guest calls.
        unsafe {
            let button = self.api.reference_field(receiver, self.button).ok_or("BtnOk field read failed")?;
            if button.is_null() { return Ok(false); }
            let click = self.api.invoke(self.get_click, button, std::ptr::null_mut()).map_err(|_| "button getter threw")?;
            if click.is_null() || STOPPED.load(Ordering::Acquire) { return Ok(false); }
            self.api.invoke(self.invoke_click, click, std::ptr::null_mut()).map_err(|_| "button event threw")?;
            std::ptr::read_volatile(std::ptr::addr_of!(button));
            std::ptr::read_volatile(std::ptr::addr_of!(click));
            std::ptr::read_volatile(std::ptr::addr_of!(receiver));
        }
        Ok(true)
    }

    /// # Safety
    /// Same callback lifetime/thread contract as accept. No managed pointer
    /// is retained after return, and scene loading is attempted only once.
    unsafe fn transition(self, receiver: *mut c_void) -> Result<(), &'static str> {
        let Some(load) = self.load else { return Ok(()); };
        // SAFETY: all five parameters and both enum widths were verified.
        // Reference arguments use object pointers; scalar values use addresses.
        unsafe {
            let key = self.api.new_string(c"Scenes/Pre_Menu_Intro").ok_or("scene key allocation failed")?;
            let mut single = 0i32; let mut activate = true; let mut priority = 100i32; let mut release = 0i32;
            let mut args = [key, (&mut single as *mut i32).cast(), (&mut activate as *mut bool).cast(),
                (&mut priority as *mut i32).cast(), (&mut release as *mut i32).cast()];
            let handle = self.api.invoke(load, std::ptr::null_mut(), args.as_mut_ptr()).map_err(|_| "initialization scene request threw")?;
            std::ptr::read_volatile(std::ptr::addr_of!(key));
            if handle.is_null() { return Err("initialization scene request returned null"); }
            if STOPPED.load(Ordering::Acquire) { return Ok(()); }
            self.bootstrap.run(self.api, || STOPPED.load(Ordering::Acquire));
            if STOPPED.load(Ordering::Acquire) { return Ok(()); }
            let object = self.api.invoke(self.get_game_object, receiver, std::ptr::null_mut()).map_err(|_| "launcher GameObject getter threw")?;
            if object.is_null() { return Err("launcher GameObject unavailable"); }
            if STOPPED.load(Ordering::Acquire) { return Ok(()); }
            let mut inactive = false; let mut args = [(&mut inactive as *mut bool).cast()];
            self.api.invoke(self.set_active, object, args.as_mut_ptr()).map_err(|_| "launcher deactivation threw")?;
            std::ptr::read_volatile(std::ptr::addr_of!(object));
            std::ptr::read_volatile(std::ptr::addr_of!(receiver));
        }
        Ok(())
    }
}

pub(super) fn stop() { STOPPED.store(true, Ordering::Release); }

/// # Safety
/// Called only from UnityFrame on the owning player thread, after nativeRender.
/// The static getter and its String result are validated before use.
pub(super) unsafe fn observe_scene() {
    if STOPPED.load(Ordering::Acquire) || OBSERVATIONS.fetch_add(1, Ordering::Relaxed) % 180 != 0 { return; }
    let Some(engine) = *super::lock(&ENGINE) else { return; };
    let api = engine.api;
    if !api.thread_attached() { return; }
    let Some(class) = api.find_class(c"UnityEngine", c"Application") else { return; };
    let Some(getter) = api.method(class, c"get_loadedLevelName", 0).filter(|m| api.method_signature(*m, true, &[], 0x0e)) else { return; };
    // SAFETY: validated static String getter on the owning attached thread;
    // copy the callback-local result before another managed call can allocate.
    let scene = unsafe { api.invoke(getter, std::ptr::null_mut(), std::ptr::null_mut())
        .ok().filter(|p| !p.is_null()).and_then(|p| api.string_utf16(p, 256)) };
    if let Some(scene) = scene.and_then(|s| String::from_utf16(&s).ok()) {
        super::host_log_raw(&format!("launcher observed active scene: {scene}"));
    }
}

/// # Safety
/// Receiver/metadata are the live arguments of a validated Unity callback;
/// the original trampoline remains mapped throughout the call.
unsafe fn callback(receiver: *mut c_void, mi: *mut c_void, starting: bool) {
    let orig = if starting { START.get() } else { UPDATE.get() };
    if let Some(orig) = orig {
        // SAFETY: same native callback ABI and live arguments as entry.
        unsafe { orig(receiver, mi) };
    }
    if receiver.is_null() || STOPPED.load(Ordering::Acquire) { return; }
    let Some(engine) = *super::lock(&ENGINE) else { return; };
    if !engine.api.thread_attached() || BUSY.with(|v| v.replace(true)) { return; }
    let _guard = BusyGuard;
    let identity = receiver as usize;
    let mut progress = PROGRESS.with(|v| {
        let (id, p) = v.get(); if starting || id != identity { Progress::default() } else { p }
    });
    if progress.attempted { return; }
    progress.updates = progress.updates.saturating_add(u32::from(!starting));
    // Publish a terminal sentinel before calling external code. Reentry sees
    // BUSY; unwind leaves this attempt consumed and restores BUSY via Drop.
    PROGRESS.with(|v| v.set((identity, Progress { attempted: true, ..progress })));
    if starting && engine.hide_prompt_panel {
        super::bootstrap::hide_launcher_prompt(engine.api, || STOPPED.load(Ordering::Acquire));
        if STOPPED.load(Ordering::Acquire) { return; }
    }
    if !progress.accepted && (starting || progress.updates >= 30) {
        // SAFETY: current callback receiver stays reachable on this stack.
        match unsafe { engine.accept(receiver) } {
            Ok(accepted) => progress.accepted = accepted,
            Err(reason) => { super::host_log_raw(&format!("launcher acceptance stopped: {reason}")); return; }
        }
    }
    if STOPPED.load(Ordering::Acquire) { return; }
    if !starting && progress.updates >= 30 && progress.accepted {
        // SAFETY: receiver belongs to this active callback, not stored state.
        let result = unsafe { engine.transition(receiver) };
        match result {
            Ok(()) => super::host_log_raw("launcher initialization scene request accepted once"),
            Err(reason) => super::host_log_raw(&format!("launcher transition stopped without retry: {reason}")),
        }
        return;
    }
    PROGRESS.with(|v| v.set((identity, progress)));
}

unsafe extern "C-unwind" fn start(receiver: *mut c_void, mi: *mut c_void) {
    // SAFETY: installed only against validated instance void Start().
    unsafe { callback(receiver, mi, true) };
}
unsafe extern "C-unwind" fn update(receiver: *mut c_void, mi: *mut c_void) {
    // SAFETY: installed only against validated instance void Update().
    unsafe { callback(receiver, mi, false) };
}

/// # Safety
/// Same quiescent, process-pinned install contract as the parent title patch.
pub(super) unsafe fn install(api: Il2cpp, features: &SilksongFeatures) -> Vec<InstallEvent> {
    stop(); *super::lock(&ENGINE) = None;
    if !features.skip_launcher_disclaimer { return vec![InstallEvent::Skipped { behavior: BEHAVIOR, reason: "switch off" }]; }
    let discovered = (|| {
        let class = api.find_class_named(c"GameLauncher").ok_or("GameLauncher unavailable")?;
        let engine = Engine::discover(api, class, features)?;
        let start = method(api, class, c"Start", 0)?;
        let update = method(api, class, c"Update", 0)?;
        if !api.method_signature(start, false, &[], TYPE_VOID) || !api.method_signature(update, false, &[], TYPE_VOID) {
            return Err("GameLauncher callback signature mismatch");
        }
        Ok((engine, api.method_entry(start).ok_or("Start entry unavailable")?, api.method_entry(update).ok_or("Update entry unavailable")?))
    })();
    let (engine, a, b) = match discovered {
        Ok(v) => v,
        Err(reason) => return vec![InstallEvent::TargetMissing { behavior: BEHAVIOR, target: reason.to_owned() }],
    };
    let mut events = Vec::new();
    // SAFETY: fully checked callback ABIs; host supplies idle target window.
    unsafe {
        for (target, entry, original, replacement) in [("GameLauncher.Start", a, &START, start as UpdateFn), ("GameLauncher.Update", b, &UPDATE, update as UpdateFn)] {
            match install_detour(BEHAVIOR, target, entry, original, replacement) {
                Ok(hook) => events.push(InstallEvent::Installed { behavior: BEHAVIOR, target: target.to_owned(), shape: hook.shape }),
                Err(e) => { events.push(e); return events; }
            }
        }
    }
    *super::lock(&ENGINE) = Some(engine);
    STOPPED.store(false, Ordering::Release);
    events
}
