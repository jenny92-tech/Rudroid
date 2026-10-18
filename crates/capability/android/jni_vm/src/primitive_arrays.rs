//! INPUT:  shared JNI array identities, native POD buffers and release modes
//! OUTPUT:  eight typed primitive arrays, bounded regions, copy leases and critical access
//! POS:    one array boundary; stable UnsafeCell copies separate native writes from Rust heap data
use super::{abi, objects::ObjectArray, RefTarget, Session};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
};

macro_rules! payloads {
    ($($kind:ident:$ty:ty=$name:literal),+ $(,)?)=> {
        #[derive(Clone,Copy,Debug,PartialEq,Eq)]
        pub(super) enum Kind {$($kind),+}
        pub(super) enum Values {$($kind(Box<[$ty]>)),+}
        enum Copies {$($kind(Vec<UnsafeCell<$ty>>)),+}
        impl Kind {fn class(self)->&'static str {match self {$(Self::$kind=>$name),+}}}
        impl Values {
            fn zeroed(kind:Kind,n:usize)->Option<Self> {match kind {$(Kind::$kind=>{
                let mut v=Vec::<$ty>::new();v.try_reserve_exact(n).ok()?;v.resize(n,0 as $ty);Some(Self::$kind(v.into_boxed_slice()))
            }),+}}
            pub fn kind(&self)->Kind {match self {$(Self::$kind(_)=>Kind::$kind),+}}
            pub fn len(&self)->usize {match self {$(Self::$kind(v)=>v.len()),+}}
            fn copy(&self)->Option<Copies> {match self {$(Self::$kind(v)=>{
                let mut copy=Vec::new();copy.try_reserve_exact(v.len().max(1)).ok()?;
                copy.extend(v.iter().copied().map(UnsafeCell::new));if copy.is_empty(){copy.push(UnsafeCell::new(0 as $ty));}
                Some(Copies::$kind(copy))
            }),+}}
            /// # Safety
            /// buffer covers length initialized POD elements for Set, or writable elements
            /// for Get, of this exact kind. The validated range remains in the owned slice.
            unsafe fn region(&mut self,start:usize,length:usize,buffer:*mut c_void,write:bool) {
                match self {$(Self::$kind(v)=>{
                    // SAFETY: checked range plus the native caller's exact typed region contract.
                    unsafe {if write {std::ptr::copy_nonoverlapping(buffer.cast::<$ty>(),v.as_mut_ptr().add(start),length);}
                    else {std::ptr::copy_nonoverlapping(v.as_ptr().add(start),buffer.cast::<$ty>(),length);}}
                }),+}
            }
        }
        impl Copies {
            fn pointer(&self)->*mut c_void {match self {$(Self::$kind(v)=>v[0].get().cast()),+}}
            /// # Safety
            /// Native code has ceased accesses for this release call. COMMIT keeps the
            /// allocation valid afterwards; no Rust references to its elements are created.
            unsafe fn commit(&self,target:&mut Values) {match (self,target) {$(
                (Self::$kind(copy),Values::$kind(values))=> {
                    for (cell,out) in copy.iter().zip(values.iter_mut()) {
                        // SAFETY: initialized POD cell; caller excludes native writes during Release.
                        *out=unsafe {*cell.get()};
                    }
                }),+,_=>unreachable!("lease kind was checked before committing"),
            }}
        }
    };
}
payloads! {Boolean:u8="[Z",Byte:i8="[B",Char:u16="[C",Short:i16="[S",Int:i32="[I",Long:i64="[J",Float:f32="[F",Double:f64="[D"}
impl Values {
    pub fn bytes(&self) -> Option<&[i8]> {
        if let Self::Byte(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn bytes_mut(&mut self) -> Option<&mut [i8]> {
        if let Self::Byte(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
pub(super) struct Lease {
    array: u64,
    kind: Kind,
    copy: Copies,
    critical: bool,
}
impl Session {
    pub(super) fn new_primitive_array(&self, kind: Kind, length: i32) -> *mut c_void {
        if self.blocked_by_pending("NewPrimitiveArray") {
            return std::ptr::null_mut();
        }
        let Ok(n) = usize::try_from(length) else {
            self.set_pending(
                "java/lang/NegativeArraySizeException",
                "negative primitive array size",
            );
            return std::ptr::null_mut();
        };
        let Some(values) = Values::zeroed(kind, n) else {
            self.set_pending(
                "java/lang/OutOfMemoryError",
                "primitive array allocation failed",
            );
            return std::ptr::null_mut();
        };
        let Some(class) = self.find_class(kind.class()) else {
            return std::ptr::null_mut();
        };
        let class_id = self.resolve_class(class, "NewPrimitiveArray").unwrap();
        self.delete_ref(class, false, "NewPrimitiveArray");
        let Some(id) = self.next_serial() else {
            self.serial_exhausted("NewPrimitiveArray");
            return std::ptr::null_mut();
        };
        {
            let mut arrays = self.arrays.lock().unwrap();
            if arrays.try_reserve(1).is_err() {
                drop(arrays);
                self.set_pending(
                    "java/lang/OutOfMemoryError",
                    "primitive array table allocation failed",
                );
                return std::ptr::null_mut();
            }
        }
        let result = self.new_ref(RefTarget::Array(id), false);
        if result.is_null() {
            return result;
        }
        self.arrays.lock().unwrap().insert(
            id,
            ObjectArray {
                class: class_id,
                element_class: 0,
                elements: Vec::new(),
                primitive: Some(values),
                marked: Cell::new(false),
                trace_next: Cell::new(None),
                incoming: Cell::new(0),
            },
        );
        result
    }
    /// # Safety
    /// buffer lends length initialized/readable or writable POD elements of kind.
    /// Native callers must exclude concurrent accesses to their own buffer for this call.
    pub(super) unsafe fn primitive_region(
        &self,
        array: *mut c_void,
        kind: Kind,
        start: i32,
        length: i32,
        buffer: *mut c_void,
        write: bool,
    ) {
        if self.blocked_by_pending("PrimitiveArrayRegion") {
            return;
        }
        let Some(RefTarget::Array(id)) = self.resolve(array) else {
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "not a primitive array",
            );
            return;
        };
        let mut arrays = self.arrays.lock().unwrap();
        let Some(values) = arrays
            .get_mut(&id)
            .and_then(|a| a.primitive.as_mut())
            .filter(|v| v.kind() == kind)
        else {
            drop(arrays);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "wrong primitive array kind",
            );
            return;
        };
        let range = usize::try_from(start)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(s, n)| {
                s.checked_add(n)
                    .filter(|end| *end <= values.len())
                    .map(|_| (s, n))
            });
        let Some((start, length)) = range else {
            drop(arrays);
            self.set_pending(
                "java/lang/ArrayIndexOutOfBoundsException",
                "primitive array region out of bounds",
            );
            return;
        };
        if length == 0 {
            return;
        }
        if buffer.is_null() {
            drop(arrays);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "null primitive region buffer",
            );
            return;
        }
        // SAFETY: native POD contract plus exact-kind/range validation; array lock excludes heap mutations.
        unsafe {
            values.region(start, length, buffer, write);
        }
    }
    pub(super) fn borrow_primitive_array(
        &self,
        array: *mut c_void,
        expected: Option<Kind>,
        critical: bool,
    ) -> *mut c_void {
        if self.blocked_by_pending("GetPrimitiveArrayElements") {
            return std::ptr::null_mut();
        }
        let Some(RefTarget::Array(id)) = self.resolve(array) else {
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "not a primitive array",
            );
            return std::ptr::null_mut();
        };
        // Root promotion and payload inspection share the collector's lock order. The
        // native copy uses Vec<UnsafeCell<T>>: moving a ledger entry never relocates or
        // creates a frozen Rust reference to the allocation exposed to native writes.
        let mut live = self.liveness.lock().unwrap();
        let arrays = self.arrays.lock().unwrap();
        let Some(values) = arrays
            .get(&id)
            .and_then(|a| a.primitive.as_ref())
            .filter(|v| expected.is_none_or(|k| k == v.kind()))
        else {
            drop(arrays);
            drop(live);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "wrong primitive array kind",
            );
            return std::ptr::null_mut();
        };
        let kind = values.kind();
        let Some(copy) = values.copy() else {
            drop(arrays);
            drop(live);
            self.set_pending(
                "java/lang/OutOfMemoryError",
                "primitive element copy failed",
            );
            return std::ptr::null_mut();
        };
        drop(arrays);
        let mut leases = self.primitive_leases.lock().unwrap();
        let count = live.get(&id).copied().unwrap_or(0).checked_add(1);
        if count.is_none() || leases.try_reserve(1).is_err() {
            drop(leases);
            drop(live);
            self.set_pending(
                "java/lang/OutOfMemoryError",
                "primitive lease ownership failed",
            );
            return std::ptr::null_mut();
        }
        let pointer = copy.pointer();
        live.insert(id, count.unwrap());
        leases.insert(
            pointer as usize,
            Lease {
                array: id,
                kind,
                copy,
                critical,
            },
        );
        pointer
    }
    /// # Safety
    /// pointer belongs to this live lease; no native reads/writes overlap the release.
    /// COMMIT retains ownership and permits later accesses; 0/ABORT end its lifetime.
    pub(super) unsafe fn release_primitive_array(
        &self,
        array: *mut c_void,
        expected: Option<Kind>,
        pointer: *mut c_void,
        mode: i32,
        critical: bool,
    ) {
        if !matches!(mode, 0 | 1 | 2) {
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "invalid primitive release mode",
            );
            return;
        }
        let target = self.resolve(array);
        let mut leases = self.primitive_leases.lock().unwrap();
        let Some(lease) = leases.get(&(pointer as usize)) else {
            drop(leases);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "foreign primitive array lease",
            );
            return;
        };
        if target != Some(RefTarget::Array(lease.array))
            || expected.is_some_and(|k| k != lease.kind)
            || critical != lease.critical
        {
            drop(leases);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "primitive lease belongs to another array or entry",
            );
            return;
        }
        let id = lease.array;
        if mode != 2 {
            let mut arrays = self.arrays.lock().unwrap();
            let values = arrays.get_mut(&id).unwrap().primitive.as_mut().unwrap();
            // SAFETY: lease retains the exact array and native accesses pause during this Release call.
            unsafe {
                lease.copy.commit(values);
            }
        }
        if mode != 1 {
            leases.remove(&(pointer as usize));
            super::trim_reference_table(&mut leases);
            drop(leases);
            self.release_target(RefTarget::Array(id));
        }
    }
}

