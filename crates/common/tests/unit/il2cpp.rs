//! INPUT:  private items of `src/il2cpp.rs` via `super`, a fake IL2CPP export table
//! OUTPUT:  binding, checked scalar methods, metadata, version, GC-root and string checks
//! POS:    keeps test bodies out of `src/il2cpp.rs`; declared there with `#[path]`
use super::*;
use crate::registry::HostStr;
use std::sync::Mutex;

const BASE: usize = 0x7000;

#[repr(C)]
struct FakeMethod {
    entry: usize,
    name: &'static CStr,
    argc: i32,
    flags: u32,
    returns: i32,
}
#[repr(C)]
struct FakeClass {
    namespace: &'static CStr,
    name: &'static CStr,
    methods: &'static [FakeMethod],
    field: (&'static CStr, usize),
}
struct FakeImage {
    name: &'static CStr,
    classes: &'static [FakeClass],
}
#[repr(C)]
struct FakeType(i32);

static SCREEN_METHODS: [FakeMethod; 2] = [
    FakeMethod { entry: 0x1234, name: c"get_width", argc: 0, flags: 0x10, returns: 8 },
    FakeMethod { entry: 0, name: c"get_height", argc: 0, flags: 0x10, returns: 8 },
];
static APPLICATION_METHODS: [FakeMethod; 1] =
    [FakeMethod { entry: 0x5678, name: c"get_unityVersion", argc: 0, flags: 0x10, returns: TYPE_STRING }];
static CORE: [FakeClass; 2] = [
    FakeClass { namespace: c"UnityEngine", name: c"Screen", methods: &SCREEN_METHODS, field: (c"m_Width", 0x18) },
    FakeClass { namespace: c"UnityEngine", name: c"Application", methods: &APPLICATION_METHODS, field: (c"", 0) },
];
static GAME: [FakeClass; 1] =
    [FakeClass { namespace: c"", name: c"HeroController", methods: &[], field: (c"speed", 0x40) }];
static IMAGES: [FakeImage; 2] = [
    FakeImage { name: c"UnityEngine.CoreModule.dll", classes: &CORE },
    FakeImage { name: c"Assembly-CSharp.dll", classes: &GAME },
];
static ASSEMBLIES: [usize; 2] = [1, 2];
static TYPES: [FakeType; 3] = [FakeType(8), FakeType(TYPE_STRING), FakeType(TYPE_VOID)];
/// A managed string: length then UTF-16 units.
static VERSION: (i32, [u16; 11]) = (11, [54, 48, 48, 48, 46, 48, 46, 54, 49, 102, 49]);
static GC: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn image(raw: Ptr) -> &'static FakeImage {
    &IMAGES[raw as usize - 1]
}

