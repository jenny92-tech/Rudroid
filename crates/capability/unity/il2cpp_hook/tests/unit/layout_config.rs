//! INPUT:  real HostConfig getters through a private bounded Host API
//! OUTPUT:  legacy HUD keys, defaults, disabling and invalid numeric/partial movement checks
//! POS:    exercises the production parser rather than a duplicate TOML dialect
use crate::layout_config::LayoutRequest;
use rudroid_common::host_api::HostApiV0;
use rudroid_common::{host_api::ConfigGetStringFn, registry::HostStr};
use rudroid_host_config::HostConfig;
use std::{cell::RefCell, ffi::c_char};
thread_local! {static CONFIG:RefCell<Option<HostConfig>>=const{RefCell::new(None)};}
extern "C" fn boolean(k: HostStr<'_>, fallback: i32) -> i32 {
    CONFIG.with(|c| {
        i32::from(
            c.borrow()
                .as_ref()
                .unwrap()
                .get_bool(k.to_cstr().unwrap().to_str().unwrap(), fallback != 0),
        )
    })
}
extern "C" fn integer(k: HostStr<'_>, fallback: i64) -> i64 {
    CONFIG.with(|c| {
        c.borrow()
            .as_ref()
            .unwrap()
            .get_i64(k.to_cstr().unwrap().to_str().unwrap(), fallback)
    })
}
extern "C" fn count(k: HostStr<'_>) -> u32 {
    CONFIG.with(|c| {
        c.borrow()
            .as_ref()
            .unwrap()
            .array_size(k.to_cstr().unwrap().to_str().unwrap()) as u32
    })
}
unsafe extern "C" fn text(
    k: HostStr<'_>,
    fallback: HostStr<'_>,
    out: *mut c_char,
    size: u32,
) -> i32 {
    let value = CONFIG.with(|c| {
        c.borrow().as_ref().unwrap().get_string(
            k.to_cstr().unwrap().to_str().unwrap(),
            fallback.to_cstr().unwrap().to_str().unwrap(),
        )
    });
    let Some(value) = value else {
        return 0;
    };
    if size == 0 {
        return 0;
    }
    let n = value.len().min(size as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), out.cast(), n);
        *out.add(n) = 0;
    }
    1
}
fn api(config: &str) -> HostApiV0 {
    CONFIG.with(|c| c.replace(Some(HostConfig::parse(config).unwrap())));
    let mut h = HostApiV0::stubbed();
    h.config_get_bool = boolean;
    h.config_get_i64 = integer;
    h.config_array_size = count;
    h.config_get_string = unsafe { ConfigGetStringFn::new(text) };
    h
}
#[test]
fn legacy_hud_keys_are_read_through_the_real_host_contract() {
    let host=api("[ui_layout]\nclass='HUD'\nhide=[16,24]\nmove_from=32\nmove_to=40\nmove_dx=5\nmove_dy=-7\nright_margin=20\n");
    let request = LayoutRequest::read(&host).unwrap().unwrap();
    let spec = request.spec();
    assert_eq!(spec.class, c"HUD");
    assert_eq!(spec.method, c"Start");
    assert_eq!(spec.hide, [16, 24]);
    assert_eq!(spec.move_from, Some(32));
    assert_eq!(spec.move_to, Some(40));
    assert_eq!((spec.dx, spec.dy, spec.right_margin), (5, -7, Some(20)));
}
#[test]
fn absent_disabled_partial_or_overflowing_hud_config_is_handled_explicitly() {
    assert!(LayoutRequest::read(&api("")).unwrap().is_none());
    assert!(
        LayoutRequest::read(&api("[ui_layout]\nenabled=false\nclass='HUD'\nhide=[16]\n"))
            .unwrap()
            .is_none()
    );
    assert!(LayoutRequest::read(&api("[ui_layout]\nclass='HUD'\nmove_from=16\n")).is_err());
    assert!(LayoutRequest::read(&api(
        "[ui_layout]\nclass='HUD'\nhide=[16]\nmove_dx=9223372036854775807\n"
    ))
    .is_err());
}
