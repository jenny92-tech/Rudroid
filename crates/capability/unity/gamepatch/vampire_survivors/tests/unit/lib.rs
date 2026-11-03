//! INPUT: owned test strings and typed mock IL2CPP string/lookup functions bound through `Il2cpp::bind`
//! OUTPUT: original-first/name-gated fallback and paired GC handles, including unwind
//! POS: title behavior regression; the real game test separately covers actual metadata/detour
use super::*;
use rudroid_common::il2cpp_abi::GcHandleAbi;
use rudroid_common::registry::HostStr;
use std::cell::RefCell;
use std::ffi::c_char;
struct TestString(Vec<u16>);
#[derive(Default)]
struct State {
    existing: bool,
    no_fallback: bool,
    no_string: bool,
    no_root: bool,
    unwind: bool,
    calls: usize,
    roots: usize,
    releases: usize,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
static FALLBACK: OnceLock<TestString> = OnceLock::new();
// Unity 6 uses pointer-width GC handles, unlike older IL2CPP releases.
const HANDLE: usize = usize::MAX - 42;
fn fallback() -> *mut c_void {
    FALLBACK.get_or_init(|| TestString("Sprites/Default".encode_utf16().collect()))
        as *const TestString as *mut c_void
}
unsafe extern "C" fn length(string: *mut c_void) -> i32 {
    // SAFETY: lookup tests pass a live, owned TestString for the whole call.
    let string = unsafe { &*string.cast::<TestString>() };
    string.0.len() as i32
}
unsafe extern "C" fn chars(string: *mut c_void) -> *const u16 {
    // SAFETY: lookup tests keep the TestString alive until its chars are consumed.
    let string = unsafe { &*string.cast::<TestString>() };
    string.0.as_ptr()
}
unsafe extern "C-unwind" fn new(name: *const c_char) -> *mut c_void {
    // SAFETY: the mock string_new is called with the live fallback CStr literal.
    assert_eq!(unsafe { CStr::from_ptr(name) }, c"Sprites/Default");
    if STATE.with(|s| s.borrow().no_string) {
        std::ptr::null_mut()
    } else {
        fallback()
    }
}
unsafe extern "C-unwind" fn root(string: *mut c_void, pinned: bool) -> usize {
    assert_eq!(string, fallback());
    assert!(!pinned);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.no_root {
            0
        } else {
            s.roots += 1;
            HANDLE
        }
    })
}
unsafe extern "C" fn target(id: usize) -> *mut c_void {
    assert_eq!(id, HANDLE);
    fallback()
}
unsafe extern "C" fn free(id: usize) {
    assert_eq!(id, HANDLE);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.roots -= 1;
        s.releases += 1;
    });
}
unsafe extern "C-unwind" fn find(name: *mut c_void, _method: *mut c_void) -> *mut c_void {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.calls += 1;
        if s.existing {
            return 0x10usize as *mut c_void;
        }
        if !name.is_null() && name == fallback() {
            if s.unwind {
                panic!("fixture native lookup unwinds");
            }
            if !s.no_fallback {
                return 0x20usize as *mut c_void;
            }
        }
        std::ptr::null_mut()
    })
}
extern "C" fn so_symbol(_module: *mut c_void, name: HostStr<'_>) -> usize {
    match name.to_cstr().unwrap_or_default().to_bytes() {
        b"il2cpp_string_length" => length as usize,
        b"il2cpp_string_chars" => chars as usize,
        b"il2cpp_string_new" => new as usize,
        b"il2cpp_gchandle_new" => root as usize,
        b"il2cpp_gchandle_get_target" => target as usize,
        b"il2cpp_gchandle_free" => free as usize,
        _ => 0,
    }
}
fn invoke(name: Option<&str>, state: State) -> *mut c_void {
    STATE.with(|s| *s.borrow_mut() = state);
    let string = name.map(|s| TestString(s.encode_utf16().collect()));
    let ptr = string.as_ref().map_or(std::ptr::null_mut(), |s| {
        s as *const TestString as *mut c_void
    });
    let api = unsafe { Il2cpp::bind(0x1000, so_symbol, &[]) }.unwrap();
    let handles = unsafe { api.gc_handles(GcHandleAbi::PointerWidth) }.unwrap();
    unsafe { lookup(api, handles, find, ptr, std::ptr::null_mut()) }
}
#[test]
fn an_existing_shader_is_always_preserved() {
    assert_eq!(
        invoke(
            Some(MISSING),
            State {
                existing: true,
                ..Default::default()
            }
        ) as usize,
        0x10
    );
    STATE.with(|s| {
        let s = s.borrow();
        assert_eq!(s.calls, 1);
        assert_eq!(s.releases, 0);
    });
}
#[test]
fn only_the_exact_missing_debug_name_can_fall_back() {
    for name in [
        None,
        Some("Hidden/Internal-ColoredX"),
        Some("Hidden/Other"),
        Some("hidden/Internal-Colored"),
    ] {
        assert!(invoke(name, State::default()).is_null());
        STATE.with(|s| {
            let s = s.borrow();
            assert_eq!(s.calls, 1);
            assert_eq!(s.roots, 0);
            assert_eq!(s.releases, 0);
        });
    }
    assert_eq!(invoke(Some(MISSING), State::default()) as usize, 0x20);
    STATE.with(|s| {
        let s = s.borrow();
        assert_eq!(s.calls, 2);
        assert_eq!(s.roots, 0);
        assert_eq!(s.releases, 1);
    });
}
#[test]
fn allocation_and_fallback_failures_keep_the_original_failure() {
    for state in [
        State {
            no_string: true,
            ..Default::default()
        },
        State {
            no_root: true,
            ..Default::default()
        },
        State {
            no_fallback: true,
            ..Default::default()
        },
    ] {
        assert!(invoke(Some(MISSING), state).is_null());
        STATE.with(|s| assert_eq!(s.borrow().roots, 0));
    }
}
#[test]
fn temporary_gc_handle_is_released_when_lookup_unwinds() {
    let error = std::panic::catch_unwind(|| {
        invoke(
            Some(MISSING),
            State {
                unwind: true,
                ..Default::default()
            },
        )
    });
    assert!(error.is_err());
    STATE.with(|s| {
        let s = s.borrow();
        assert_eq!(s.roots, 0);
        assert_eq!(s.releases, 1);
    });
}
#[test]
fn the_patch_requires_both_the_title_and_explicit_configuration() {
    assert!(applies(c"com.poncle.vampiresurvivors", true));
    assert!(!applies(c"com.poncle.vampiresurvivors", false));
    assert!(!applies(c"com.other.game", true));
    assert!(!applies(c"", true));
}