extern "C" fn domain_get() -> Ptr {
    0xd0 as Ptr
}
extern "C" fn domain_get_assemblies(_domain: Ptr, count: *mut usize) -> *const Ptr {
    unsafe { *count = ASSEMBLIES.len() };
    ASSEMBLIES.as_ptr().cast()
}
extern "C" fn assembly_get_image(assembly: Ptr) -> Ptr {
    assembly
}
extern "C" fn image_get_name(image_raw: Ptr) -> *const c_char {
    image(image_raw).name.as_ptr()
}
extern "C" fn image_get_class_count(image_raw: Ptr) -> usize {
    image(image_raw).classes.len()
}
extern "C" fn image_get_class(image_raw: Ptr, index: usize) -> Ptr {
    &image(image_raw).classes[index] as *const FakeClass as Ptr
}
extern "C" fn class_from_name(image_raw: Ptr, namespace: *const c_char, name: *const c_char) -> Ptr {
    let (namespace, name) = unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) };
    image(image_raw)
        .classes
        .iter()
        .find(|class| class.namespace == namespace && class.name == name)
        .map_or(std::ptr::null_mut(), |class| class as *const FakeClass as Ptr)
}
extern "C" fn class_get_name(class: Ptr) -> *const c_char {
    unsafe { &*(class as *const FakeClass) }.name.as_ptr()
}
extern "C" fn class_get_nested_types(class: Ptr, cursor: *mut Ptr) -> Ptr {
    if class != &CORE[0] as *const FakeClass as Ptr { return std::ptr::null_mut(); }
    // SAFETY: the typed metadata adapter supplies its own live iterator slot.
    unsafe {
        if !(*cursor).is_null() { return std::ptr::null_mut(); }
        *cursor = 1usize as Ptr;
    }
    &CORE[1] as *const FakeClass as Ptr
}
extern "C" fn repeated_nested_type(_class: Ptr, _cursor: *mut Ptr) -> Ptr {
    &CORE[1] as *const FakeClass as Ptr
}
extern "C" fn class_get_method_from_name(class: Ptr, name: *const c_char, argc: i32) -> Ptr {
    let (class, name) = unsafe { (&*(class as *const FakeClass), CStr::from_ptr(name)) };
    class
        .methods
        .iter()
        .find(|method| method.name == name && (argc == -1 || method.argc == argc))
        .map_or(std::ptr::null_mut(), |method| method as *const FakeMethod as Ptr)
}
extern "C" fn class_get_field_from_name(class: Ptr, name: *const c_char) -> Ptr {
    let (class, name) = unsafe { (&*(class as *const FakeClass), CStr::from_ptr(name)) };
    if class.field.0 == name { &class.field as *const (&CStr, usize) as Ptr } else { std::ptr::null_mut() }
}
extern "C" fn field_get_offset(field: Ptr) -> usize {
    unsafe { &*(field as *const (&CStr, usize)) }.1
}
extern "C" fn method_get_flags(method: Ptr, implementation: *mut u32) -> u32 {
    unsafe { *implementation = 0 };
    unsafe { &*(method as *const FakeMethod) }.flags
}
extern "C" fn method_get_param_count(method: Ptr) -> u32 {
    unsafe { &*(method as *const FakeMethod) }.argc as u32
}
extern "C" fn method_get_return_type(method: Ptr) -> Ptr {
    let kind = unsafe { &*(method as *const FakeMethod) }.returns;
    TYPES.iter().find(|ty| ty.0 == kind).map_or(std::ptr::null_mut(), |ty| ty as *const FakeType as Ptr)
}
extern "C" fn type_get_type(ty: Ptr) -> i32 {
    unsafe { &*(ty as *const FakeType) }.0
}
extern "C" fn runtime_invoke(method: Ptr, _object: Ptr, _args: *mut Ptr, _exception: *mut Ptr) -> Ptr {
    assert_eq!(unsafe { &*(method as *const FakeMethod) }.name, c"get_unityVersion");
    &VERSION as *const (i32, [u16; 11]) as Ptr
}
extern "C" fn string_length(string: Ptr) -> i32 {
    unsafe { &*(string as *const (i32, [u16; 11])) }.0
}
extern "C" fn string_chars(string: Ptr) -> *const u16 {
    unsafe { &*(string as *const (i32, [u16; 11])) }.1.as_ptr()
}
extern "C" fn thread_current() -> Ptr {
    0x7e as Ptr
}
extern "C" fn gchandle_new(object: Ptr, pinned: bool) -> usize {
    GC.lock().unwrap().push(format!("new {:#x} {pinned}", object as usize));
    if object.is_null() {
        return 0;
    }
    // Modern ABI must preserve every handle bit.
    0xffff_ffff_0000_0000 | 0x2a
}
extern "C" fn gchandle_get_target(handle: usize) -> Ptr {
    GC.lock().unwrap().push(format!("target {handle:#x}"));
    0x99 as Ptr
}
extern "C" fn gchandle_free(handle: usize) {
    GC.lock().unwrap().push(format!("free {handle:#x}"));
}

