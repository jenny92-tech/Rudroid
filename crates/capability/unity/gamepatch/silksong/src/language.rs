//! INPUT:  checked TeamCherry language-switch callbacks and Unity PlayerPrefs methods
//! OUTPUT:  successful language selections saved under the key used by the next startup
//! POS:    Silksong localization bridge; callback-local roots, no frame work or cached game objects
#![deny(unsafe_op_in_unsafe_fn)]

use std::{cell::Cell, ffi::{c_void, CStr}, sync::{atomic::{AtomicBool, Ordering}, Mutex}};
use rudroid_common::{arm64_detour::Original, gamepatch_support::{install_detour, InstallEvent},
    il2cpp::{Class, GcHandles, Il2cpp, Method, Type, TYPE_BOOLEAN, TYPE_STRING, TYPE_VOID}};

const BEHAVIOR: &str = "persist_selected_language";
type Switch = unsafe extern "C-unwind" fn(i32, *mut c_void) -> bool;
static ORIGINAL: Original<Switch> = Original::new();
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static STOPPED: AtomicBool = AtomicBool::new(true);
thread_local! { static BUSY: Cell<bool> = const { Cell::new(false) }; }
struct Guard;
impl Drop for Guard { fn drop(&mut self) { BUSY.with(|value| value.set(false)); } }

#[derive(Clone, Copy)]
struct Engine { api: Il2cpp, roots: GcHandles, code: Class, current: Method,
    to_string: Method, set_string: Method, save: Method }

fn language_code(api: Il2cpp, ty: Type, code: Class) -> bool {
    api.type_kind(ty) == Some(0x11) && api.type_byref(ty) == Some(false)
        && api.class_from_type(ty) == Some(code)
}

fn checked(api: Il2cpp, class: Class, name: &CStr, static_: bool, args: &[(i32, bool)], returns: i32) -> Result<Method, &'static str> {
    let method = api.unique_method(class, name, args.len() as u32).ok_or("language dependency unavailable or ambiguous")?;
    if !api.ordinary_method(method) || !api.method_signature(method, static_, args, returns) {
        return Err("language dependency signature mismatch");
    }
    Ok(method)
}

impl Engine {
    fn discover(api: Il2cpp) -> Result<(Self, Method), &'static str> {
        let image = api.exact_image("TeamCherry.Localization.dll").ok_or("localization image unavailable")?;
        let language = api.class_from_name(image, c"TeamCherry.Localization", c"Language").ok_or("Language unavailable")?;
        let code = api.class_from_name(image, c"TeamCherry.Localization", c"LanguageCode").ok_or("LanguageCode unavailable")?;
        if api.enum_base_type(code).is_none_or(|ty| api.type_kind(ty) != Some(8)) {
            return Err("LanguageCode is not an Int32 enum");
        }
        let switch = api.matching_method(language, c"SwitchLanguage", true, &[(0x11, false)], TYPE_BOOLEAN)
            .filter(|m| api.ordinary_method(*m)
                && api.method_param(*m, 0).is_some_and(|ty| language_code(api, ty, code)))
            .ok_or("enum language switch signature unavailable")?;
        let current = api.unique_method(language, c"CurrentLanguage", 0)
            .filter(|m| api.ordinary_method(*m) && api.method_shape(*m, 0, true)
                && api.method_return_type(*m).is_some_and(|ty| language_code(api, ty, code)))
            .ok_or("current language signature unavailable")?;
        let enumeration = api.find_class(c"System", c"Enum").ok_or("System.Enum unavailable")?;
        if api.parent(code) != Some(enumeration) { return Err("LanguageCode parent mismatch"); }
        let to_string = checked(api, enumeration, c"ToString", false, &[], TYPE_STRING)?;
        let prefs = api.find_class(c"UnityEngine", c"PlayerPrefs").ok_or("PlayerPrefs unavailable")?;
        let set_string = checked(api, prefs, c"SetString", true, &[(TYPE_STRING, false), (TYPE_STRING, false)], TYPE_VOID)?;
        let save = checked(api, prefs, c"Save", true, &[], TYPE_VOID)?;
        let application = api.find_class(c"UnityEngine", c"Application").ok_or("Application unavailable")?;
        let roots = api.verified_gc_handles(application)?;
        for name in ["il2cpp_runtime_invoke", "il2cpp_object_get_class", "il2cpp_string_new",
            "il2cpp_string_length", "il2cpp_string_chars"] {
            if !api.has(name) { return Err("language runtime export unavailable"); }
        }
        Ok((Self { api, roots, code, current, to_string, set_string, save }, switch))
    }

    /// # Safety
    /// Owning attached Unity callback; metadata/code and verified root exports stay pinned.
    unsafe fn persist(self) -> Result<String, &'static str> {
        // SAFETY: discovery verified static getter, enum identity/base and every
        // signature. Each newly returned object is rooted before another managed
        // call; all roots drop in this callback, including errors and unwinds.
        unsafe {
            let boxed = self.api.invoke(self.current, std::ptr::null_mut(), std::ptr::null_mut())
                .ok().filter(|p| !p.is_null()).ok_or("current language query failed")?;
            let boxed = self.roots.root(boxed).ok_or("language root failed")?;
            if self.api.object_class(boxed.target()) != Some(self.code) { return Err("current language object mismatch"); }
            let text = self.api.invoke(self.to_string, boxed.target(), std::ptr::null_mut())
                .ok().filter(|p| !p.is_null()).ok_or("language name query failed")?;
            let text = self.roots.root(text).ok_or("language name root failed")?;
            let name = self.api.string_utf16(text.target(), 8)
                .and_then(|units| String::from_utf16(&units).ok()).ok_or("language name invalid")?;
            if name.len() != 2 || !name.bytes().all(|byte| byte.is_ascii_uppercase()) {
                return Err("unsupported language name");
            }
            if STOPPED.load(Ordering::Acquire) { return Err("player stopping"); }
            let key = self.api.new_string(c"M2H_lastLanguage").ok_or("language key allocation failed")?;
            let key = self.roots.root(key).ok_or("language key root failed")?;
            let mut args = [key.target(), text.target()];
            self.api.invoke(self.set_string, std::ptr::null_mut(), args.as_mut_ptr()).map_err(|_| "language write failed")?;
            if STOPPED.load(Ordering::Acquire) { return Err("player stopping"); }
            self.api.invoke(self.save, std::ptr::null_mut(), std::ptr::null_mut()).map_err(|_| "language save failed")?;
            Ok(name)
        }
    }
}

