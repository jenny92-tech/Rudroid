//! INPUT:  private IL2CPP oracle and real common HUD/native gateway
//! OUTPUT:  object visibility, vector placement, null/error/stop and ownership assertions
//! POS:    models Unity managed effects without raw production object offsets
use super::*;
use crate::hud_layout::HudSpec;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct V2 {
    x: f32,
    y: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}
#[repr(C)]
struct GameObject {
    class: usize,
    active: bool,
}
#[repr(C)]
struct Transform {
    class: usize,
    pivot: V2,
    position: V3,
    scale: V3,
}
#[repr(C)]
struct Component {
    class: usize,
    game: usize,
    transform: usize,
}
#[repr(C)]
struct Hud {
    class: usize,
    source: usize,
    destination: usize,
    hidden: usize,
    canary: u64,
}
#[repr(C)]
struct Boxed<T> {
    class: usize,
    value: T,
}
struct State {
    pivot: Boxed<V2>,
    position: Boxed<V3>,
    width: Boxed<i32>,
    fail_position: bool,
    stop_after_hide: bool,
    original_calls: usize,
}
thread_local! {static STATE:Cell<usize>=const{Cell::new(0)};}
fn state() -> &'static mut State {
    STATE.with(|s| unsafe { &mut *(s.get() as *mut State) })
}
extern "C-unwind" fn ui_invoke(m: Ptr, o: Ptr, args: *mut Ptr, e: &mut Ptr) -> Ptr {
    *e = std::ptr::null_mut();
    match method(m).name.to_bytes() {
        b"get_gameObject" => unsafe { (*(o as *const Component)).game as Ptr },
        b"get_transform" => unsafe { (*(o as *const Component)).transform as Ptr },
        b"SetActive" => {
            unsafe {
                (*(o as *mut GameObject)).active = *(*args).cast::<bool>();
            }
            if state().stop_after_hide {
                stop();
            }
            std::ptr::null_mut()
        }
        b"get_pivot" => {
            state().pivot.value = unsafe { (*(o as *const Transform)).pivot };
            &mut state().pivot as *mut _ as Ptr
        }
        b"set_pivot" => {
            unsafe {
                (*(o as *mut Transform)).pivot = *(*args).cast::<V2>();
            }
            std::ptr::null_mut()
        }
        b"get_position" => {
            state().position.value = unsafe { (*(o as *const Transform)).position };
            &mut state().position as *mut _ as Ptr
        }
        b"set_position" => {
            if state().fail_position {
                *e = 1 as Ptr;
            } else {
                unsafe {
                    (*(o as *mut Transform)).position = *(*args).cast::<V3>();
                }
            }
            std::ptr::null_mut()
        }
        b"set_localScale" => {
            unsafe {
                (*(o as *mut Transform)).scale = *(*args).cast::<V3>();
            }
            std::ptr::null_mut()
        }
        b"get_width" => &mut state().width as *mut _ as Ptr,
        name => panic!("unexpected HUD oracle method {name:?}"),
    }
}
unsafe extern "C-unwind" fn hud_original(_: Ptr, _: i32, _: f64, _: Ptr) -> f64 {
    state().original_calls += 1;
    67.25
}
pub(super) fn add_class(f: &mut Fixture, name: &str, parent: usize) -> usize {
    f.owners.push(Box::new(C {
        name: CString::new(name).unwrap(),
        methods: Vec::new(),
        fields: Vec::new(),
        parent,
    }));
    let p = &**f.owners.last().unwrap() as *const C as usize;
    f.classes.push(p);
    f.classes.len() - 1
}
pub(super) fn add_type(f: &mut Fixture, kind: i32, class: usize) -> usize {
    f.types.push(Box::new(Ty {
        kind,
        byref: false,
        class,
    }));
    &**f.types.last().unwrap() as *const Ty as usize
}
pub(super) fn add_method(
    f: &mut Fixture,
    owner: usize,
    name: &str,
    static_: bool,
    ret: usize,
    params: &[usize],
) {
    f.methods.push(Box::new(M {
        entry: 0,
        name: CString::new(name).unwrap(),
        flags: if static_ { 0x10 } else { 0 },
        params: params.to_vec(),
        returns: ret,
        generic: false,
    }));
    let m = &**f.methods.last().unwrap() as *const M as usize;
    f.owners[owner].methods.push(m);
}
fn model() -> (Fixture, Box<State>) {
    let mut f = setup();
    f.methods.last_mut().unwrap().entry = hud_original as usize;
    f.owners[0].fields.clear();
    let component = add_class(&mut f, "Component", 0);
    let game = add_class(&mut f, "GameObject", 0);
    let cp = f.classes[component];
    let transform = add_class(&mut f, "Transform", cp);
    let tp = f.classes[transform];
    let rect = add_class(&mut f, "RectTransform", tp);
    let v2 = add_class(&mut f, "Vector2", 0);
    let v3 = add_class(&mut f, "Vector3", 0);
    let int = add_class(&mut f, "Int32", 0);
    let screen = add_class(&mut f, "Screen", 0);
    let classes = f.classes.clone();
    let ctype = add_type(&mut f, 0x12, classes[component]);
    let gtype = add_type(&mut f, 0x12, classes[game]);
    let ttype = add_type(&mut f, 0x12, classes[transform]);
    let v2type = add_type(&mut f, 0x11, classes[v2]);
    let v3type = add_type(&mut f, 0x11, classes[v3]);
    let boolean = add_type(&mut f, 2, 0);
    let void = f.ty(1);
    let integer = f.ty(8);
    add_method(&mut f, component, "get_gameObject", false, gtype, &[]);
    add_method(&mut f, component, "get_transform", false, ttype, &[]);
    add_method(&mut f, game, "SetActive", false, void, &[boolean]);
    add_method(&mut f, transform, "set_localScale", false, void, &[v3type]);
    add_method(&mut f, rect, "get_pivot", false, v2type, &[]);
    add_method(&mut f, rect, "set_pivot", false, void, &[v2type]);
    add_method(&mut f, transform, "get_position", false, v3type, &[]);
    add_method(&mut f, transform, "set_position", false, void, &[v3type]);
    add_method(&mut f, screen, "get_width", true, integer, &[]);
    for offset in [
        std::mem::offset_of!(Hud, source),
        std::mem::offset_of!(Hud, destination),
        std::mem::offset_of!(Hud, hidden),
    ] {
        f.fields.push(Box::new(F {
            offset,
            kind: ctype,
            flags: 0,
            parent: f.classes[0],
        }));
        let p = &**f.fields.last().unwrap() as *const F as usize;
        f.owners[0].fields.push(p);
    }
    let mut s = Box::new(State {
        pivot: Boxed {
            class: f.classes[v2],
            value: V2::default(),
        },
        position: Boxed {
            class: f.classes[v3],
            value: V3::default(),
        },
        width: Boxed {
            class: f.classes[int],
            value: 640,
        },
        fail_position: false,
        stop_after_hide: false,
        original_calls: 0,
    });
    STATE.with(|p| p.set(&mut *s as *mut State as usize));
    EXTRA_INVOKE.with(|p| p.set(Some(ui_invoke)));
    (f, s)
}
fn spec<'a>(hide: &'a [usize], margin: Option<i32>) -> HudSpec<'a> {
    HudSpec {
        assembly: c"Assembly-CSharp",
        namespace: c"",
        class: c"Example",
        method: c"Run",
        argc: 2,
        hide,
        move_from: Some(std::mem::offset_of!(Hud, source)),
        move_to: Some(std::mem::offset_of!(Hud, destination)),
        dx: 5,
        dy: -7,
        right_margin: margin,
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn hud_applies_after_original_preserves_vectors_and_cleans_all_roots() {
    let _lock = SERIAL.lock().unwrap();
    let (f, mut s) = model();
    ACTIVE.store(true, Ordering::Release);
    let hide = [std::mem::offset_of!(Hud, hidden)];
    let plan = ManagedHookPlan::hud(f.api(), &spec(&hide, Some(20))).unwrap();
    let rect = f
        .owners
        .iter()
        .position(|c| c.name.as_c_str() == c"RectTransform")
        .unwrap();
    let game = f
        .owners
        .iter()
        .position(|c| c.name.as_c_str() == c"GameObject")
        .unwrap();
    let component = f
        .owners
        .iter()
        .position(|c| c.name.as_c_str() == c"Component")
        .unwrap();
    let mut source = Transform {
        class: f.classes[rect],
        pivot: V2 { x: 0.2, y: 0.7 },
        position: V3 {
            x: 100.0,
            y: 50.0,
            z: 9.0,
        },
        scale: V3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
    };
    let mut dest = Transform {
        class: f.classes[rect],
        pivot: V2::default(),
        position: V3::default(),
        scale: V3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
    };
    let mut object = GameObject {
        class: f.classes[game],
        active: true,
    };
    let source_c = Component {
        class: f.classes[component],
        game: &mut object as *mut _ as usize,
        transform: &mut source as *mut _ as usize,
    };
    let dest_c = Component {
        class: f.classes[component],
        game: &mut object as *mut _ as usize,
        transform: &mut dest as *mut _ as usize,
    };
    let mut hud = Hud {
        class: f.classes[0],
        source: &source_c as *const _ as usize,
        destination: &dest_c as *const _ as usize,
        hidden: &dest_c as *const _ as usize,
        canary: 0xabcd,
    };
    let mut frame = Frame {
        x: [0; 10],
        q: [0; 128],
        result_x: [0; 2],
        result_q: [0; 32],
    };
    frame.x[0] = &mut hud as *mut _ as u64;
    frame.x[2] = plan.method.as_ptr() as u64;
    plan.run(plan.entry, &host(), &mut frame);
    assert_eq!(s.original_calls, 1);
    assert_eq!(
        f64::from_le_bytes(frame.result_q[..8].try_into().unwrap()),
        67.25
    );
    assert!(!object.active);
    assert_eq!(dest.pivot, source.pivot);
    assert_eq!(
        dest.position,
        V3 {
            x: 620.0,
            y: 43.0,
            z: 9.0
        }
    );
    assert_eq!(dest.scale, V3::default());
    assert_eq!(hud.canary, 0xabcd);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    // A managed failure must retain the original return and release every temporary owner.
    s.fail_position = true;
    object.active = true;
    plan.run(plan.entry, &host(), &mut frame);
    assert!(object.active);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    s.fail_position = false;
    s.stop_after_hide = true;
    object.active = true;
    dest.scale = V3 {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    plan.run(plan.entry, &host(), &mut frame);
    assert!(!object.active);
    assert_eq!(
        dest.scale,
        V3 {
            x: 1.0,
            y: 1.0,
            z: 1.0
        }
    );
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    ACTIVE.store(true, Ordering::Release);
}

#[test]
fn hud_refuses_incompatible_fields_and_vector_types() {
    let _lock = SERIAL.lock().unwrap();
    let (mut f, _s) = model();
    let hide = [123456];
    assert!(ManagedHookPlan::hud(f.api(), &spec(&hide, None)).is_err());
    let hide = [std::mem::offset_of!(Hud, hidden)];
    f.fields.last_mut().unwrap().flags = 0x10;
    assert!(ManagedHookPlan::hud(f.api(), &spec(&hide, None)).is_err());
    f.fields.last_mut().unwrap().flags = 0;
    let index = f
        .methods
        .iter()
        .position(|m| m.name.as_c_str() == c"set_pivot")
        .unwrap();
    f.methods[index].params[0] = f.ty(8);
    assert!(ManagedHookPlan::hud(f.api(), &spec(&hide, None)).is_err());
}
