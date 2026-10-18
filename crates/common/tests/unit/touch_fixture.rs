//! INPUT:  private metadata oracle and real TouchManager bridge
//! OUTPUT:  valid-original/null-array/null-element/bool-state and root-release assertions
//! POS:    exercises collection fallback using checked managed methods rather than array offsets
use super::hud_fixture::{add_class, add_method, add_type};
use super::*;
#[repr(C)]
struct Manager {
    class: usize,
    controls: usize,
    enabled: bool,
    canary: u32,
}
#[repr(C)]
struct Control {
    class: usize,
    enabled: bool,
}
#[repr(C)]
struct Array {
    class: usize,
    count: i32,
    elements: [usize; 3],
}
#[repr(C)]
struct BoxedI32 {
    class: usize,
    value: i32,
}
thread_local! {static LENGTH:Cell<usize>=const{Cell::new(0)};}
extern "C-unwind" fn touch_invoke(m: Ptr, o: Ptr, args: *mut Ptr, e: &mut Ptr) -> Ptr {
    *e = std::ptr::null_mut();
    match method(m).name.to_bytes() {
        b"get_Length" => {
            let value = LENGTH.with(|v| v.get());
            unsafe {
                (*(value as *mut BoxedI32)).value = (*(o as *const Array)).count;
            }
            value as Ptr
        }
        b"GetValue" => unsafe {
            let i = *(*args).cast::<i32>() as usize;
            (*(o as *const Array)).elements[i] as Ptr
        },
        b"set_enabled" => {
            unsafe {
                (*(o as *mut Control)).enabled = *(*args).cast::<bool>();
            }
            std::ptr::null_mut()
        }
        name => panic!("unexpected touch oracle method {name:?}"),
    }
}
unsafe extern "C-unwind" fn stock(o: *mut Manager, value: bool, _: Ptr) {
    CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        assert!(!(*o).controls.eq(&0));
        let array = &*((*o).controls as *const Array);
        for i in 0..array.count as usize {
            assert_ne!(array.elements[i], 0);
            (*(array.elements[i] as *mut Control)).enabled = value;
        }
        (*o).enabled = value;
    }
}
fn model() -> (Fixture, Box<BoxedI32>, usize, usize) {
    let mut f = Fixture::new();
    let manager = add_class(&mut f, "TouchManager", 0);
    let array = add_class(&mut f, "Array", 0);
    let int = add_class(&mut f, "Int32", 0);
    let behaviour = add_class(&mut f, "Behaviour", 0);
    let classes = f.classes.clone();
    let boolean = add_type(&mut f, 2, 0);
    let array_type = add_type(&mut f, 0x1d, classes[array]);
    let object = add_type(&mut f, 0x1c, classes[behaviour]);
    let void = f.ty(1);
    let integer = f.ty(8);
    add_method(
        &mut f,
        manager,
        "set_controlsEnabled",
        false,
        void,
        &[boolean],
    );
    f.methods.last_mut().unwrap().entry = stock as usize;
    add_method(&mut f, array, "get_Length", false, integer, &[]);
    add_method(&mut f, array, "GetValue", false, object, &[integer]);
    add_method(&mut f, behaviour, "set_enabled", false, void, &[boolean]);
    for (offset, kind) in [
        (std::mem::offset_of!(Manager, controls), array_type),
        (std::mem::offset_of!(Manager, enabled), boolean),
    ] {
        f.fields.push(Box::new(F {
            offset,
            kind,
            flags: 0,
            parent: classes[manager],
        }));
        let field = &**f.fields.last().unwrap() as *const F as usize;
        f.owners[manager].fields.push(field);
    }
    let mut boxed = Box::new(BoxedI32 {
        class: classes[int],
        value: 0,
    });
    LENGTH.with(|v| v.set(&mut *boxed as *mut BoxedI32 as usize));
    EXTRA_INVOKE.with(|v| v.set(Some(touch_invoke)));
    (f, boxed, manager, behaviour)
}
extern "C-unwind" fn named_field(c: Ptr, n: *const c_char) -> Ptr {
    let name = unsafe { CStr::from_ptr(n) };
    let index = if name == c"touchControls" {
        0
    } else if name == c"_controlsEnabled" {
        1
    } else {
        return std::ptr::null_mut();
    };
    class(c).fields.get(index).copied().unwrap_or(0) as Ptr
}
extern "C" fn touch_resolve(p: Ptr, n: HostStr<'_>) -> usize {
    if n.to_cstr() == Some(c"il2cpp_class_get_field_from_name") {
        named_field as usize
    } else {
        resolve(p, n)
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn touch_null_collection_and_elements_are_safe_and_valid_stock_setter_is_preserved() {
    let _lock = SERIAL.lock().unwrap();
    let (f, _boxed, manager, behaviour) = model();
    CURRENT.with(|p| p.set(&f as *const Fixture as usize));
    let api = unsafe { Il2cpp::bind(0x42, touch_resolve, &[]) }.unwrap();
    let plan = ManagedHookPlan::touch_controls(api).unwrap();
    ACTIVE.store(true, Ordering::Release);
    let mut o = Manager {
        class: f.classes[manager],
        controls: 0,
        enabled: false,
        canary: 0x7654,
    };
    let mut frame = Frame {
        x: [0; 10],
        q: [0; 128],
        result_x: [0; 2],
        result_q: [0; 32],
    };
    frame.x[0] = &mut o as *mut _ as u64;
    frame.x[1] = 1;
    frame.x[2] = plan.method.as_ptr() as u64;
    let before = CALLS.load(Ordering::SeqCst);
    plan.run(plan.entry, &host(), &mut frame);
    assert!(o.enabled);
    assert_eq!(CALLS.load(Ordering::SeqCst), before);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    let mut a = Control {
        class: f.classes[behaviour],
        enabled: true,
    };
    let mut b = Control {
        class: f.classes[behaviour],
        enabled: true,
    };
    let array_class = f
        .owners
        .iter()
        .position(|c| c.name.as_c_str() == c"Array")
        .unwrap();
    let mut array = Array {
        class: f.classes[array_class],
        count: 3,
        elements: [&mut a as *mut _ as usize, 0, &mut b as *mut _ as usize],
    };
    o.controls = &mut array as *mut _ as usize;
    frame.x[1] = 0;
    plan.run(plan.entry, &host(), &mut frame);
    assert!(!o.enabled);
    assert!(!a.enabled);
    assert!(!b.enabled);
    assert_eq!(CALLS.load(Ordering::SeqCst), before);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    array.count = 2;
    array.elements[1] = &mut b as *mut _ as usize;
    frame.x[1] = 1;
    plan.run(plan.entry, &host(), &mut frame);
    assert!(o.enabled);
    assert!(a.enabled);
    assert!(b.enabled);
    assert_eq!(CALLS.load(Ordering::SeqCst), before + 1);
    stop();
    frame.x[1] = 0;
    plan.run(plan.entry, &host(), &mut frame);
    assert!(o.enabled);
    assert_eq!(CALLS.load(Ordering::SeqCst), before + 1);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    assert_eq!(o.canary, 0x7654);
    ACTIVE.store(true, Ordering::Release);
}
