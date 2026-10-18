//! INPUT:  real ARM64 gateway and a private standard-export IL2CPP oracle
//! OUTPUT:  actual patch execution, ABI preservation, rejection and local-root lifetime checks
//! POS:    validates native effects rather than configuration counts
use super::*;
use crate::registry::HostStr;
use std::{
    cell::Cell,
    ffi::{c_char, CString},
    sync::atomic::{AtomicI32, AtomicUsize},
};
type Ptr = *mut c_void;
thread_local! {static CURRENT:Cell<usize>=const{Cell::new(0)};}
thread_local! {static EXTRA_INVOKE:Cell<Option<extern "C-unwind" fn(Ptr,Ptr,*mut Ptr,&mut Ptr)->Ptr>>=const{Cell::new(None)};}
static ROOTS: AtomicI32 = AtomicI32::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static SERIAL: Mutex<()> = Mutex::new(());

#[repr(C)]
struct Object {
    class: usize,
    value: f32,
    source: f32,
    count: i32,
    canary: u32,
}
struct Ty {
    kind: i32,
    byref: bool,
    class: usize,
}
#[repr(C)]
struct M {
    entry: usize,
    name: CString,
    flags: u32,
    params: Vec<usize>,
    returns: usize,
    generic: bool,
}
struct F {
    offset: usize,
    kind: usize,
    flags: i32,
    parent: usize,
}
struct C {
    name: CString,
    methods: Vec<usize>,
    fields: Vec<usize>,
    parent: usize,
}
struct Fixture {
    classes: Vec<usize>,
    types: Vec<Box<Ty>>,
    methods: Vec<Box<M>>,
    fields: Vec<Box<F>>,
    owners: Vec<Box<C>>,
}
impl Fixture {
    fn new() -> Self {
        let mut fixture = Self {
            classes: Vec::new(),
            types: Vec::new(),
            methods: Vec::new(),
            fields: Vec::new(),
            owners: Vec::new(),
        };
        for name in ["Example", "Application"] {
            fixture.owners.push(Box::new(C {
                name: CString::new(name).unwrap(),
                methods: Vec::new(),
                fields: Vec::new(),
                parent: 0,
            }));
            fixture
                .classes
                .push(&**fixture.owners.last().unwrap() as *const C as usize);
        }
        for kind in [1, 8, 12, 13, 0x0e, 0x12, 0x11] {
            fixture.types.push(Box::new(Ty {
                kind,
                byref: false,
                class: fixture.classes[0],
            }));
        }
        fixture.add_method(1, "get_unityVersion", 0x10, 0x0e, &[], 0);
        for (offset, kind) in [
            (std::mem::offset_of!(Object, value), 12),
            (std::mem::offset_of!(Object, source), 12),
            (std::mem::offset_of!(Object, count), 8),
            (std::mem::offset_of!(Object, canary), 8),
        ] {
            fixture.fields.push(Box::new(F {
                offset,
                kind: fixture.ty(kind),
                flags: 0,
                parent: fixture.classes[0],
            }));
            fixture.owners[0]
                .fields
                .push(&**fixture.fields.last().unwrap() as *const F as usize);
        }
        fixture
    }
    fn ty(&self, kind: i32) -> usize {
        self.types
            .iter()
            .find(|t| t.kind == kind)
            .map(|t| &**t as *const Ty as usize)
            .unwrap()
    }
    fn add_method(
        &mut self,
        owner: usize,
        name: &str,
        flags: u32,
        returns: i32,
        params: &[i32],
        entry: usize,
    ) -> usize {
        self.methods.push(Box::new(M {
            entry,
            name: CString::new(name).unwrap(),
            flags,
            params: params.iter().map(|&kind| self.ty(kind)).collect(),
            returns: self.ty(returns),
            generic: false,
        }));
        let raw = &**self.methods.last().unwrap() as *const M as usize;
        self.owners[owner].methods.push(raw);
        raw
    }
    fn api(&self) -> Il2cpp {
        CURRENT.with(|v| v.set(self as *const Self as usize));
        // SAFETY: private oracle implements every exposed standard export with process-call lifetime.
        unsafe { Il2cpp::bind(0x42, resolve, &[]) }.unwrap()
    }
    fn plan(
        &self,
        mode: PatchMode,
        field: usize,
        value: f64,
    ) -> Result<ManagedHookPlan, &'static str> {
        ManagedHookPlan::patch(
            self.api(),
            &PatchSpec {
                assembly: c"Assembly-CSharp",
                namespace: c"",
                class: c"Example",
                method: c"Run",
                argc: 2,
                object_register: 0,
                field_offset: Some(field),
                source_offset: None,
                multiplier: 2.0,
                value,
                integer: false,
                mode,
            },
        )
    }
}
fn fixture() -> &'static Fixture {
    CURRENT.with(|v| unsafe { &*(v.get() as *const Fixture) })
}
fn method(p: Ptr) -> &'static M {
    unsafe { &*(p as *const M) }
}
fn class(p: Ptr) -> &'static C {
    unsafe { &*(p as *const C) }
}
fn field(p: Ptr) -> &'static F {
    unsafe { &*(p as *const F) }
}
fn ty(p: Ptr) -> &'static Ty {
    unsafe { &*(p as *const Ty) }
}
fn width(raw: usize) -> usize {
    match ty(raw as Ptr).kind {
        2 => 1,
        0x0e | 0x12 | 0x14 | 0x1c | 0x1d => 8,
        _ => 4,
    }
}
extern "C-unwind" fn domain() -> Ptr {
    0x42 as Ptr
}
extern "C-unwind" fn assemblies(_: Ptr, n: &mut usize) -> *const Ptr {
    *n = 1;
    static ASSEMBLY: usize = 0x43;
    (&ASSEMBLY as *const usize).cast()
}
extern "C-unwind" fn image(p: Ptr) -> Ptr {
    p
}
extern "C-unwind" fn image_name(_: Ptr) -> *const c_char {
    c"Assembly-CSharp.dll".as_ptr()
}
extern "C-unwind" fn from_name(_: Ptr, _: *const c_char, n: *const c_char) -> Ptr {
    let name = unsafe { CStr::from_ptr(n) };
    fixture()
        .classes
        .iter()
        .copied()
        .find(|&c| class(c as Ptr).name.as_c_str() == name)
        .unwrap_or(0) as Ptr
}
extern "C-unwind" fn methods(c: Ptr, iter: &mut Ptr) -> Ptr {
    let index = *iter as usize;
    *iter = (index + 1) as Ptr;
    class(c).methods.get(index).copied().unwrap_or(0) as Ptr
}
extern "C-unwind" fn fields(c: Ptr, iter: &mut Ptr) -> Ptr {
    let index = *iter as usize;
    *iter = (index + 1) as Ptr;
    class(c).fields.get(index).copied().unwrap_or(0) as Ptr
}
extern "C-unwind" fn by_name(c: Ptr, n: *const c_char, argc: i32) -> Ptr {
    let n = unsafe { CStr::from_ptr(n) };
    class(c)
        .methods
        .iter()
        .copied()
        .find(|&m| {
            method(m as Ptr).name.as_c_str() == n && method(m as Ptr).params.len() == argc as usize
        })
        .unwrap_or(0) as Ptr
}
extern "C-unwind" fn method_name(p: Ptr) -> *const c_char {
    method(p).name.as_ptr()
}
extern "C-unwind" fn flags(p: Ptr, i: &mut u32) -> u32 {
    *i = 0;
    method(p).flags
}
extern "C-unwind" fn count(p: Ptr) -> u32 {
    method(p).params.len() as u32
}
extern "C-unwind" fn param(p: Ptr, i: u32) -> Ptr {
    method(p).params[i as usize] as Ptr
}
extern "C-unwind" fn returns(p: Ptr) -> Ptr {
    method(p).returns as Ptr
}
extern "C-unwind" fn generic(p: Ptr) -> bool {
    method(p).generic
}
extern "C-unwind" fn inflated(_: Ptr) -> bool {
    false
}
extern "C-unwind" fn type_kind(p: Ptr) -> i32 {
    ty(p).kind
}
extern "C-unwind" fn byref(p: Ptr) -> bool {
    ty(p).byref
}
extern "C-unwind" fn from_type(p: Ptr) -> Ptr {
    ty(p).class as Ptr
}
extern "C-unwind" fn parent(p: Ptr) -> Ptr {
    class(p).parent as Ptr
}
extern "C-unwind" fn field_parent(p: Ptr) -> Ptr {
    field(p).parent as Ptr
}
extern "C-unwind" fn field_kind(p: Ptr) -> Ptr {
    field(p).kind as Ptr
}
extern "C-unwind" fn field_flags(p: Ptr) -> i32 {
    field(p).flags
}
extern "C-unwind" fn offset(p: Ptr) -> usize {
    field(p).offset
}
extern "C-unwind" fn get(o: Ptr, f: Ptr, out: Ptr) {
    unsafe {
        std::ptr::copy_nonoverlapping(
            o.cast::<u8>().add(field(f).offset),
            out.cast(),
            width(field(f).kind),
        )
    }
}
extern "C-unwind" fn set(o: Ptr, f: Ptr, input: Ptr) {
    unsafe {
        std::ptr::copy_nonoverlapping(
            input.cast::<u8>(),
            o.cast::<u8>().add(field(f).offset),
            width(field(f).kind),
        )
    }
}
extern "C-unwind" fn object_class(o: Ptr) -> Ptr {
    unsafe { (*(o as *const Object)).class as Ptr }
}
extern "C-unwind" fn root(o: Ptr, _: bool) -> usize {
    ROOTS.fetch_add(1, Ordering::SeqCst);
    o as usize
}
extern "C-unwind" fn target(r: usize) -> Ptr {
    r as Ptr
}
extern "C-unwind" fn free(_: usize) {
    ROOTS.fetch_sub(1, Ordering::SeqCst);
}
extern "C-unwind" fn attached() -> Ptr {
    1 as Ptr
}
static VERSION: [u16; 11] = [54, 48, 48, 48, 46, 48, 46, 54, 49, 102, 49];
extern "C-unwind" fn invoke(m: Ptr, o: Ptr, args: *mut Ptr, e: &mut Ptr) -> Ptr {
    if method(m).name.as_c_str() != c"get_unityVersion" {
        return EXTRA_INVOKE.with(|f| f.get().expect("managed oracle"))(m, o, args, e);
    }
    *e = std::ptr::null_mut();
    VERSION.as_ptr() as Ptr
}
extern "C-unwind" fn string_len(_: Ptr) -> i32 {
    11
}
extern "C-unwind" fn string_chars(p: Ptr) -> *const u16 {
    p.cast()
}
extern "C-unwind" fn unbox(p: Ptr) -> Ptr {
    unsafe { p.cast::<u8>().add(8).cast() }
}
extern "C" fn resolve(_: Ptr, n: HostStr<'_>) -> usize {
    match n.to_cstr().unwrap().to_bytes() {
        b"il2cpp_domain_get" => domain as usize,
        b"il2cpp_domain_get_assemblies" => assemblies as usize,
        b"il2cpp_assembly_get_image" => image as usize,
        b"il2cpp_image_get_name" => image_name as usize,
        b"il2cpp_class_from_name" => from_name as usize,
        b"il2cpp_class_get_methods" => methods as usize,
        b"il2cpp_class_get_method_from_name" => by_name as usize,
        b"il2cpp_class_get_fields" => fields as usize,
        b"il2cpp_class_get_parent" => parent as usize,
        b"il2cpp_method_get_name" => method_name as usize,
        b"il2cpp_method_get_flags" => flags as usize,
        b"il2cpp_method_get_param_count" => count as usize,
        b"il2cpp_method_get_param" => param as usize,
        b"il2cpp_method_get_return_type" => returns as usize,
        b"il2cpp_method_is_generic" => generic as usize,
        b"il2cpp_method_is_inflated" => inflated as usize,
        b"il2cpp_type_get_type" => type_kind as usize,
        b"il2cpp_type_is_byref" => byref as usize,
        b"il2cpp_class_from_type" => from_type as usize,
        b"il2cpp_field_get_offset" => offset as usize,
        b"il2cpp_field_get_type" => field_kind as usize,
        b"il2cpp_field_get_flags" => field_flags as usize,
        b"il2cpp_field_get_parent" => field_parent as usize,
        b"il2cpp_field_get_value" => get as usize,
        b"il2cpp_field_set_value" => set as usize,
        b"il2cpp_object_get_class" => object_class as usize,
        b"il2cpp_object_unbox" => unbox as usize,
        b"il2cpp_gchandle_new" => root as usize,
        b"il2cpp_gchandle_get_target" => target as usize,
        b"il2cpp_gchandle_free" => free as usize,
        b"il2cpp_thread_current" => attached as usize,
        b"il2cpp_runtime_invoke" => invoke as usize,
        b"il2cpp_string_length" => string_len as usize,
        b"il2cpp_string_chars" => string_chars as usize,
        _ => 0,
    }
}
extern "C" fn log(_: HostStr<'_>, _: HostStr<'_>) {}
fn host() -> HostApiV0 {
    let mut h = HostApiV0::stubbed();
    h.log = log;
    h
}
unsafe extern "C-unwind" fn original(object: *mut Object, n: i32, f: f64, _method: Ptr) -> f64 {
    CALLS.fetch_add(1, Ordering::SeqCst);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 1);
    unsafe {
        (*object).count = n;
        (*object).value += 1.0;
        (*object).source = 17.0;
        (*object).value as f64 + f
    }
}
fn setup() -> Fixture {
    let mut f = Fixture::new();
    f.add_method(0, "Run", 0, 13, &[8, 13], original as usize);
    f
}
fn packet(object: &mut Object, method: Method) -> Frame {
    let mut p = Frame {
        x: [0; 10],
        q: [0; 128],
        result_x: [0; 2],
        result_q: [0; 32],
    };
    p.x[0] = object as *mut Object as u64;
    p.x[1] = 9;
    p.x[2] = method.as_ptr() as u64;
    p.q[..8].copy_from_slice(&3.5f64.to_le_bytes());
    p
}

