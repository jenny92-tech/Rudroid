//! INPUT:  private items of `src/il2cpp.rs` via `super`, a fake IL2CPP export table
//! OUTPUT:  binding, checked scalar methods, metadata, version, GC-root and string checks
//! POS:    keeps test bodies out of `src/il2cpp.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::HostStr;
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

extern "C" fn ordinary_flag(_: Ptr) -> bool { false }

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
        "il2cpp_method_is_generic" | "il2cpp_method_is_inflated" => ordinary_flag as usize,
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

thread_local! {
    static UTF16_VALUE: std::cell::RefCell<Box<(i32, [u16; 16])>> =
        std::cell::RefCell::new(Box::new((0, [0; 16])));
}

extern "C-unwind" fn create_utf16(text: *const u16, length: i32) -> Ptr {
    assert!((0..=16).contains(&length));
    // SAFETY: the actual constructor wrapper lends this many input units.
    let text = unsafe { std::slice::from_raw_parts(text, length as usize) };
    UTF16_VALUE.with(|cell| {
        let mut value = cell.borrow_mut(); value.0 = length; value.1.fill(0);
        value.1[..text.len()].copy_from_slice(text);
        (&mut **value as *mut (i32, [u16; 16])).cast()
    })
}
extern "C-unwind" fn utf16_size(text: Ptr) -> i32 {
    // SAFETY: this export receives the stable fixture object returned above.
    unsafe { (*(text as *const (i32, [u16; 16]))).0 }
}
extern "C-unwind" fn utf16_chars(text: Ptr) -> *const u16 {
    // SAFETY: the fixture object's backing box stays live on this thread.
    unsafe { (&*(text as *const (i32, [u16; 16]))).1.as_ptr() }
}
extern "C" fn utf16_exports(_: Ptr, name: HostStr<'_>) -> usize {
    match name.to_cstr().unwrap().to_bytes() {
        b"il2cpp_string_new_utf16" => create_utf16 as usize,
        b"il2cpp_string_length" => utf16_size as usize,
        b"il2cpp_string_chars" => utf16_chars as usize,
        b"il2cpp_thread_current" => thread_current as usize,
        _ => 0,
    }
}

#[test]
fn utf16_constructor_crosses_the_real_export_boundary_without_text_conversion() {
    // SAFETY: every nonzero fixture export has its exact standard ABI.
    let api = unsafe { Il2cpp::bind(BASE, utf16_exports, &["il2cpp_string_new_utf16"]) }.unwrap();
    assert!(api.thread_attached());
    for units in [vec![65, 0, 66], vec![0x5143, 0x6c14, 0xd83d, 0xde00], vec![0xd800, 0x61, 0xdc00]] {
        // SAFETY: attached fixture thread, stable boxed result until next construction.
        let result = unsafe { api.new_string_utf16(&units) }.unwrap();
        // SAFETY: the returned fixture String remains live during this exact-unit copy.
        assert_eq!(unsafe { api.string_utf16(result, 16) }, Some(units));
    }
    // SAFETY: length zero is a valid standard UTF-16 constructor input.
    let empty = unsafe { api.new_string_utf16(&[]) }.unwrap();
    // SAFETY: the fixture owns this returned empty String through the assertion.
    assert!(unsafe { api.string_eq(empty, "") });
}