extern "C" fn so_symbol(module: Ptr, name: HostStr<'_>) -> usize {
    assert_eq!(module as usize, BASE);
    let name = name.to_cstr().unwrap().to_str().unwrap();
    match name {
        "il2cpp_domain_get" => domain_get as usize,
        "il2cpp_domain_get_assemblies" => domain_get_assemblies as usize,
        "il2cpp_assembly_get_image" => assembly_get_image as usize,
        "il2cpp_image_get_name" => image_get_name as usize,
        "il2cpp_image_get_class_count" => image_get_class_count as usize,
        "il2cpp_image_get_class" => image_get_class as usize,
        "il2cpp_class_from_name" => class_from_name as usize,
        "il2cpp_class_get_name" => class_get_name as usize,
        "il2cpp_class_get_nested_types" => class_get_nested_types as usize,
        "il2cpp_class_get_method_from_name" => class_get_method_from_name as usize,
        "il2cpp_class_get_field_from_name" => class_get_field_from_name as usize,
        "il2cpp_field_get_offset" => field_get_offset as usize,
        "il2cpp_method_get_flags" => method_get_flags as usize,
        "il2cpp_method_get_param_count" => method_get_param_count as usize,
        "il2cpp_method_get_param" => method_get_param as usize,
        "il2cpp_method_get_return_type" => method_get_return_type as usize,
        "il2cpp_type_get_type" => type_get_type as usize,
        "il2cpp_type_is_byref" => type_is_byref as usize,
        "il2cpp_runtime_invoke" => runtime_invoke as usize,
        "il2cpp_string_length" => string_length as usize,
        "il2cpp_string_chars" => string_chars as usize,
        "il2cpp_thread_current" => thread_current as usize,
        "il2cpp_gchandle_new" => gchandle_new as usize,
        "il2cpp_gchandle_get_target" => gchandle_get_target as usize,
        "il2cpp_gchandle_free" => gchandle_free as usize,
        _ => 0,
    }
}

extern "C" fn only_class_lookup(module: Ptr, name: HostStr<'_>) -> usize {
    match name.to_cstr().unwrap().to_bytes() {
        b"il2cpp_class_from_name" => so_symbol(module, name),
        _ => 0,
    }
}

fn bound() -> Il2cpp {
    unsafe { Il2cpp::bind(BASE, so_symbol, &["il2cpp_domain_get", "il2cpp_class_from_name"]) }.unwrap()
}

#[test]
fn bind_reports_exactly_the_missing_required_exports() {
    let missing = unsafe { Il2cpp::bind(BASE, so_symbol, &["il2cpp_domain_get", "il2cpp_gc_collect", "il2cpp_nope"]) };
    assert_eq!(missing.err(), Some(vec!["il2cpp_gc_collect".to_owned(), "il2cpp_nope".to_owned()]));
    let api = bound();
    assert!(api.has("il2cpp_runtime_invoke") && !api.has("il2cpp_gc_collect") && !api.has("il2cpp_nope"));
    assert!(!api.gc_collect(0) && api.gc_used_size().is_none());
    assert!(EXPORTS.contains(&"il2cpp_gc_get_heap_size"));
}