#[test]
fn gateway_layout_and_scalar_rejections() {
    assert_eq!(std::mem::size_of::<Frame>(), 256);
    assert_eq!(std::mem::offset_of!(Frame, q), 80);
    assert_eq!(std::mem::offset_of!(Frame, result_x), 208);
    assert_eq!(std::mem::offset_of!(Frame, result_q), 224);
    assert_eq!(encode_return(13, 1.25), Ok((0, 1.25f64.to_bits())));
    assert!(encode_return(8, 2147483648.0).is_err());
    assert!(encode_return(2, 2.0).is_err());
    assert!(encode_return(0x12, 1.0).is_err());
    assert!(encode_return(10, 9223372036854775808.0).is_err());
    assert!(encode_return(13, f64::NAN).is_err());
    assert!(!startup_prefix(&[
        0xd503201f, 0xd65f03c0, 0xd503201f, 0xd503201f
    ]));
    assert!(!startup_prefix(&[
        0x14000080, 0xd503201f, 0xd503201f, 0xd503201f
    ]));
    assert!(startup_prefix(&[0xd503201f; 4]));
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn real_bridge_preserves_mixed_arguments_and_executes_all_five_modes() {
    let _lock = SERIAL.lock().unwrap();
    let f = setup();
    ACTIVE.store(true, Ordering::Release);
    for mode in [
        PatchMode::Scale,
        PatchMode::Freeze,
        PatchMode::ForceReturn,
        PatchMode::Trace,
        PatchMode::Bypass,
    ] {
        let plan = f
            .plan(mode, std::mem::offset_of!(Object, value), 42.0)
            .unwrap();
        let mut o = Object {
            class: f.classes[0],
            value: 4.0,
            source: 8.0,
            count: 0,
            canary: 0x12345678,
        };
        let mut frame = packet(&mut o, plan.method);
        let before = CALLS.load(Ordering::SeqCst);
        plan.run(plan.entry, &host(), &mut frame);
        let result = f64::from_le_bytes(frame.result_q[..8].try_into().unwrap());
        match mode {
            PatchMode::Scale => {
                assert_eq!(o.value, 9.0);
                assert_eq!(result, 12.5);
            }
            PatchMode::Freeze => {
                assert_eq!(o.value, 42.0);
                assert_eq!(result, 8.5);
            }
            PatchMode::ForceReturn => assert_eq!(result, 42.0),
            PatchMode::Trace => assert_eq!(result, 8.5),
            PatchMode::Bypass => {
                assert_eq!(result, 0.0);
                assert_eq!(o.value, 4.0);
            }
        }
        assert_eq!(
            CALLS.load(Ordering::SeqCst) - before,
            usize::from(mode != PatchMode::Bypass)
        );
        assert_eq!(o.canary, 0x12345678);
        assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
        if mode != PatchMode::Bypass {
            assert_eq!(o.count, 9);
        }
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn freeze_reads_live_source_after_original_and_preserves_exact_i32_fields() {
    let _lock = SERIAL.lock().unwrap();
    let f = setup();
    ACTIVE.store(true, Ordering::Release);
    for (integer, source, value, expected) in [
        (false, Some(std::mem::offset_of!(Object, source)), 0.0, 17),
        (true, None, 21.0, 21),
        (true, Some(std::mem::offset_of!(Object, canary)), 0.0, 123),
    ] {
        let spec = PatchSpec {
            assembly: c"Assembly-CSharp",
            namespace: c"",
            class: c"Example",
            method: c"Run",
            argc: 2,
            object_register: 0,
            field_offset: Some(if integer {
                std::mem::offset_of!(Object, count)
            } else {
                std::mem::offset_of!(Object, value)
            }),
            source_offset: source,
            multiplier: 1.0,
            value,
            integer,
            mode: PatchMode::Freeze,
        };
        let plan = ManagedHookPlan::patch(f.api(), &spec).unwrap();
        let mut o = Object {
            class: f.classes[0],
            value: 4.0,
            source: 8.0,
            count: 0,
            canary: 123,
        };
        let mut frame = packet(&mut o, plan.method);
        plan.run(plan.entry, &host(), &mut frame);
        if integer {
            assert_eq!(o.count, expected);
        } else {
            assert_eq!(o.value, expected as f32);
        }
        assert_eq!(
            f64::from_le_bytes(frame.result_q[..8].try_into().unwrap()),
            8.5
        );
        assert_eq!(o.canary, 123);
        assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn invalid_offsets_fields_overloads_generics_and_byrefs_are_rejected() {
    let _lock = SERIAL.lock().unwrap();
    let mut f = setup();
    assert!(f.plan(PatchMode::Scale, 123456, 0.0).is_err());
    assert!(f
        .plan(PatchMode::Scale, std::mem::offset_of!(Object, count), 0.0)
        .is_err());
    f.fields[0].flags = 0x20;
    assert!(f
        .plan(PatchMode::Scale, std::mem::offset_of!(Object, value), 0.0)
        .is_err());
    f.fields[0].flags = 0;
    let index = f.methods.len() - 1;
    f.methods[index].generic = true;
    assert!(f.plan(PatchMode::Trace, 0, 0.0).is_err());
    f.methods[index].generic = false;
    f.types.iter_mut().find(|t| t.kind == 13).unwrap().byref = true;
    assert!(f.plan(PatchMode::Trace, 0, 0.0).is_err());
    f.types.iter_mut().find(|t| t.kind == 13).unwrap().byref = false;
    f.add_method(0, "Run", 0, 13, &[12, 12], original as usize);
    assert!(f.plan(PatchMode::Trace, 0, 0.0).is_err());
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn installed_gateway_changes_the_real_entry_and_rejects_conflicts() {
    let _lock = SERIAL.lock().unwrap();
    let mut f = setup();
    ACTIVE.store(true, Ordering::Release);
    // SAFETY: private RW reservation, initialized before sealing RX, retained for installed hook lifetime.
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(page, libc::MAP_FAILED);
    let address = original as usize;
    let words = [
        0xd503201fu32,
        0x58000051,
        0xd61f0220,
        address as u32,
        (address >> 32) as u32,
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(words.as_ptr(), page.cast(), words.len());
        assert_eq!(
            libc::mprotect(page, 4096, libc::PROT_READ | libc::PROT_EXEC),
            0
        );
    }
    f.methods.last_mut().unwrap().entry = page as usize;
    let plan = f
        .plan(PatchMode::Scale, std::mem::offset_of!(Object, value), 0.0)
        .unwrap();
    let method = plan.method;
    unsafe { install(plan, host()) }.unwrap();
    let duplicate = f
        .plan(PatchMode::Scale, std::mem::offset_of!(Object, value), 0.0)
        .unwrap();
    assert!(unsafe { install(duplicate, host()) }.is_err());
    let mut o = Object {
        class: f.classes[0],
        value: 4.0,
        source: 8.0,
        count: 0,
        canary: 99,
    };
    let call: unsafe extern "C-unwind" fn(*mut Object, i32, f64, Ptr) -> f64 =
        unsafe { std::mem::transmute(page) };
    assert_eq!(unsafe { call(&mut o, 9, 3.5, method.as_ptr()) }, 12.5);
    assert_eq!(o.value, 9.0);
    assert_eq!(o.canary, 99);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    std::mem::forget(f); // The installed gateway's immutable fixture metadata remains process-pinned.
}

unsafe extern "C-unwind" fn throwing(_: Ptr, _: i32, _: f64, _: Ptr) -> f64 {
    panic!("fixture managed unwind");
}
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn original_unwind_releases_callback_roots_across_the_assembly_frame() {
    let _lock = SERIAL.lock().unwrap();
    let mut f = setup();
    ACTIVE.store(true, Ordering::Release);
    f.methods.last_mut().unwrap().entry = throwing as usize;
    let plan = f.plan(PatchMode::Trace, 0, 0.0).unwrap();
    let mut o = Object {
        class: f.classes[0],
        value: 1.0,
        source: 1.0,
        count: 0,
        canary: 77,
    };
    let mut frame = packet(&mut o, plan.method);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plan.run(
            plan.entry,
            &host(),
            &mut frame
        )))
        .is_err()
    );
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    assert_eq!(o.canary, 77);
}

#[path = "hud_fixture.rs"]
mod hud_fixture;
#[path = "touch_fixture.rs"]
mod touch_fixture;

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn startup_absolute_gateway_works_when_the_whole_branch_window_is_reserved() {
    let _lock = SERIAL.lock().unwrap();
    let mut f = setup();
    ACTIVE.store(true, Ordering::Release);
    let size = 512 * 1024 * 1024usize;
    // Reserve address space only (PROT_NONE), forcing a genuine no-free-near-page condition.
    let reservation = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(reservation, libc::MAP_FAILED);
    let target = unsafe { reservation.cast::<u8>().add(size / 2) };
    assert_eq!(
        unsafe { libc::mprotect(target.cast(), 4096, libc::PROT_READ | libc::PROT_WRITE) },
        0
    );
    let address = original as usize;
    let words = [
        0xd503201fu32,
        0xd503201f,
        0xd503201f,
        0xd503201f,
        0x58000051,
        0xd61f0220,
        address as u32,
        (address >> 32) as u32,
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(words.as_ptr(), target.cast(), words.len());
        assert_eq!(
            libc::mprotect(target.cast(), 4096, libc::PROT_READ | libc::PROT_EXEC),
            0
        );
    }
    f.methods.last_mut().unwrap().entry = target as usize;
    let plan = f
        .plan(PatchMode::Scale, std::mem::offset_of!(Object, value), 0.0)
        .unwrap();
    let method = plan.method;
    unsafe { install(plan, host()) }.unwrap();
    assert_eq!(
        unsafe { target.cast::<u32>().read() },
        crate::arm64_detour::LDR_X17_P8,
        "actual absolute four-word redirect"
    );
    let call: unsafe extern "C-unwind" fn(*mut Object, i32, f64, Ptr) -> f64 =
        unsafe { std::mem::transmute(target) };
    let mut o = Object {
        class: f.classes[0],
        value: 4.0,
        source: 8.0,
        count: 0,
        canary: 88,
    };
    assert_eq!(unsafe { call(&mut o, 9, 3.5, method.as_ptr()) }, 12.5);
    assert_eq!(o.value, 9.0);
    assert_eq!(o.canary, 88);
    assert_eq!(ROOTS.load(Ordering::SeqCst), 0);
    // Installed code and its metadata are process-pinned; only one page is committed physically.
    std::mem::forget(f);
}
