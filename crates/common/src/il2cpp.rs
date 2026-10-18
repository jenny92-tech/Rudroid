//! INPUT:  a mapped IL2CPP module (load base from a module event) and the loader's guest symbol lookup
//! OUTPUT:  `Il2cpp`: bound exports, typed metadata, paired static i32 methods and GC roots
//! POS:    the single place IL2CPP exports are transmuted; titles and memory policy share its domain/class/method lookups
//!
//! Safety model: [`Il2cpp::bind`] is the one unsafe step — the caller vouches
//! that `base` is a live, never-unmapped IL2CPP module whose exports have the
//! standard IL2CPP ABI. Metadata (images, classes, methods, fields, types) is
//! immutable and lives for the process, and handle types can only be built
//! from this binding's own results. Each handle records the module base;
//! another binding rejects it before calling guest code. Metadata methods are safe. Anything
//! touching a managed object (invoke, unbox, string contents, new GC roots)
//! stays `unsafe fn`: objects move under the GC's control.
//!
//! Every export is typed `extern "C-unwind"`, so a guest C++ exception may
//! cross a Rust hook that also permits unwinding. Plain `extern "C"` registry
//! callbacks still require that no guest exception escapes them.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::NonNull;

use crate::host_api::SoSymbolFn;
use crate::il2cpp_abi::GcHandleAbi;

type Ptr = *mut c_void;