#[test]
fn utf16_constructor_rejects_missing_export_and_nonrepresentable_unit_count() {
    let api = bound();
    assert!(!api.has("il2cpp_string_new_utf16"));
    // SAFETY: missing export returns before any native construction.
    assert!(unsafe { api.new_string_utf16(&[0, 0xd800]) }.is_none());
    assert_eq!(utf16_length(0), Some(0));
    assert_eq!(utf16_length(i32::MAX as usize), Some(i32::MAX));
    assert_eq!(utf16_length(i32::MAX as usize + 1), None);
    assert_eq!(utf16_length(usize::MAX), None);
    assert!(EXPORTS.contains(&"il2cpp_string_new_utf16"));
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
fn declared_fields_cross_checked_exports_and_reject_broken_or_foreign_metadata() {
    thread_local! { static MODE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
    extern "C-unwind" fn fields(owner: Ptr, cursor: *mut Ptr) -> Ptr {
        assert_eq!(owner as usize, 0x1000);
        // SAFETY: fields() supplies its own initialized metadata iterator slot.
        unsafe {
            let index = *cursor as usize; *cursor = (index + 1) as Ptr;
            match MODE.with(|mode| mode.get()) {
                1 => 0x2000 as Ptr,
                3 => (0x2000 + index * 8) as Ptr,
                _ if index == 0 => 0x2000 as Ptr,
                _ => std::ptr::null_mut(),
            }
        }
    }
    extern "C-unwind" fn parent(_: Ptr) -> Ptr {
        if MODE.with(|mode| mode.get()) == 2 { 0x1001 as Ptr } else { 0x1000 as Ptr }
    }
    extern "C-unwind" fn name(_: Ptr) -> *const c_char { c"DeclaredField".as_ptr() }
    extern "C" fn exports(_: Ptr, symbol: HostStr<'_>) -> usize {
        match symbol.to_cstr().unwrap().to_bytes() {
            b"il2cpp_class_get_fields" => fields as usize,
            b"il2cpp_field_get_parent" => parent as usize,
            b"il2cpp_field_get_name" => name as usize,
            _ => 0,
        }
    }
    // SAFETY: every fixture symbol has its standard metadata-only prototype.
    let api = unsafe { Il2cpp::bind(BASE, exports, &[]) }.unwrap();
    let owner = Class::new(0x1000 as Ptr, BASE).unwrap();
    let field = api.fields(owner).unwrap()[0];
    assert_eq!(api.field_name(field).as_deref(), Some("DeclaredField"));
    assert!(api.fields(Class::new(owner.as_ptr(), BASE + 1).unwrap()).is_none());
    assert!(api.field_name(Field::new(field.as_ptr(), BASE + 1).unwrap()).is_none());
    for mode in [1, 2, 3] {
        MODE.with(|value| value.set(mode));
        assert!(api.fields(owner).is_none(), "repeated, wrong-owner and nonterminating cursors reject");
    }
    MODE.with(|value| value.set(0));
    assert!(bound().fields(owner).is_none(), "missing optional exports do not return a partial list");
    assert!(bound().field_name(field).is_none());
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
    assert_eq!(api.unity_version(screen), Err("Unity version requires UnityEngine.Application"));
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
        Err("Unity version requires UnityEngine.Application")));
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

// A small managed heap behind real typed export calls. In particular the
// constructor consumes a value-address or a direct object pointer, so swapping
// those two runtime_invoke conventions fails here rather than merely comparing
// the wrapper's own argument descriptions.
mod managed_tools {
    use super::*;

    const REF: usize = 0x1100;
    const VALUE: usize = 0x1200;
    // A different assembly's value class may share the same FullName.
    const SAME_NAME_VALUE: usize = 0x1600;
    const OTHER: usize = 0x1300;
    const VOID: usize = 0x1400;
    const VALUE_CTOR: usize = 0x2100;
    const REF_CTOR: usize = 0x2200;
    const GETTER: usize = 0x2300;
    const VALUE_INSTANCE: usize = 0x2400;
    const VALUE_FIELD: usize = 0x3100;
    const REF_FIELD: usize = 0x3200;
    const STATIC_FIELD: usize = 0x3300;
    const READONLY_FIELD: usize = 0x3400;

    struct Object { class: usize, value: u64, reference: usize }
    struct Heap {
        objects: Vec<Box<Object>>, roots: Vec<(usize, usize)>, next: usize,
        freed: usize, calls: usize, class_calls: usize, static_ref: usize,
        names_new: usize, names_free: usize, fail: bool, repeat: bool,
        value_receiver_box: usize, value_receiver_payload: usize,
    }
    static HEAP: Mutex<Heap> = Mutex::new(Heap { objects: Vec::new(), roots: Vec::new(), next: 0,
        freed: 0, calls: 0, class_calls: 0, static_ref: 0,
        names_new: 0, names_free: 0, fail: false, repeat: false,
        value_receiver_box: 0, value_receiver_payload: 0 });

    fn class(id: usize, base: usize) -> Class { Class::new(id as Ptr, base).unwrap() }
    fn ty(id: usize, base: usize) -> Type { Type::new(id as Ptr, base).unwrap() }
    fn field(id: usize, base: usize) -> Field { Field::new(id as Ptr, base).unwrap() }
    extern "C-unwind" fn type_kind(t: Ptr) -> i32 {
        match t as usize { REF | OTHER => 0x12, VALUE | SAME_NAME_VALUE => 0x0a, VOID => TYPE_VOID, _ => panic!("foreign type") }
    }
    extern "C-unwind" fn type_name(t: Ptr) -> *mut c_char {
        HEAP.lock().unwrap().names_new += 1;
        CString::new(match t as usize { REF => "Fixture.Ref".to_owned(), VALUE | SAME_NAME_VALUE => "System.Int64".to_owned(),
            OTHER => "Fixture.Other".to_owned(), VOID => "System.Void".to_owned(),
            0x1500 => "x".repeat(1025), _ => panic!("foreign type") }).unwrap().into_raw()
    }
    extern "C-unwind" fn free_name(p: Ptr) {
        // SAFETY: only type_name allocations enter the bound il2cpp_free export.
        drop(unsafe { CString::from_raw(p.cast()) });
        HEAP.lock().unwrap().names_free += 1;
    }
    extern "C-unwind" fn no_byref(_: Ptr) -> bool { false }
    extern "C-unwind" fn class_from_type(t: Ptr) -> Ptr { t }
    extern "C-unwind" fn value_type(c: Ptr) -> bool { matches!(c as usize, VALUE | SAME_NAME_VALUE) }
    extern "C-unwind" fn assignable(a: Ptr, b: Ptr) -> bool { a == b }
    extern "C-unwind" fn value_size(c: Ptr, align: *mut u32) -> i32 {
        assert!(matches!(c as usize, VALUE | SAME_NAME_VALUE));
        // SAFETY: value_layout supplies its live initialized output slot.
        unsafe { *align = 8; }
        8
    }
    extern "C-unwind" fn methods(c: Ptr, cursor: *mut Ptr) -> Ptr {
        // SAFETY: methods supplies its own live cursor slot.
        let index = unsafe { *cursor as usize };
        // SAFETY: as above.
        unsafe { *cursor = (index + 1) as Ptr; }
        if c as usize == VALUE {
            return if index == 0 { VALUE_INSTANCE as Ptr } else { std::ptr::null_mut() };
        }
        assert_eq!(c as usize, REF);
        match index { 0 => VALUE_CTOR as Ptr, 1 if HEAP.lock().unwrap().repeat => VALUE_CTOR as Ptr,
            1 => REF_CTOR as Ptr, 2 => GETTER as Ptr, _ => std::ptr::null_mut() }
    }
    extern "C-unwind" fn method_name(m: Ptr) -> *const c_char {
        match m as usize { GETTER => c"Get".as_ptr(), VALUE_INSTANCE => c"GetValue".as_ptr(), _ => c".ctor".as_ptr() }
    }
    extern "C-unwind" fn method_class(m: Ptr) -> Ptr {
        if m as usize == VALUE_INSTANCE { VALUE as Ptr } else { REF as Ptr }
    }
    extern "C-unwind" fn flags(m: Ptr, implementation: *mut u32) -> u32 {
        // SAFETY: the binding provides its live metadata output slot.
        unsafe { *implementation = 0; }
        if m as usize == GETTER { METHOD_STATIC } else { 0 }
    }
    extern "C-unwind" fn count(m: Ptr) -> u32 { if matches!(m as usize, GETTER | VALUE_INSTANCE) { 0 } else { 1 } }
    extern "C-unwind" fn param(m: Ptr, index: u32) -> Ptr {
        assert_eq!(index, 0);
        if m as usize == VALUE_CTOR { VALUE as Ptr } else { REF as Ptr }
    }
    extern "C-unwind" fn returns(m: Ptr) -> Ptr {
        match m as usize { GETTER => REF as Ptr, VALUE_INSTANCE => VALUE as Ptr, _ => VOID as Ptr }
    }
    extern "C-unwind" fn object_class(p: Ptr) -> Ptr {
        HEAP.lock().unwrap().class_calls += 1;
        // SAFETY: helpers only pass live pointers belonging to this fixture heap.
        unsafe { (*(p.cast::<Object>())).class as Ptr }
    }
    extern "C-unwind" fn object_new(c: Ptr) -> Ptr {
        let mut object = Box::new(Object { class: c as usize, value: 0, reference: 0 });
        let p = (&mut *object as *mut Object).cast();
        HEAP.lock().unwrap().objects.push(object); p
    }
    extern "C-unwind" fn value_box(c: Ptr, value: Ptr) -> Ptr {
        assert_eq!(c as usize, VALUE); assert_eq!(value as usize % 8, 0);
        let p = object_new(c);
        // SAFETY: box_value supplies exactly eight initialized aligned bytes.
        unsafe { (*p.cast::<Object>()).value = value.cast::<u64>().read(); } p
    }
    extern "C-unwind" fn unbox(p: Ptr) -> Ptr {
        // SAFETY: the adapter checked this live box's exact value class.
        unsafe { std::ptr::addr_of_mut!((*p.cast::<Object>()).value).cast() }
    }
    extern "C-unwind" fn new_root(p: Ptr, pinned: bool) -> usize {
        assert!(!pinned); let mut heap = HEAP.lock().unwrap(); heap.next += 1;
        let id = heap.next; heap.roots.push((id, p as usize)); id
    }
    extern "C-unwind" fn target(id: usize) -> Ptr {
        HEAP.lock().unwrap().roots.iter().find(|r| r.0 == id).unwrap().1 as Ptr
    }
    extern "C-unwind" fn free_root(id: usize) {
        let mut heap = HEAP.lock().unwrap(); let index = heap.roots.iter().position(|r| r.0 == id).unwrap();
        heap.roots.remove(index); heap.freed += 1;
    }
    extern "C-unwind" fn field_parent(_: Ptr) -> Ptr { REF as Ptr }
    extern "C-unwind" fn field_type(f: Ptr) -> Ptr { if f as usize == VALUE_FIELD { VALUE as Ptr } else { REF as Ptr } }
    extern "C-unwind" fn field_flags(f: Ptr) -> i32 {
        match f as usize { STATIC_FIELD => 0x10, READONLY_FIELD => 0x20, _ => 0 }
    }
    extern "C-unwind" fn get_field(p: Ptr, f: Ptr, out: Ptr) {
        // SAFETY: typed metadata and rooted receiver select the exact slot.
        unsafe { if f as usize == VALUE_FIELD { out.cast::<u64>().write((*p.cast::<Object>()).value); }
            else { out.cast::<Ptr>().write((*p.cast::<Object>()).reference as Ptr); } }
    }
    extern "C-unwind" fn get_static(f: Ptr, out: Ptr) {
        assert_eq!(f as usize, STATIC_FIELD);
        // SAFETY: static_reference_field supplies a reference-sized output slot.
        unsafe { out.cast::<Ptr>().write(HEAP.lock().unwrap().static_ref as Ptr); }
    }
    extern "C-unwind" fn set_reference(p: Ptr, f: Ptr, value: Ptr) {
        assert_eq!(f as usize, REF_FIELD);
        // SAFETY: helper checked both roots and the field's reference ABI.
        unsafe { (*p.cast::<Object>()).reference = value as usize; }
    }
    extern "C-unwind" fn invoke(m: Ptr, object: Ptr, args: *mut Ptr, exception: *mut Ptr) -> Ptr {
        let mut heap = HEAP.lock().unwrap(); heap.calls += 1;
        if heap.fail {
            // SAFETY: runtime_invoke supplies its initialized exception slot.
            unsafe { *exception = 0xdead as Ptr; } return std::ptr::null_mut();
        }
        // SAFETY: helpers validate the receiver and provide a live argument list.
        unsafe { match m as usize {
            VALUE_CTOR => { assert_eq!(*args as usize % 8, 0); (*object.cast::<Object>()).value = (*args).cast::<u64>().read(); }
            REF_CTOR => { assert_eq!((*(*args).cast::<Object>()).class, REF); (*object.cast::<Object>()).reference = *args as usize; }
            GETTER => return heap.static_ref as Ptr,
            VALUE_INSTANCE => {
                assert_eq!(object as usize, heap.value_receiver_payload,
                    "value-type instance method receives the payload address");
                assert_ne!(object as usize, heap.value_receiver_box,
                    "passing the managed box would have a different ABI");
                assert_eq!(object.cast::<u64>().read(), 42);
                return heap.value_receiver_box as Ptr;
            }
            _ => panic!("wrong managed method"),
        } }
        std::ptr::null_mut()
    }
    extern "C" fn symbols(_: Ptr, name: HostStr<'_>) -> usize {
        match name.to_cstr().unwrap().to_bytes() {
            b"il2cpp_class_from_il2cpp_type" => class_from_type as usize,
            b"il2cpp_class_is_valuetype" => value_type as usize,
            b"il2cpp_class_is_assignable_from" => assignable as usize,
            b"il2cpp_class_value_size" => value_size as usize,
            b"il2cpp_type_get_name" => type_name as usize, b"il2cpp_free" => free_name as usize,
            b"il2cpp_type_get_type" => type_kind as usize, b"il2cpp_type_is_byref" => no_byref as usize,
            b"il2cpp_class_get_methods" => methods as usize, b"il2cpp_method_get_name" => method_name as usize,
            b"il2cpp_method_is_generic" | b"il2cpp_method_is_inflated" => ordinary_flag as usize,
            b"il2cpp_method_get_class" => method_class as usize, b"il2cpp_method_get_flags" => flags as usize,
            b"il2cpp_method_get_param_count" => count as usize, b"il2cpp_method_get_param" => param as usize,
            b"il2cpp_method_get_return_type" => returns as usize, b"il2cpp_object_get_class" => object_class as usize,
            b"il2cpp_object_new" => object_new as usize, b"il2cpp_value_box" => value_box as usize,
            b"il2cpp_object_unbox" => unbox as usize, b"il2cpp_runtime_invoke" => invoke as usize,
            b"il2cpp_gchandle_new" => new_root as usize, b"il2cpp_gchandle_get_target" => target as usize,
            b"il2cpp_gchandle_free" => free_root as usize, b"il2cpp_thread_current" => super::thread_current as usize,
            b"il2cpp_field_get_parent" => field_parent as usize, b"il2cpp_field_get_type" => field_type as usize,
            b"il2cpp_field_get_flags" => field_flags as usize, b"il2cpp_field_get_value" => get_field as usize,
            b"il2cpp_field_static_get_value" => get_static as usize,
            b"il2cpp_field_set_value_object" => set_reference as usize, _ => 0,
        }
    }

    #[test]
    fn checked_managed_operations_preserve_abi_roots_and_reject_before_calls() {
        // SAFETY: the static fixture implements every named standard prototype.
        let api = unsafe { Il2cpp::bind(BASE, symbols, &[]) }.unwrap();
        // SAFETY: these fixture exports deliberately use the pointer-width ABI.
        let gc = unsafe { api.gc_handles(GcHandleAbi::PointerWidth) }.unwrap();
        let c = class(REF, BASE); let v = class(VALUE, BASE); let vt = ty(VALUE, BASE);
        assert_eq!(api.class_from_type(vt), Some(v), "standard alias is sufficient");
        assert_eq!(api.type_name(ty(0x1500, BASE)), None, "overlong name is freed");
        let ctor = api.matching_checked_method(c, c".ctor", false, &["System.Int64"], "System.Void").unwrap();
        assert_eq!(ctor.metadata().as_ptr() as usize, VALUE_CTOR);
        assert!(api.matching_checked_method(c, c".ctor", true, &["System.Int64"], "System.Void").is_none());
        HEAP.lock().unwrap().repeat = true;
        assert!(api.methods(c).is_none(), "partial/repeated inventory cannot pick an overload");
        HEAP.lock().unwrap().repeat = false;
        // SAFETY: fixture objects contain initialized scalars/references only.
        unsafe {
            let object = api.allocate_object(c, &gc).unwrap();
            let child = api.allocate_object(c, &gc).unwrap();
            let value = api.box_value(v, &42u64.to_ne_bytes(), &gc).unwrap();
            assert_eq!(api.copy_unboxed(&value, v).unwrap(), 42u64.to_ne_bytes());
            let boxed_pointer = value.target();
            let payload_pointer = std::ptr::addr_of_mut!((*boxed_pointer.cast::<Object>()).value) as usize;
            {
                let mut heap = HEAP.lock().unwrap();
                heap.value_receiver_box = boxed_pointer as usize;
                heap.value_receiver_payload = payload_pointer;
            }
            let value_instance = api.matching_checked_method(v, c"GetValue", false, &[], "System.Int64").unwrap();
            let returned_value = api.invoke_checked(&value_instance, Some(&value), &[], &gc).unwrap().unwrap();
            assert_eq!(api.copy_unboxed(&returned_value, v).unwrap(), 42u64.to_ne_bytes());
            assert!(api.invoke_checked(&ctor, Some(&object), &[ManagedArg::BoxedValue(&value)], &gc).unwrap().is_none());
            assert_eq!(api.value_field(&object, field(VALUE_FIELD, BASE)).unwrap(), 42u64.to_ne_bytes());
            api.invoke_checked(&ctor, Some(&object), &[ManagedArg::Value(vt, &99u64.to_ne_bytes())], &gc).unwrap();
            assert_eq!(api.value_field(&object, field(VALUE_FIELD, BASE)).unwrap(), 99u64.to_ne_bytes());
            let reference_ctor = api.matching_checked_method(c, c".ctor", false, &["Fixture.Ref"], "System.Void").unwrap();
            api.invoke_checked(&reference_ctor, Some(&object), &[ManagedArg::Reference(Some(&child))], &gc).unwrap();
            assert_eq!(api.read_reference_field(&object, field(REF_FIELD, BASE), &gc).unwrap().unwrap().target(), child.target());
            api.write_reference_field(&object, field(REF_FIELD, BASE), None).unwrap();
            assert!(api.read_reference_field(&object, field(REF_FIELD, BASE), &gc).unwrap().is_none());
            assert!(api.write_reference_field(&object, field(READONLY_FIELD, BASE), Some(&child)).is_err());
            let child_pointer = child.target() as usize;
            HEAP.lock().unwrap().static_ref = child_pointer;
            assert_eq!(api.static_reference_field(field(STATIC_FIELD, BASE), &gc).unwrap().unwrap().target(), child.target());
            let getter = api.matching_checked_method(c, c"Get", true, &[], "Fixture.Ref").unwrap();
            assert_eq!(api.invoke_checked(&getter, None, &[], &gc).unwrap().unwrap().target(), child.target());
            let before = HEAP.lock().unwrap().calls;
            let same_name = ty(SAME_NAME_VALUE, BASE);
            assert_eq!(api.type_name(same_name), api.type_name(vt));
            assert_ne!(api.class_from_type(same_name), api.class_from_type(vt));
            assert!(api.invoke_checked(&ctor, Some(&object), &[ManagedArg::Value(same_name, &[0; 8])], &gc).is_err());
            assert!(api.invoke_checked(&ctor, None, &[ManagedArg::Value(vt, &[0; 8])], &gc).is_err());
            assert!(api.invoke_checked(&ctor, Some(&object), &[ManagedArg::Value(vt, &[0; 7])], &gc).is_err());
            assert!(api.invoke_checked(&ctor, Some(&object), &[ManagedArg::BoxedValue(&child)], &gc).is_err());
            assert_eq!(HEAP.lock().unwrap().calls, before);
            let other = api.allocate_object(class(OTHER, BASE), &gc).unwrap();
            assert!(api.write_reference_field(&object, field(REF_FIELD, BASE), Some(&other)).is_err());
            assert!(api.value_field(&other, field(VALUE_FIELD, BASE)).is_err());
            let foreign = Il2cpp::bind(BASE + 4, symbols, &[]).unwrap();
            let foreign_gc = foreign.gc_handles(GcHandleAbi::PointerWidth).unwrap();
            let foreign_object = foreign.allocate_object(class(REF, BASE + 4), &foreign_gc).unwrap();
            let class_calls = HEAP.lock().unwrap().class_calls;
            assert!(api.read_reference_field(&foreign_object, field(REF_FIELD, BASE), &gc).is_err());
            assert!(api.allocate_object(class(REF, BASE + 4), &gc).is_err());
            assert!(api.static_reference_field(field(STATIC_FIELD, BASE), &foreign_gc).is_err());
            assert_eq!(HEAP.lock().unwrap().class_calls, class_calls, "foreign roots reject before native object access");
            let roots = HEAP.lock().unwrap().roots.len();
            HEAP.lock().unwrap().fail = true;
            assert!(api.invoke_checked(&ctor, Some(&object), &[ManagedArg::BoxedValue(&value)], &gc).is_err());
            HEAP.lock().unwrap().fail = false;
            assert_eq!(HEAP.lock().unwrap().roots.len(), roots, "managed exception creates no result root");
        }
        let mut heap = HEAP.lock().unwrap();
        assert!(heap.roots.is_empty()); assert!(heap.freed > 0);
        assert_eq!(heap.names_new, heap.names_free);
        heap.objects.clear();
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