#[test]
fn metadata_walks_find_images_classes_methods_and_fields() {
    let api = bound();
    let images = api.images();
    assert_eq!(images.len(), 2);
    assert_eq!(api.inventory(), Ok(images.clone()));
    let no_domain = unsafe { Il2cpp::bind(BASE, only_class_lookup, &[]) }.unwrap();
    assert_eq!(no_domain.inventory(), Err(Inventory::NoDomain));
    assert!(no_domain.images().is_empty());
    assert_eq!(api.image_name(images[1]).as_deref(), Some("Assembly-CSharp.dll"));
    assert_eq!(api.find_image("Assembly-CSharp"), Some(images[1]));

    let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
    assert_eq!(api.class_name(screen).as_deref(), Some("Screen"));
    assert_eq!(api.find_class(c"", c"Screen"), None, "namespace must match");
    assert_eq!(api.find_class_named(c"HeroController"), api.class_from_name(images[1], c"", c"HeroController"));
    assert_eq!(api.find_class_named(c"Nope"), None);

    let width = api.method(screen, c"get_width", 0).unwrap();
    assert_eq!(api.method_entry(width), Some(0x1234));
    assert_eq!(api.method_entry(api.method(screen, c"get_height", 0).unwrap()), None, "zero entry");
    assert_eq!(api.method(screen, c"get_width", 1), None);
    assert!(api.method_shape(width, 0, true) && !api.method_shape(width, 0, false) && !api.method_shape(width, 1, true));
    assert_eq!(api.method_return_type(width).and_then(|ty| api.type_kind(ty)), Some(8));

    assert_eq!(api.field(screen, c"m_Width").and_then(|f| api.field_offset(f)), Some(0x18));
    assert_eq!(api.field(screen, c"missing"), None);
}

#[test]
fn nested_metadata_rejects_foreign_handles_missing_exports_and_nonadvancing_cursors() {
    let api = bound();
    let owner = api.find_class(c"UnityEngine", c"Screen").unwrap();
    let child = api.find_class(c"UnityEngine", c"Application").unwrap();
    assert_eq!(api.nested_classes(owner), Some(vec![child]));
    assert_eq!(api.nested_classes(child), Some(vec![]));
    let foreign = Class::new(owner.as_ptr(), BASE + 1).unwrap();
    assert_eq!(api.nested_classes(foreign), None);
    // SAFETY: both symbol tables and every fake metadata object are static.
    let missing = unsafe { Il2cpp::bind(BASE, only_class_lookup, &[]) }.unwrap();
    assert_eq!(missing.nested_classes(owner), None);
    extern "C" fn repeated(module: Ptr, name: HostStr<'_>) -> usize {
        if name.to_cstr().unwrap() == c"il2cpp_class_get_nested_types" { repeated_nested_type as usize }
        else { so_symbol(module, name) }
    }
    // SAFETY: same static fixture, with an intentionally nonadvancing cursor.
    let broken = unsafe { Il2cpp::bind(BASE, repeated, &[]) }.unwrap();
    assert_eq!(broken.nested_classes(owner), None);
}

#[test]
fn unity_version_reads_the_static_getter() {
    let api = bound();
    let application = api.find_class(c"UnityEngine", c"Application").unwrap();
    assert_eq!(api.unity_version(application).as_deref(), Ok("6000.0.61f1"));
    VERSION_BYREF.with(|cell| cell.set(true));
    let rejected = api.unity_version(application);
    VERSION_BYREF.with(|cell| cell.set(false));
    assert_eq!(rejected, Err("Unity version getter return type mismatch"));
    let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
    assert_eq!(api.unity_version(screen), Err("Unity version getter unavailable"));
}

#[test]
fn verified_roots_keep_metadata_errors_and_reject_unknown_stripped_images() {
    let api = bound();
    let application = api.find_class(c"UnityEngine", c"Application").unwrap();
    assert!(api.verified_gc_handles(application).is_ok());
    VERSION_BYREF.with(|cell| cell.set(true));
    let rejected = api.verified_gc_handles(application);
    VERSION_BYREF.with(|cell| cell.set(false));
    assert!(matches!(rejected, Err("Unity version getter return type mismatch")));
    let foreign = Class(application.0, BASE + 1);
    assert!(matches!(api.verified_gc_handles(foreign),
        Err("Application belongs to another IL2CPP module")));
    let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
    assert!(matches!(api.verified_gc_handles(screen),
        Err("Unity version getter unavailable; native GC-handle ABI unverified")));
}