macro_rules! handles {
    ($($(#[$doc:meta])* $name:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(NonNull<c_void>, usize);

        impl $name {
            fn new(raw: Ptr, base: usize) -> Option<Self> {
                NonNull::new(raw).map(|raw| Self(raw, base))
            }

            pub fn as_ptr(self) -> Ptr {
                self.0.as_ptr()
            }
        }

        // SAFETY: IL2CPP metadata is immutable and process-lifetime.
        unsafe impl Send for $name {}
        // SAFETY: as above.
        unsafe impl Sync for $name {}
    )*};
}

handles! {
    /// `const Il2CppImage*`.
    Image;
    /// `Il2CppClass*`.
    Class;
    /// `const MethodInfo*`.
    Method;
    /// `FieldInfo*`.
    Field;
    /// `const Il2CppType*`.
    Type;
}

/// A verified static `int32_t Method(const MethodInfo*)` entry, paired with
/// its immutable metadata. Safe code cannot substitute either pointer.
#[derive(Clone, Copy)]
pub struct StaticI32Method {
    entry: unsafe extern "C-unwind" fn(Ptr) -> i32,
    method: Method,
}

impl StaticI32Method {
    /// # Safety
    /// The caller must satisfy the method's execution preconditions, including
    /// Unity's thread requirements. A guest exception may unwind this call.
    pub unsafe fn call(self) -> i32 {
        // SAFETY: the binding verified the ABI and keeps the code/metadata
        // mapped; the caller supplies the method-specific execution context.
        unsafe { (self.entry)(self.method.as_ptr()) }
    }
}

macro_rules! exports {
    ($($field:ident = $name:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// Every export name this binding knows, for required-export lists.
        pub const EXPORTS: &[&str] = &[$($name),*];

        #[derive(Clone, Copy, Default)]
        struct Exports {
            $($field: Option<unsafe extern "C-unwind" fn($($arg),*) $(-> $ret)?>,)*
        }

        impl Exports {
            /// # Safety
            /// `resolve` returns 0 or the address of the named standard export.
            unsafe fn resolve(resolve: impl Fn(&CStr) -> usize) -> Self {
                Self {
                    $($field: {
                        let address = resolve(&CString::new($name).unwrap());
                        // SAFETY: caller contract — the named export with this signature.
                        (address != 0).then(|| unsafe {
                            std::mem::transmute::<usize, unsafe extern "C-unwind" fn($($arg),*) $(-> $ret)?>(address)
                        })
                    },)*
                }
            }

            fn bound(&self, name: &str) -> bool {
                match name {
                    $($name => self.$field.is_some(),)*
                    _ => false,
                }
            }
        }
    };
}

exports! {
    domain_get = "il2cpp_domain_get": fn() -> Ptr;
    domain_get_assemblies = "il2cpp_domain_get_assemblies": fn(Ptr, *mut usize) -> *const Ptr;
    assembly_get_image = "il2cpp_assembly_get_image": fn(Ptr) -> Ptr;
    image_get_name = "il2cpp_image_get_name": fn(Ptr) -> *const c_char;
    image_get_class_count = "il2cpp_image_get_class_count": fn(Ptr) -> usize;
    image_get_class = "il2cpp_image_get_class": fn(Ptr, usize) -> Ptr;
    class_from_name = "il2cpp_class_from_name": fn(Ptr, *const c_char, *const c_char) -> Ptr;
    class_get_method_from_name = "il2cpp_class_get_method_from_name": fn(Ptr, *const c_char, i32) -> Ptr;
    class_get_methods = "il2cpp_class_get_methods": fn(Ptr, *mut Ptr) -> Ptr;
    class_get_nested_types = "il2cpp_class_get_nested_types": fn(Ptr, *mut Ptr) -> Ptr;
    class_get_fields = "il2cpp_class_get_fields": fn(Ptr, *mut Ptr) -> Ptr;
    class_get_parent = "il2cpp_class_get_parent": fn(Ptr) -> Ptr;
    method_get_name = "il2cpp_method_get_name": fn(Ptr) -> *const c_char;
    class_get_field_from_name = "il2cpp_class_get_field_from_name": fn(Ptr, *const c_char) -> Ptr;
    class_get_type = "il2cpp_class_get_type": fn(Ptr) -> Ptr;
    class_get_name = "il2cpp_class_get_name": fn(Ptr) -> *const c_char;
    class_get_namespace = "il2cpp_class_get_namespace": fn(Ptr) -> *const c_char;
    class_from_type = "il2cpp_class_from_type": fn(Ptr) -> Ptr;
    class_enum_basetype = "il2cpp_class_enum_basetype": fn(Ptr) -> Ptr;
    field_get_offset = "il2cpp_field_get_offset": fn(Ptr) -> usize;
    field_get_type = "il2cpp_field_get_type": fn(Ptr) -> Ptr;
    field_get_flags = "il2cpp_field_get_flags": fn(Ptr) -> i32;
    field_get_value = "il2cpp_field_get_value": fn(Ptr, Ptr, Ptr);
    field_set_value = "il2cpp_field_set_value": fn(Ptr, Ptr, Ptr);
    field_get_parent = "il2cpp_field_get_parent": fn(Ptr) -> Ptr;
    method_is_generic = "il2cpp_method_is_generic": fn(Ptr) -> bool;
    method_is_inflated = "il2cpp_method_is_inflated": fn(Ptr) -> bool;
    type_get_type = "il2cpp_type_get_type": fn(Ptr) -> i32;
    type_is_byref = "il2cpp_type_is_byref": fn(Ptr) -> bool;
    type_get_object = "il2cpp_type_get_object": fn(Ptr) -> Ptr;
    method_get_flags = "il2cpp_method_get_flags": fn(Ptr, *mut u32) -> u32;
    method_get_param_count = "il2cpp_method_get_param_count": fn(Ptr) -> u32;
    method_get_param = "il2cpp_method_get_param": fn(Ptr, u32) -> Ptr;
    method_get_return_type = "il2cpp_method_get_return_type": fn(Ptr) -> Ptr;
    object_get_class = "il2cpp_object_get_class": fn(Ptr) -> Ptr;
    object_unbox = "il2cpp_object_unbox": fn(Ptr) -> Ptr;
    runtime_invoke = "il2cpp_runtime_invoke": fn(Ptr, Ptr, *mut Ptr, *mut Ptr) -> Ptr;
    string_new = "il2cpp_string_new": fn(*const c_char) -> Ptr;
    string_length = "il2cpp_string_length": fn(Ptr) -> i32;
    string_chars = "il2cpp_string_chars": fn(Ptr) -> *const u16;
    thread_current = "il2cpp_thread_current": fn() -> Ptr;
    // Native pinned parameter is C/C++ bool, not a JNI jboolean/u8.
    gchandle_new = "il2cpp_gchandle_new": fn(Ptr, bool) -> usize;
    gchandle_get_target = "il2cpp_gchandle_get_target": fn(usize) -> Ptr;
    gchandle_free = "il2cpp_gchandle_free": fn(usize);
    gc_collect = "il2cpp_gc_collect": fn(i32);
    gc_get_used_size = "il2cpp_gc_get_used_size": fn() -> i64;
    gc_get_heap_size = "il2cpp_gc_get_heap_size": fn() -> i64;
}

/// Why [`Il2cpp::inventory`] has no images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inventory {
    /// `il2cpp_domain_get` is missing or returned null.
    NoDomain,
    /// The assembly list is missing, null or implausibly long.
    InvalidAssemblies,
}

/// `METHOD_ATTRIBUTE_STATIC`.
pub const METHOD_STATIC: u32 = 0x10;
/// `Il2CppTypeEnum` values the kit checks.
pub const TYPE_VOID: i32 = 0x01;
pub const TYPE_BOOLEAN: i32 = 0x02;
pub const TYPE_STRING: i32 = 0x0e;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumericValue { I32(i32), F32(f32), Bool(bool) }

/// The bound IL2CPP API of one live module.
#[derive(Clone, Copy)]
pub struct Il2cpp {
    base: usize,
    f: Exports,
}

/// Call a bound export that the methods below already checked is present.
macro_rules! call {
    ($self:ident.$export:ident($($arg:expr),*)) => {
        // SAFETY: `Il2cpp::bind` contract (live module, standard ABI); the
        // arguments are handles from this binding or valid locals.
        $self.f.$export.map(|function| unsafe { function($($arg),*) })
    };
}

impl Il2cpp {
    /// Bind every known export of the module at `base`; fails with the exact
    /// list of `required` names that did not resolve.
    ///
    /// # Safety
    /// `base` uniquely identifies a live IL2CPP module that is never unmapped.
    /// `so_symbol` returns zero or its named standard export with the declared
    /// ABI; metadata and metadata strings remain immutable for the process.
    /// Compiled method entries use the standard generated IL2CPP ABI, with
    /// their final `MethodInfo*` argument, and remain mapped for the process.
    /// GC-handle exports are called only after selecting their exact ABI in
    /// [`Self::gc_handles`].
    pub unsafe fn bind(base: usize, so_symbol: SoSymbolFn, required: &[&str]) -> Result<Self, Vec<String>> {
        let resolve = |name: &CStr| so_symbol(base as Ptr, crate::registry::HostStr::new(name));
        let missing: Vec<String> = required
            .iter()
            .filter(|name| CString::new(**name).map_or(true, |name| resolve(&name) == 0))
            .map(|name| (*name).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(missing);
        }
        // SAFETY: caller contract — standard exports of a live module.
        Ok(Self { base, f: unsafe { Exports::resolve(resolve) } })
    }

    /// Load base of the bound module (the module event's `base`).
    pub fn base(&self) -> usize {
        self.base
    }

    /// Whether the export `name` (e.g. `"il2cpp_gc_collect"`) is bound.
    pub fn has(&self, name: &str) -> bool {
        self.f.bound(name)
    }

    /// Images of every assembly in the current domain, in domain order;
    /// empty when the inventory is unavailable.
    pub fn images(&self) -> Vec<Image> {
        self.inventory().unwrap_or_default()
    }

    /// [`Self::images`] that tells why the inventory is unavailable.
    pub fn inventory(&self) -> Result<Vec<Image>, Inventory> {
        let Some(domain) = call!(self.domain_get()).filter(|d| !d.is_null()) else {
            return Err(Inventory::NoDomain);
        };
        let mut count = 0usize;
        let assemblies = call!(self.domain_get_assemblies(domain, &mut count))
            .filter(|assemblies| !assemblies.is_null() && count <= 65_536)
            .ok_or(Inventory::InvalidAssemblies)?;
        // SAFETY: IL2CPP returns `count` assembly pointers owned by the domain.
        let assemblies = unsafe { std::slice::from_raw_parts(assemblies, count) };
        Ok(assemblies
            .iter()
            .filter_map(|&assembly| call!(self.assembly_get_image(assembly)).and_then(|raw| Image::new(raw, self.base)))
            .collect())
    }

    pub fn image_name(&self, image: Image) -> Option<String> {
        if image.1 != self.base { return None; }
        call!(self.image_get_name(image.as_ptr())).and_then(|name| owned(name))
    }

    /// First image whose name contains `want` (e.g. `Assembly-CSharp`).
    pub fn find_image(&self, want: &str) -> Option<Image> {
        self.images().into_iter().find(|&image| self.image_name(image).is_some_and(|name| name.contains(want)))
    }

    /// Exact assembly identity for automatic configuration; ambiguous matches fail closed.
    pub fn exact_image(&self,want:&str)->Option<Image>{
        let want=want.strip_suffix(".dll").unwrap_or(want);
        let mut images=self.images().into_iter().filter(|&image|self.image_name(image)
            .is_some_and(|name|name.strip_suffix(".dll").unwrap_or(&name)==want));
        let first=images.next()?;if images.next().is_some(){None}else{Some(first)}
    }

    pub fn class_from_name(&self, image: Image, namespace: &CStr, name: &CStr) -> Option<Class> {
        if image.1 != self.base { return None; }
        call!(self.class_from_name(image.as_ptr(), namespace.as_ptr(), name.as_ptr())).and_then(|raw| Class::new(raw, self.base))
    }

    /// `namespace.name` from the first image that defines it.
    pub fn find_class(&self, namespace: &CStr, name: &CStr) -> Option<Class> {
        self.images().into_iter().find_map(|image| self.class_from_name(image, namespace, name))
    }

    /// First class named `name` in any namespace, walking every image's class list.
    pub fn find_class_named(&self, name: &CStr) -> Option<Class> {
        self.images().into_iter().find_map(|image| {
            let count = call!(self.image_get_class_count(image.as_ptr()))?;
            (0..count).find_map(|index| {
                let class = call!(self.image_get_class(image.as_ptr(), index)).and_then(|raw| Class::new(raw, self.base))?;
                let class_name = call!(self.class_get_name(class.as_ptr()))?;
                // SAFETY: IL2CPP class names are NUL-terminated metadata strings.
                (!class_name.is_null() && unsafe { CStr::from_ptr(class_name) } == name).then_some(class)
            })
        })
    }

    pub fn class_name(&self, class: Class) -> Option<String> {
        if class.1 != self.base { return None; }
        call!(self.class_get_name(class.as_ptr())).and_then(|name| owned(name))
    }

    pub fn class_namespace(&self, class: Class) -> Option<String> {
        if class.1 != self.base { return None; }
        call!(self.class_get_namespace(class.as_ptr())).and_then(|name| owned(name))
    }

    pub fn class_type(&self, class: Class) -> Option<Type> {
        if class.1 != self.base { return None; }
        call!(self.class_get_type(class.as_ptr())).and_then(|raw| Type::new(raw, self.base))
    }

    pub fn parent(&self, class: Class) -> Option<Class> {
        if class.1 != self.base { return None; }
        call!(self.class_get_parent(class.as_ptr())).and_then(|raw| Class::new(raw, self.base))
    }

    /// Snapshot a class's nested metadata types. No managed objects are created
    /// or retained. Reject missing exports, foreign handles and broken cursors.
    pub fn nested_classes(&self, class: Class) -> Option<Vec<Class>> {
        if class.1 != self.base { return None; }
        let mut cursor = std::ptr::null_mut();
        let mut classes = Vec::new();
        for _ in 0..4096 {
            let raw = call!(self.class_get_nested_types(class.as_ptr(), &mut cursor))?;
            if raw.is_null() { return Some(classes); }
            let nested = Class::new(raw, self.base)?;
            if classes.contains(&nested) { return None; }
            classes.push(nested);
        }
        None
    }

    /// Resolve an arity-only legacy request only when the overload is unique.
    pub fn unique_method(&self, class: Class, name: &CStr, argc: u32) -> Option<Method> {
        if class.1 != self.base { return None; }
        let mut cursor = std::ptr::null_mut();
        let mut found = None;
        for _ in 0..4096 {
            let raw = call!(self.class_get_methods(class.as_ptr(), &mut cursor))?;
            if raw.is_null() { return found; }
            let method = Method::new(raw, self.base)?;
            let raw_name = call!(self.method_get_name(method.as_ptr()))?;
            // SAFETY: a process-owned, immutable metadata name from the bound export.
            if !raw_name.is_null() && unsafe { CStr::from_ptr(raw_name) } == name
                && self.method_param_count(method) == Some(argc) {
                if found.is_some() { return None; }
                found = Some(method);
            }
        }
        None
    }

    /// Reject shared generic entries: another MethodInfo can have a different native ABI.
    pub fn ordinary_method(&self, method: Method) -> bool {
        method.1 == self.base && call!(self.method_is_generic(method.as_ptr())) == Some(false)
            && call!(self.method_is_inflated(method.as_ptr())) == Some(false)
    }

    /// Legacy offsets are selectors of real metadata fields, never addresses to write.
    pub fn field_at_offset(&self, mut class: Class, offset: usize) -> Option<Field> {
        if class.1 != self.base { return None; }
        let mut found = None;
        for _ in 0..128 {
            let mut cursor = std::ptr::null_mut();
            let mut finished = false;
            for _ in 0..4096 {
                let raw = call!(self.class_get_fields(class.as_ptr(), &mut cursor))?;
                if raw.is_null() { finished = true; break; }
                let field = Field::new(raw, self.base)?;
                if self.field_offset(field) == Some(offset) {
                    if found.is_some() { return None; }
                    found = Some(field);
                }
            }
            if !finished { return None; }
            match self.parent(class) { Some(parent) => class = parent, None => return found }
        }
        None
    }

    pub fn field_parent(&self, field: Field) -> Option<Class> {
        if field.1 != self.base { return None; }
        call!(self.field_get_parent(field.as_ptr())).and_then(|raw| Class::new(raw, self.base))
    }

    /// Read only an exactly typed instance primitive through IL2CPP's field accessor.
    /// # Safety
    /// `object` is a rooted instance of the declaring class on an attached thread.
    pub unsafe fn numeric_field(&self, object: Ptr, field: Field) -> Option<NumericValue> {
        if object.is_null() || field.1 != self.base || self.field_flags(field)? & 0x10 != 0 { return None; }
        let ty = self.field_type(field)?;
        if self.type_byref(ty) != Some(false) { return None; }
        let get = self.f.field_get_value?;
        macro_rules! read { ($t:ty,$variant:ident) => {{
            let mut value = 0 as $t;
            // SAFETY: metadata selects the exact initialized output width; caller roots the owner.
            unsafe { get(object, field.as_ptr(), (&mut value as *mut $t).cast()) };
            Some(NumericValue::$variant(value))
        }}; }
        match self.type_kind(ty)? {
            2 => {let mut value=0u8;
                // SAFETY: metadata selects exactly one initialized byte, interpreted without invalid Rust bool bits.
                unsafe {get(object,field.as_ptr(),(&mut value as *mut u8).cast())};
                Some(NumericValue::Bool(value!=0))},
            8 => read!(i32,I32), 12 => read!(f32,F32), _ => None
        }
    }

    /// Write only a mutable, exactly typed instance primitive through IL2CPP.
    /// # Safety
    /// `object` is a rooted instance of the declaring class on an attached thread.
    pub unsafe fn set_numeric_field(&self, object: Ptr, field: Field, value: NumericValue) -> bool {
        if object.is_null() || field.1 != self.base || self.field_flags(field).is_none_or(|f| f & 0x70 != 0) { return false; }
        let Some(ty) = self.field_type(field) else { return false; };
        if self.type_byref(ty) != Some(false) { return false; }
        let Some(set) = self.f.field_set_value else { return false; };
        macro_rules! write { ($value:ident,$kind:literal,$t:ty) => {{
            if self.type_kind(ty) != Some($kind) { return false; }
            // SAFETY: exact metadata width, mutable instance field and rooted caller-owned receiver.
            unsafe { set(object, field.as_ptr(), (&mut $value as *mut $t).cast()) };
            true
        }}; }
        match value { NumericValue::I32(mut value) => write!(value,8,i32), NumericValue::F32(mut value) => write!(value,12,f32),
            NumericValue::Bool(value)=>{let mut value=u8::from(value);write!(value,2,u8)} }
    }

    /// Bounded metadata walk; false for foreign classes or cyclic/invalid hierarchies.
    pub fn derives_from(&self, mut class: Class, base: Class) -> bool {
        if class.1 != self.base || base.1 != self.base { return false; }
        for _ in 0..128 {
            if class == base { return true; }
            let Some(parent) = call!(self.class_get_parent(class.as_ptr()))
                .and_then(|raw| Class::new(raw, self.base)) else { return false; };
            class = parent;
        }
        false
    }

    /// Select a non-byref overload by complete signature, rather than just arity.
    pub fn matching_method(&self, class: Class, name: &CStr, static_: bool,
        parameters: &[(i32, bool)], returns: i32) -> Option<Method> {
        if class.1 != self.base { return None; }
        let mut cursor = std::ptr::null_mut();
        for _ in 0..4096 {
            let method = call!(self.class_get_methods(class.as_ptr(), &mut cursor))
                .and_then(|raw| Method::new(raw, self.base))?;
            let raw = call!(self.method_get_name(method.as_ptr()))?;
            // SAFETY: the bound metadata API lends a terminated immutable method name.
            if !raw.is_null() && unsafe { CStr::from_ptr(raw) } == name
                && self.method_signature(method, static_, parameters, returns) { return Some(method); }
        }
        None
    }

    /// # Safety
    /// Attached thread; root the returned reflection object before further managed calls.
    pub unsafe fn type_object(&self, ty: Type) -> Option<Ptr> {
        if ty.1 != self.base { return None; }
        call!(self.type_get_object(ty.as_ptr())).filter(|raw| !raw.is_null())
    }

    /// # Safety
    /// `object` is a live, rooted object from this module on an attached thread.
    pub unsafe fn object_class(&self, object: Ptr) -> Option<Class> {
        if object.is_null() { return None; }
        call!(self.object_get_class(object)).and_then(|raw| Class::new(raw, self.base))
    }

    pub fn class_from_type(&self, ty: Type) -> Option<Class> {
        if ty.1 != self.base { return None; }
        call!(self.class_from_type(ty.as_ptr())).and_then(|raw| Class::new(raw, self.base))
    }

    /// Underlying metadata type of an enum; foreign classes are rejected.
    pub fn enum_base_type(&self, class: Class) -> Option<Type> {
        if class.1 != self.base { return None; }
        call!(self.class_enum_basetype(class.as_ptr())).and_then(|raw| Type::new(raw, self.base))
    }

    /// `il2cpp_class_get_method_from_name(class, name, argc)`; `argc = -1` matches any.
    pub fn method(&self, class: Class, name: &CStr, argc: i32) -> Option<Method> {
        if class.1 != self.base { return None; }
        call!(self.class_get_method_from_name(class.as_ptr(), name.as_ptr(), argc)).and_then(|raw| Method::new(raw, self.base))
    }

    /// The compiled entry (`MethodInfo::methodPointer`, its first word); `None` when 0.
    pub fn method_entry(&self, method: Method) -> Option<usize> {
        if method.1 != self.base { return None; }
        // SAFETY: every supported IL2CPP version starts MethodInfo with the
        // methodPointer word, and the MethodInfo lives for the process.
        let entry = unsafe { method.as_ptr().cast::<usize>().read() };
        (entry != 0).then_some(entry)
    }

    /// Bind a static, non-byref, no-argument i32 method without invoking it.
    /// The standard generated IL2CPP ABI includes the final `MethodInfo*`.
    /// Missing metadata, foreign handles and incompatible methods fail closed.
    pub fn static_i32_method(&self, method: Method) -> Option<StaticI32Method> {
        if !self.method_signature(method, true, &[], 8) { return None; }
        let entry = self.method_entry(method)?;
        Some(StaticI32Method {
            // SAFETY: bind's standard IL2CPP ABI contract plus the complete
            // scalar signature check above establish this native prototype.
            entry: unsafe { std::mem::transmute(entry as *const ()) },
            method,
        })
    }

    pub fn method_flags(&self, method: Method) -> Option<u32> {
        if method.1 != self.base { return None; }
        let mut implementation = 0;
        call!(self.method_get_flags(method.as_ptr(), &mut implementation))
    }

    pub fn method_param_count(&self, method: Method) -> Option<u32> {
        if method.1 != self.base { return None; }
        call!(self.method_get_param_count(method.as_ptr()))
    }

    pub fn method_param(&self, method: Method, index: u32) -> Option<Type> {
        if method.1 != self.base { return None; }
        if index >= self.method_param_count(method)? { return None; }
        call!(self.method_get_param(method.as_ptr(), index)).and_then(|raw| Type::new(raw, self.base))
    }

    pub fn method_return_type(&self, method: Method) -> Option<Type> {
        if method.1 != self.base { return None; }
        call!(self.method_get_return_type(method.as_ptr())).and_then(|raw| Type::new(raw, self.base))
    }

    /// True when `method` exists with exactly `argc` parameters and the given staticness.
    pub fn method_shape(&self, method: Method, argc: u32, is_static: bool) -> bool {
        self.method_flags(method).is_some_and(|flags| (flags & METHOD_STATIC != 0) == is_static)
            && self.method_param_count(method) == Some(argc)
    }

    /// Check the complete scalar/reference ABI before casting a managed method
    /// entry: staticness, count, return kind and every parameter's byref flag.
    /// Missing metadata fails closed.
    pub fn method_signature(&self, method: Method, is_static: bool, parameters: &[(i32, bool)], returns: i32) -> bool {
        let Ok(count) = u32::try_from(parameters.len()) else { return false; };
        if !self.method_shape(method, count, is_static) { return false; }
        let Some(ret) = self.method_return_type(method) else { return false; };
        self.type_kind(ret) == Some(returns) && self.type_byref(ret) == Some(false)
            && parameters.iter().enumerate().all(|(index, &(kind, byref))| {
                self.method_param(method, index as u32).is_some_and(|ty| {
                    self.type_kind(ty) == Some(kind) && self.type_byref(ty) == Some(byref)
                })
            })
    }

    pub fn type_byref(&self, ty: Type) -> Option<bool> {
        if ty.1 != self.base { return None; }
        call!(self.type_is_byref(ty.as_ptr()))
    }

    /// `Il2CppTypeEnum` of `ty`.
    pub fn type_kind(&self, ty: Type) -> Option<i32> {
        if ty.1 != self.base { return None; }
        call!(self.type_get_type(ty.as_ptr()))
    }

    pub fn field(&self, class: Class, name: &CStr) -> Option<Field> {
        if class.1 != self.base { return None; }
        call!(self.class_get_field_from_name(class.as_ptr(), name.as_ptr())).and_then(|raw| Field::new(raw, self.base))
    }

    pub fn field_offset(&self, field: Field) -> Option<usize> {
        if field.1 != self.base { return None; }
        call!(self.field_get_offset(field.as_ptr()))
    }

    pub fn field_type(&self, field: Field) -> Option<Type> {
        if field.1 != self.base { return None; }
        call!(self.field_get_type(field.as_ptr())).and_then(|raw| Type::new(raw, self.base))
    }

    pub fn field_flags(&self, field: Field) -> Option<u32> {
        if field.1 != self.base { return None; }
        call!(self.field_get_flags(field.as_ptr())).map(|flags| flags as u32)
    }

    /// Read an instance reference field through the runtime, without layout offsets.
    ///
    /// # Safety
    /// `object` is a live instance declaring `field`, kept reachable on the
    /// attached calling thread throughout this call. The returned reference
    /// must remain reachable before any subsequent managed allocation.
    pub unsafe fn reference_field(&self, object: Ptr, field: Field) -> Option<Ptr> {
        if object.is_null() || field.1 != self.base { return None; }
        let ty = self.field_type(field)?;
        if self.type_byref(ty) != Some(false) || !self.type_kind(ty).is_some_and(|k|matches!(k,0x0e|0x12|0x14|0x1c|0x1d))
            || self.field_flags(field)? & 0x10 != 0 { return None; }
        let get = self.f.field_get_value?;
        let mut value = std::ptr::null_mut::<c_void>();
        // SAFETY: the caller supplies the live instance; metadata above
        // guarantees a reference-sized instance field and initialized output.
        unsafe { get(object, field.as_ptr(), (&mut value as *mut Ptr).cast()) };
        Some(value)
    }

    /// Create a managed String on an attached thread.
    ///
    /// # Safety
    /// The calling thread is attached. Keep the returned reference reachable
    /// through its final managed use; never retain it unrooted across frames.
    pub unsafe fn new_string(&self, text: &CStr) -> Option<Ptr> {
        let new = self.f.string_new?;
        // SAFETY: text is a valid C string and the caller supplies thread context.
        let value = unsafe { new(text.as_ptr()) };
        (!value.is_null()).then_some(value)
    }

    /// Whether the calling thread is attached to the IL2CPP runtime.
    pub fn thread_attached(&self) -> bool {
        call!(self.thread_current()).is_some_and(|thread| !thread.is_null())
    }

    pub fn gc_collect(&self, generation: i32) -> bool {
        call!(self.gc_collect(generation)).is_some()
    }

    pub fn gc_used_size(&self) -> Option<i64> {
        call!(self.gc_get_used_size())
    }

    pub fn gc_heap_size(&self) -> Option<i64> {
        call!(self.gc_get_heap_size())
    }

    /// `Application.unityVersion` via its static getter, on an attached thread.
    pub fn unity_version(&self, application: Class) -> Result<String, &'static str> {
        if application.1 != self.base { return Err("Application belongs to another IL2CPP module"); }
        if !self.thread_attached() {
            return Err("Application metadata/attached IL2CPP thread unavailable");
        }
        let Some(method) = self.method(application, c"get_unityVersion", 0) else {
            return Err("Unity version getter unavailable");
        };
        if !self.method_shape(method, 0, true) {
            return Err("Unity version getter shape mismatch");
        }
        if !self.method_return_type(method).is_some_and(|ty| self.type_kind(ty) == Some(TYPE_STRING) && self.type_byref(ty) == Some(false)) {
            return Err("Unity version getter return type mismatch");
        }
        // SAFETY: a validated static, parameterless getter returning a string.
        let version = unsafe { self.invoke(method, std::ptr::null_mut(), std::ptr::null_mut()) };
        let version = version.ok().filter(|v| !v.is_null()).ok_or("Unity version query failed")?;
        // SAFETY: `version` is the live string the getter just returned; it
        // stays reachable from this attached native stack during the copy.
        let units = unsafe { self.string_utf16(version, 128) }.ok_or("Unity version string invalid")?;
        String::from_utf16(&units).map_err(|_| "Unity version string has invalid UTF-16")
    }

    /// `il2cpp_runtime_invoke`; `Err(())` when the export is missing or a
    /// managed exception was thrown.
    ///
    /// # Safety
    /// `object` is null (static) or a live instance of the method's class, and
    /// `args` matches the method's parameters (`void*` per argument, values
    /// by pointer).
    pub unsafe fn invoke(&self, method: Method, object: Ptr, args: *mut Ptr) -> Result<Ptr, ()> {
        if method.1 != self.base { return Err(()); }
        let function = self.f.runtime_invoke.ok_or(())?;
        let mut exception = std::ptr::null_mut();
        // SAFETY: caller contract.
        let result = unsafe { function(method.as_ptr(), object, args, &mut exception) };
        if exception.is_null() {
            Ok(result)
        } else {
            Err(())
        }
    }

    /// `il2cpp_object_unbox`.
    ///
    /// # Safety
    /// `object` is a live boxed value type.
    pub unsafe fn unbox(&self, object: Ptr) -> Option<Ptr> {
        // SAFETY: caller contract.
        self.f.object_unbox.map(|function| unsafe { function(object) })
    }

    /// Copy of a managed string of at most `max` UTF-16 units; `None` for
    /// null, empty-export, negative or longer strings.
    ///
    /// # Safety
    /// `string` is null or a live `Il2CppString*`, kept reachable for the call.
    pub unsafe fn string_utf16(&self, string: Ptr, max: usize) -> Option<Vec<u16>> {
        let (length, chars) = (self.f.string_length?, self.f.string_chars?);
        if string.is_null() {
            return None;
        }
        // SAFETY: caller contract.
        let length = usize::try_from(unsafe { length(string) }).ok().filter(|l| (1..=max).contains(l))?;
        // Reserve before borrowing the guest character address. Subsequent
        // copies cannot allocate while that address is in use.
        let mut units = Vec::with_capacity(length);
        // SAFETY: caller contract.
        let chars = unsafe { chars(string) };
        if chars.is_null() {
            return None;
        }
        let mut copy = [0u16; 256];
        for start in (0..length).step_by(copy.len()) {
            let len = (length - start).min(copy.len());
            // SAFETY: `length` units at `chars` belong to the live string.
            copy[..len].copy_from_slice(unsafe { std::slice::from_raw_parts(chars.add(start), len) });
            units.extend_from_slice(&copy[..len]);
        }
        // SAFETY: keeps `string` visible on this native stack until the copy completed.
        unsafe { std::ptr::read_volatile(std::ptr::addr_of!(string)) };
        Some(units)
    }

    /// Whether a managed string equals `text` (UTF-16 compare, no allocation).
    ///
    /// # Safety
    /// As [`Self::string_utf16`].
    pub unsafe fn string_eq(&self, string: Ptr, text: &str) -> bool {
        let (Some(length), Some(chars)) = (self.f.string_length, self.f.string_chars) else {
            return false;
        };
        if string.is_null() {
            return false;
        }
        let expected = text.encode_utf16();
        // SAFETY: caller contract.
        let length = unsafe { length(string) };
        if usize::try_from(length).ok() != Some(text.encode_utf16().count()) {
            return false;
        }
        // SAFETY: caller contract.
        let chars = unsafe { chars(string) };
        // SAFETY: `length` units at `chars` belong to the live string.
        !chars.is_null() && unsafe {
            let matches = std::slice::from_raw_parts(chars, length as usize)
                .iter().copied().eq(expected);
            // Keep the managed owner visible until the character comparison
            // finishes, just as string_utf16 does for its copy.
            std::ptr::read_volatile(std::ptr::addr_of!(string));
            matches
        }
    }

    /// `il2cpp_string_new`; null when the export is missing or allocation failed.
    pub fn string_new(&self, text: &CStr) -> Ptr {
        call!(self.string_new(text.as_ptr())).unwrap_or(std::ptr::null_mut())
    }

    /// Discover callback-local rooting without guessing a handle width.
    /// A stripped version getter may use one exact native IL2CPP identity;
    /// malformed/foreign getters and unknown version strings remain errors.
    /// This binds functions only; it does not create or retain managed roots.
    pub fn verified_gc_handles(&self, application: Class) -> Result<GcHandles, &'static str> {
        use crate::il2cpp_abi::{require_gc_handle_abi, stripped_gc_handle_abi};
        let abi = match self.unity_version(application) {
            Ok(version) => require_gc_handle_abi(&version)
                .map_err(|_| "unverified Unity build/target GC-handle ABI")?,
            Err("Unity version getter unavailable") => stripped_gc_handle_abi(
                self.base,
                [self.f.gchandle_new.map(|f| f as usize),
                 self.f.gchandle_get_target.map(|f| f as usize),
                 self.f.gchandle_free.map(|f| f as usize)],
            ).ok_or("Unity version getter unavailable; native GC-handle ABI unverified")?,
            Err(reason) => return Err(reason),
        };
        // SAFETY: the exact version table or complete native identity guard
        // selected all three prototypes. bind pins the immutable standard
        // exports; no managed object or caller-supplied ABI enters this call.
        unsafe { self.gc_handles(abi) }.ok_or("GC root exports unavailable")
    }

    /// GC-handle API with the build's verified handle width, when all three
    /// exports are bound.
    ///
    /// # Safety
    /// `abi` must match this exact module's three GC-handle exports. Handle
    /// widths are function signatures, not just masks on returned integers.
    pub unsafe fn gc_handles(&self, abi: GcHandleAbi) -> Option<GcHandles> {
        let (new, target, free) = (self.f.gchandle_new?, self.f.gchandle_get_target?, self.f.gchandle_free?);
        Some(GcHandles(match abi {
            GcHandleAbi::PointerWidth => GcHandleExports::PointerWidth { new, target, free },
            // SAFETY: the caller verified that all three exports use u32;
            // the stored pointer-width prototypes have not been called.
            GcHandleAbi::U32 => unsafe { GcHandleExports::U32 {
                new: std::mem::transmute::<unsafe extern "C-unwind" fn(Ptr, bool) -> usize, unsafe extern "C-unwind" fn(Ptr, bool) -> u32>(new),
                target: std::mem::transmute::<unsafe extern "C-unwind" fn(usize) -> Ptr, unsafe extern "C-unwind" fn(u32) -> Ptr>(target),
                free: std::mem::transmute::<unsafe extern "C-unwind" fn(usize), unsafe extern "C-unwind" fn(u32)>(free),
            } },
        }))
    }
}

