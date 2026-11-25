//! INPUT: two successive backend activations with a fixed RGBA8 window
//! OUTPUT: cached handles cannot access a replacement window and release safely
//! POS: device-independent regression for the shared NDK window ABI
use super::*;
use std::sync::Mutex;

struct Window;
static ACTIVE: Mutex<Option<Arc<Window>>> = Mutex::new(None);

impl Backend for Window {
    const FORMAT: Option<i32> = Some(1);
    const NULL_WINDOW: i32 = -19;
    const INVALID_GEOMETRY: i32 = -95;
    fn active() -> Option<Arc<Self>> {
        ACTIVE.lock().unwrap().clone()
    }
    fn size(&self) -> (i32, i32) {
        (640, 480)
    }
}

#[test]
fn rgba_window_leases_survive_replacement_without_aliasing_it() {
    *ACTIVE.lock().unwrap() = Some(Arc::new(Window));
    let weak = Arc::downgrade(&Window::active().unwrap());
    let handle = from_surface::<Window>(ptr::null_mut(), ptr::dangling_mut());
    assert_eq!(format::<Window>(handle), 1);
    assert_eq!(geometry::<Window>(handle, 640, 480, 1), 0);
    assert_eq!(geometry::<Window>(handle, 0, 480, 1), -95);
    assert_eq!(width::<Window>(ptr::null_mut()), -19);
    assert_eq!(
        resolve::<Window>(b"ANativeWindow_getFormat"),
        format::<Window> as usize
    );
    // SAFETY: handle owns one reference; acquire adds a second.
    unsafe { acquire::<Window>(handle) };
    *ACTIVE.lock().unwrap() = Some(Arc::new(Window));
    assert_eq!(width::<Window>(handle), -19);
    assert_eq!(weak.strong_count(), 2);
    // SAFETY: both references still belong to the first allocation.
    unsafe {
        release::<Window>(handle);
        release::<Window>(handle);
    }
    assert!(weak.upgrade().is_none());
    *ACTIVE.lock().unwrap() = None;
}

#[cfg(feature = "vulkan")]
#[test]
fn backend_symbol_sets_preserve_format_support() {
    let name = b"ANativeWindow_getFormat";
    assert_eq!(resolve::<crate::guest_graphics::Graphics>(name), 0);
    assert_ne!(resolve::<crate::vulkan::Graphics>(name), 0);
    let window = ptr::null_mut();
    assert_eq!(width::<crate::guest_graphics::Graphics>(window), -22);
    assert_eq!(width::<crate::vulkan::Graphics>(window), -19);
}