#[test]
fn strings_compare_and_copy_with_bounds() {
    let api = bound();
    let string = &VERSION as *const (i32, [u16; 11]) as Ptr;
    unsafe {
        assert!(api.string_eq(string, "6000.0.61f1"));
        assert!(!api.string_eq(string, "6000.0.61f2") && !api.string_eq(string, "6000"));
        assert!(!api.string_eq(std::ptr::null_mut(), ""));
        assert_eq!(api.string_utf16(string, 11).map(|u| u.len()), Some(11));
        assert_eq!(api.string_utf16(string, 10), None, "longer than max");
    }
}

#[test]
fn gc_roots_use_the_exact_u32_or_pointer_width_abi_and_free_once() {
    let api = bound();
    let legacy = unsafe { Il2cpp::bind(BASE, legacy_symbol, &[]) }.unwrap();
    GC.lock().unwrap().clear();
    {
        let root = unsafe { legacy.gc_handles(GcHandleAbi::U32).unwrap().root(0x55 as Ptr) }.unwrap();
        assert_eq!(root.target() as usize, 0x99);
    }
    {
        let _root = unsafe { api.gc_handles(GcHandleAbi::PointerWidth).unwrap().root(0x56 as Ptr) }.unwrap();
    }
    // A failed root is never freed.
    assert!(unsafe { api.gc_handles(GcHandleAbi::PointerWidth).unwrap().root(std::ptr::null_mut()) }.is_none());
    assert_eq!(
        *GC.lock().unwrap(),
        [
            "new 0x55 false",
            "target 0x2a",
            "free 0x2a",
            "new 0x56 false",
            "free 0xffffffff0000002a",
            "new 0x0 false",
        ]
    );
}

extern "C" fn gchandle_new_u32(object: Ptr, pinned: bool) -> u32 {
    GC.lock().unwrap().push(format!("new {:#x} {pinned}", object as usize));
    if object.is_null() { 0 } else { 0x2a }
}
extern "C" fn gchandle_target_u32(handle: u32) -> Ptr {
    GC.lock().unwrap().push(format!("target {handle:#x}"));
    0x99 as Ptr
}
extern "C" fn gchandle_free_u32(handle: u32) {
    GC.lock().unwrap().push(format!("free {handle:#x}"));
}
extern "C" fn legacy_symbol(module: Ptr, name: HostStr<'_>) -> usize {
    match name.to_cstr().unwrap().to_bytes() {
        b"il2cpp_gchandle_new" => gchandle_new_u32 as usize,
        b"il2cpp_gchandle_get_target" => gchandle_target_u32 as usize,
        b"il2cpp_gchandle_free" => gchandle_free_u32 as usize,
        _ => so_symbol(module, name),
    }
}

#[test]
fn method_parameter_indices_are_checked_before_entering_native_code() {
    let api = bound();
    let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
    let method = api.method(screen, c"get_width", 0).unwrap();
    assert_eq!(api.method_param(method, 0), None);
    assert_eq!(api.method_param(method, u32::MAX), None);
}
extern "C" fn method_get_param(_method: Ptr, _index: u32) -> Ptr {
    panic!("out-of-bounds parameter lookup reached native code");
}

extern "C" fn second_module_symbol(_: Ptr, name: HostStr<'_>) -> usize {
    so_symbol(BASE as Ptr, name)
}
#[test]
fn metadata_from_another_module_is_rejected_before_native_access() {
    let first = bound();
    let second = unsafe { Il2cpp::bind(BASE + 0x1000, second_module_symbol, &[]) }.unwrap();
    let image = first.images()[0];
    let class = first.find_class(c"UnityEngine", c"Screen").unwrap();
    let method = first.method(class, c"get_width", 0).unwrap();
    let field = first.field(class, c"m_Width").unwrap();
    let ty = first.method_return_type(method).unwrap();
    assert_eq!(second.image_name(image), None);
    assert_eq!(second.class_from_name(image, c"UnityEngine", c"Screen"), None);
    assert_eq!(second.class_name(class), None);
    assert_eq!(second.method(class, c"get_width", 0), None);
    assert_eq!(second.method_entry(method), None);
    assert_eq!(second.method_param_count(method), None);
    assert_eq!(second.method_return_type(method), None);
    assert_eq!(second.field_offset(field), None);
    assert_eq!(second.type_kind(ty), None);
    assert!(!second.method_shape(method, 0, true));
}

