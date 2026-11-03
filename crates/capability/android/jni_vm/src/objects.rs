//! INPUT: shared-session class identities and JNI reference handles
//! OUTPUT: checked object arrays retaining/releasing the same referenced entities
//! POS: shared JNI heap operations; no Android or Unity service state
#![forbid(unsafe_code)]
use super::{RefTarget, Session};
use std::ffi::c_void;
use std::cell::Cell;

pub(super) struct ObjectArray {
    pub class: u64,
    pub element_class: u64,
    pub elements: Vec<Option<RefTarget>>,
    pub primitive: Option<super::primitive_arrays::Values>,
    pub marked: Cell<bool>,
    pub trace_next: Cell<Option<RefTarget>>,
    pub incoming: Cell<usize>,
}

impl Session {
    pub(super) fn new_object_array(&self, size: i32, class: *mut c_void, initial: *mut c_void) -> *mut c_void {
        if self.blocked_by_pending("NewObjectArray") { return std::ptr::null_mut(); }
        if size < 0 {
            self.set_pending("java/lang/NegativeArraySizeException", "negative object array size");
            return std::ptr::null_mut();
        }
        let Some(element_class) = self.resolve_class(class, "NewObjectArray") else { return std::ptr::null_mut(); };
        let initial_root = if initial.is_null() { None } else { self.root_reference(initial) };
        let initial = if initial.is_null() { None } else {
            let Some(root) = &initial_root else {
                self.set_pending("java/lang/IllegalArgumentException", "invalid initial array element");
                return std::ptr::null_mut();
            };
            let Some(actual) = self.object_class(initial, "NewObjectArray") else { return std::ptr::null_mut(); };
            if !self.is_subclass(actual, element_class) {
                self.set_pending("java/lang/ArrayStoreException", "initial element has incompatible class");
                return std::ptr::null_mut();
            }
            Some(root.target)
        };
        let mut elements = Vec::new();
        if elements.try_reserve_exact(size as usize).is_err() {
            self.set_pending("java/lang/OutOfMemoryError", "object array allocation failed");
            return std::ptr::null_mut();
        }
        elements.resize(size as usize, initial);
        let element_name = self.class_name(element_class);
        let name = if element_name.starts_with('[') { format!("[{element_name}") } else { format!("[L{element_name};") };
        let Some(class) = self.find_class(&name) else { return std::ptr::null_mut(); };
        let array_class = self.resolve_class(class, "NewObjectArray").unwrap();
        self.delete_ref(class, false, "NewObjectArray");
        let Some(id) = self.next_serial() else {
            self.serial_exhausted("NewObjectArray");
            return std::ptr::null_mut();
        };
        // Establish a JNI root before publishing the payload, so another
        // thread's cycle collector cannot observe an unrooted new array.
        let array = self.new_ref(RefTarget::Array(id), false);
        if array.is_null() { return array; }
        for target in elements.iter().flatten() { self.retain_edge(*target); }
        self.arrays.lock().unwrap().insert(id, ObjectArray { class:array_class, element_class, elements, primitive:None, marked:Cell::new(false), trace_next:Cell::new(None), incoming:Cell::new(0) });
        array
    }

    pub(super) fn array_length(&self, array: *mut c_void) -> i32 {
        if self.blocked_by_pending("GetArrayLength") { return 0; }
        let Some(RefTarget::Array(id)) = self.resolve(array) else {
            self.set_pending("java/lang/IllegalArgumentException", "not a live array");
            return 0;
        };
        let arrays=self.arrays.lock().unwrap();
        let Some(array)=arrays.get(&id) else { return 0; };
        array.primitive.as_ref().map_or(array.elements.len(),super::primitive_arrays::Values::len) as i32
    }

    pub(super) fn object_array_element(&self, array: *mut c_void, index: i32) -> *mut c_void {
        if self.blocked_by_pending("GetObjectArrayElement") { return std::ptr::null_mut(); }
        let Some(RefTarget::Array(id)) = self.resolve(array) else {
            self.set_pending("java/lang/IllegalArgumentException", "not a live object array");
            return std::ptr::null_mut();
        };
        let target = {
            let mut live = self.liveness.lock().unwrap();
            let arrays = self.arrays.lock().unwrap();
            let target = arrays.get(&id).and_then(|a| usize::try_from(index).ok().and_then(|i| a.elements.get(i))).copied();
            if let Some(Some(RefTarget::Str(entity)|RefTarget::Throwable(entity)|RefTarget::Object(entity)|RefTarget::Array(entity))) = target {
                let count = live.entry(entity).or_default();
                *count = count.checked_add(1).expect("JNI pin overflow");
            }
            target
        };
        match target {
            Some(Some(target)) => { let result = self.new_ref(target, false); self.release_target(target); result },
            Some(None) => std::ptr::null_mut(),
            None => {
                self.set_pending("java/lang/ArrayIndexOutOfBoundsException", "object array index out of bounds");
                std::ptr::null_mut()
            }
        }
    }

    pub(super) fn set_object_array_element(&self, array: *mut c_void, index: i32, value: *mut c_void) {
        if self.blocked_by_pending("SetObjectArrayElement") { return; }
        let array_root = self.root_reference(array);
        let Some(RefTarget::Array(id)) = array_root.as_ref().map(|root| root.target) else {
            self.set_pending("java/lang/IllegalArgumentException", "not a live object array"); return;
        };
        let Some(index) = usize::try_from(index).ok() else {
            self.set_pending("java/lang/ArrayIndexOutOfBoundsException", "negative object array index"); return;
        };
        let element_class = {
            let arrays = self.arrays.lock().unwrap();
            let array = arrays.get(&id).unwrap();
            if index >= array.elements.len() {
                drop(arrays);
                self.set_pending("java/lang/ArrayIndexOutOfBoundsException", "object array index out of bounds"); return;
            }
            array.element_class
        };
        let value_root = if value.is_null() { None } else { self.root_reference(value) };
        let target = if value.is_null() { None } else {
            let Some(root) = &value_root else {
                self.set_pending("java/lang/IllegalArgumentException", "invalid object array element"); return;
            };
            let Some(class) = self.object_class(value, "SetObjectArrayElement") else { return; };
            if !self.is_subclass(class, element_class) {
                self.set_pending("java/lang/ArrayStoreException", "incompatible object array element"); return;
            }
            let target = root.target;
            self.retain_edge(target);
            Some(target)
        };
        let previous = std::mem::replace(&mut self.arrays.lock().unwrap().get_mut(&id).unwrap().elements[index], target);
        if let Some(previous) = previous { self.release_target_inner(previous, true); }
    }
}