/// Typed GC-handle exports for one handle ABI.
#[derive(Clone, Copy)]
pub struct GcHandles(GcHandleExports);

#[derive(Clone, Copy)]
enum GcHandleExports {
    U32 {
        new: unsafe extern "C-unwind" fn(Ptr, bool) -> u32,
        target: unsafe extern "C-unwind" fn(u32) -> Ptr,
        free: unsafe extern "C-unwind" fn(u32),
    },
    PointerWidth {
        new: unsafe extern "C-unwind" fn(Ptr, bool) -> usize,
        target: unsafe extern "C-unwind" fn(usize) -> Ptr,
        free: unsafe extern "C-unwind" fn(usize),
    },
}

impl GcHandles {
    /// A strong, non-pinned root for `object`; `None` when IL2CPP returns 0.
    ///
    /// # Safety
    /// `object` is a live managed object.
    pub unsafe fn root(&self, object: Ptr) -> Option<GcRoot> {
        // SAFETY: live object from the caller; the enum selects the exact
        // verified prototype for both handle creation and later release.
        let raw = unsafe { match self.0 {
            GcHandleExports::U32 { new, .. } => new(object, false) as usize,
            GcHandleExports::PointerWidth { new, .. } => new(object, false),
        } };
        // Lazily: a GcRoot built for 0 would free handle 0 when dropped.
        (raw != 0).then(|| GcRoot { handles: *self, raw })
    }
}

/// An owned GC handle; freed on drop.
pub struct GcRoot {
    handles: GcHandles,
    raw: usize,
}

impl GcRoot {
    /// The rooted object (null once collected for weak handles).
    pub fn target(&self) -> Ptr {
        // SAFETY: `raw` is a live handle from `GcHandles::root`.
        unsafe { match self.handles.0 {
            GcHandleExports::U32 { target, .. } => target(self.raw as u32),
            GcHandleExports::PointerWidth { target, .. } => target(self.raw),
        } }
    }
}

impl Drop for GcRoot {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live handle, freed exactly once here.
        unsafe { match self.handles.0 {
            GcHandleExports::U32 { free, .. } => free(self.raw as u32),
            GcHandleExports::PointerWidth { free, .. } => free(self.raw),
        } }
    }
}

fn owned(name: *const c_char) -> Option<String> {
    // SAFETY: IL2CPP metadata names are NUL-terminated and process-lifetime.
    (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "../tests/unit/il2cpp.rs"]
mod tests;