thread_local! { static VERSION_BYREF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
extern "C" fn type_is_byref(ty: Ptr) -> bool {
    VERSION_BYREF.with(|cell| cell.get()) && type_get_type(ty) == TYPE_STRING
}
#[test]
fn native_method_signature_requires_exact_staticness_parameters_and_return_kind() {
    let api = bound();
    let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
    let method = api.method(screen, c"get_width", 0).unwrap();
    assert!(api.method_signature(method, true, &[], 8));
    assert!(!api.method_signature(method, false, &[], 8));
    assert!(!api.method_signature(method, true, &[], TYPE_BOOLEAN));
    assert!(!api.method_signature(method, true, &[(8, false)], 8));
}

#[test]
fn static_i32_methods_keep_the_native_entry_paired_with_its_metadata() {
    unsafe extern "C-unwind" fn integer_getter(method: Ptr) -> i32 {
        // SAFETY: the method is the leaked fixture metadata paired below.
        assert_eq!(unsafe { &*method.cast::<FakeMethod>() }.name, c"IntegerGetter");
        777
    }
    let api = bound();
    let make_method = |flags, argc, returns, entry| {
        // The production bind contract requires process-lifetime metadata.
        let info = Box::leak(Box::new(FakeMethod {
            entry, name: c"IntegerGetter", argc, flags, returns,
        }));
        Method::new(info as *mut FakeMethod as Ptr, BASE).unwrap()
    };
    let method = make_method(METHOD_STATIC, 0, 8, integer_getter as usize);
    let getter = api.static_i32_method(method).unwrap();
    // SAFETY: this scalar fixture has no thread or other execution constraints.
    assert_eq!(unsafe { getter.call() }, 777);
    assert!(api.static_i32_method(make_method(0, 0, 8, integer_getter as usize)).is_none());
    assert!(api.static_i32_method(make_method(METHOD_STATIC, 1, 8, integer_getter as usize)).is_none());
    assert!(api.static_i32_method(make_method(METHOD_STATIC, 0, TYPE_STRING, integer_getter as usize)).is_none());
    assert!(api.static_i32_method(make_method(METHOD_STATIC, 0, 8, 0)).is_none());
    // SAFETY: a distinct process-pinned fixture binding with the same exports.
    let foreign = unsafe { Il2cpp::bind(BASE + 0x1000, second_module_symbol, &[]) }.unwrap();
    assert!(foreign.static_i32_method(method).is_none());
}

#[test]
fn static_i32_methods_refuse_byref_or_missing_return_metadata() {
    extern "C" fn all_byref(_: Ptr) -> bool { true }
    extern "C" fn byref_symbol(module: Ptr, name: HostStr<'_>) -> usize {
        if name.to_cstr().unwrap() == c"il2cpp_type_is_byref" {
            all_byref as usize
        } else {
            so_symbol(module, name)
        }
    }
    extern "C" fn missing_return(module: Ptr, name: HostStr<'_>) -> usize {
        if name.to_cstr().unwrap() == c"il2cpp_method_get_return_type" { 0 }
        else { so_symbol(module, name) }
    }
    for resolver in [byref_symbol as SoSymbolFn, missing_return as SoSymbolFn] {
        // SAFETY: these pinned fixture tables have the declared ABI; the
        // intentionally incompatible metadata must be rejected before casting.
        let api = unsafe { Il2cpp::bind(BASE, resolver, &[]) }.unwrap();
        let screen = api.find_class(c"UnityEngine", c"Screen").unwrap();
        let method = api.method(screen, c"get_width", 0).unwrap();
        assert!(api.static_i32_method(method).is_none());
    }
}