macro_rules! entries {
    ($kind:ident,$ty:ty,$new:ident,$region:ident,$borrow:ident,$release:ident,$get:ident,$set:ident,$elements:ident,$end:ident) => {
        impl Session {
            pub(super) fn $new(&self, n: i32) -> *mut c_void {
                self.new_primitive_array(Kind::$kind, n)
            }
            /// # Safety
            /// Typed JNI region contract; buffer covers length initialized/writable elements.
            pub(super) unsafe fn $region(
                &self,
                a: *mut c_void,
                s: i32,
                n: i32,
                b: *mut $ty,
                w: bool,
            ) {
                // SAFETY: typed native caller contract, centralized exact-kind/range validation.
                unsafe {
                    self.primitive_region(a, Kind::$kind, s, n, b.cast(), w);
                }
            }
            pub(super) fn $borrow(&self, a: *mut c_void) -> *mut $ty {
                self.borrow_primitive_array(a, Some(Kind::$kind), false)
                    .cast()
            }
            /// # Safety
            /// Typed live copy lease, with native accesses quiescent during release.
            pub(super) unsafe fn $release(&self, a: *mut c_void, p: *mut $ty, m: i32) {
                // SAFETY: typed lease contract checked against the single ownership ledger.
                unsafe {
                    self.release_primitive_array(a, Some(Kind::$kind), p.cast(), m, false);
                }
            }
        }
        pub extern "C" fn $new(_e: *mut c_void, n: i32) -> *mut c_void {
            super::session().map_or(std::ptr::null_mut(), |s| s.$new(n))
        }
        /// # Safety
        /// JNI region output lends length writable elements of the exact primitive type.
        pub unsafe extern "C" fn $get(
            _e: *mut c_void,
            a: *mut c_void,
            s: i32,
            n: i32,
            b: *mut $ty,
        ) {
            // SAFETY: native region contract; no caller pointer or Rust slice is retained.
            unsafe {
                if let Some(vm) = super::session() {
                    vm.$region(a, s, n, b, false);
                }
            }
        }
        /// # Safety
        /// JNI region input lends length initialized elements of the exact primitive type.
        pub unsafe extern "C" fn $set(
            _e: *mut c_void,
            a: *mut c_void,
            s: i32,
            n: i32,
            b: *const $ty,
        ) {
            // SAFETY: initialized input span under JNI's SetArrayRegion contract.
            unsafe {
                if let Some(vm) = super::session() {
                    vm.$region(a, s, n, b.cast_mut(), true);
                }
            }
        }
        /// # Safety
        /// Non-null isCopy lends one writable JNI boolean; native users obey the returned lease lifetime.
        pub unsafe extern "C" fn $elements(
            _e: *mut c_void,
            a: *mut c_void,
            copy: *mut u8,
        ) -> *mut $ty {
            let p = super::session().map_or(std::ptr::null_mut(), |s| s.$borrow(a));
            // SAFETY: write one scalar only on successful allocation; copying never exposes heap slices.
            unsafe {
                if !p.is_null() && !copy.is_null() {
                    *copy = abi::JNI_TRUE;
                }
            }
            p
        }
        /// # Safety
        /// Native code has stopped using this live typed lease for the duration of release.
        pub unsafe extern "C" fn $end(_e: *mut c_void, a: *mut c_void, p: *mut $ty, m: i32) {
            // SAFETY: centralized copy ownership, release mode and exact-kind validation.
            unsafe {
                if let Some(vm) = super::session() {
                    vm.$release(a, p, m);
                }
            }
        }
    };
}
entries!(
    Boolean,
    u8,
    new_boolean_array,
    boolean_region,
    borrow_boolean_array,
    release_boolean_array,
    get_boolean_array_region,
    set_boolean_array_region,
    get_boolean_array_elements,
    release_boolean_array_elements
);
entries!(
    Byte,
    i8,
    new_byte_array,
    byte_region,
    borrow_byte_array,
    release_byte_array,
    get_byte_array_region,
    set_byte_array_region,
    get_byte_array_elements,
    release_byte_array_elements
);
entries!(
    Char,
    u16,
    new_char_array,
    char_region,
    borrow_char_array,
    release_char_array,
    get_char_array_region,
    set_char_array_region,
    get_char_array_elements,
    release_char_array_elements
);
entries!(
    Short,
    i16,
    new_short_array,
    short_region,
    borrow_short_array,
    release_short_array,
    get_short_array_region,
    set_short_array_region,
    get_short_array_elements,
    release_short_array_elements
);
entries!(
    Int,
    i32,
    new_int_array,
    int_region,
    borrow_int_array,
    release_int_array,
    get_int_array_region,
    set_int_array_region,
    get_int_array_elements,
    release_int_array_elements
);
entries!(
    Long,
    i64,
    new_long_array,
    long_region,
    borrow_long_array,
    release_long_array,
    get_long_array_region,
    set_long_array_region,
    get_long_array_elements,
    release_long_array_elements
);
entries!(
    Float,
    f32,
    new_float_array,
    float_region,
    borrow_float_array,
    release_float_array,
    get_float_array_region,
    set_float_array_region,
    get_float_array_elements,
    release_float_array_elements
);
entries!(
    Double,
    f64,
    new_double_array,
    double_region,
    borrow_double_array,
    release_double_array,
    get_double_array_region,
    set_double_array_region,
    get_double_array_elements,
    release_double_array_elements
);
/// # Safety
/// Non-null isCopy lends one byte; native users must obey JNI critical-region restrictions.
pub unsafe extern "C" fn get_primitive_array_critical(
    _e: *mut c_void,
    a: *mut c_void,
    copy: *mut u8,
) -> *mut c_void {
    let p = super::session().map_or(std::ptr::null_mut(), |s| {
        s.borrow_primitive_array(a, None, true)
    });
    // SAFETY: exactly one successful-copy indicator, with no heap pointer exposed.
    unsafe {
        if !p.is_null() && !copy.is_null() {
            *copy = abi::JNI_TRUE;
        }
    }
    p
}
/// # Safety
/// Critical lease is live; accesses cease during this JNI release and after final 0/ABORT.
pub unsafe extern "C" fn release_primitive_array_critical(
    _e: *mut c_void,
    a: *mut c_void,
    p: *mut c_void,
    m: i32,
) {
    // SAFETY: shared copy/array owner checks apply equally to critical release.
    unsafe {
        if let Some(vm) = super::session() {
            vm.release_primitive_array(a, None, p, m, true);
        }
    }
}
#[cfg(test)]
#[path = "../tests/unit/primitive_arrays.rs"]
mod tests;