unsafe extern "C-unwind" fn switch(value: i32, method: *mut c_void) -> bool {
    let Some(original) = ORIGINAL.get() else { return false; };
    // SAFETY: matching checked static bool SwitchLanguage(Int32 enum) ABI.
    let changed = unsafe { original(value, method) };
    if !changed || STOPPED.load(Ordering::Acquire) { return changed; }
    let Some(engine) = *super::lock(&ENGINE) else { return changed; };
    if !engine.api.thread_attached() || BUSY.with(|value| value.replace(true)) { return changed; }
    let _guard = Guard;
    // SAFETY: successful original callback on its attached Unity thread;
    // no lock spans a managed call and no object survives this scope.
    match unsafe { engine.persist() } {
        Ok(name) => super::host_log_raw(&format!("selected language saved: {name}")),
        Err(reason) => super::host_log_raw(&format!("language persistence stopped: {reason}")),
    }
    changed
}

pub(super) fn stop() { STOPPED.store(true, Ordering::Release); }

/// # Safety
/// Same process-pinned code and quiescent install contract as the parent title patch.
pub(super) unsafe fn install(api: Il2cpp) -> Vec<InstallEvent> {
    stop(); *super::lock(&ENGINE) = None; ORIGINAL.clear();
    let (engine, method) = match Engine::discover(api) {
        Ok(value) => value,
        Err(reason) => return vec![InstallEvent::TargetMissing { behavior: BEHAVIOR, target: reason.into() }],
    };
    let Some(entry) = api.method_entry(method) else {
        return vec![InstallEvent::TargetMissing { behavior: BEHAVIOR, target: "language switch entry unavailable".into() }];
    };
    // SAFETY: checked enum width, return/staticness and non-generic method;
    // caller keeps entry idle until trampoline and metadata publication finish.
    match unsafe { install_detour(BEHAVIOR, "Language.SwitchLanguage(LanguageCode)", entry, &ORIGINAL, switch) } {
        Ok(installed) => {
            *super::lock(&ENGINE) = Some(engine); STOPPED.store(false, Ordering::Release);
            vec![InstallEvent::Installed { behavior: BEHAVIOR, target: "Language.SwitchLanguage(LanguageCode)".into(), shape: installed.shape }]
        }
        Err(event) => vec![event],
    }
}
