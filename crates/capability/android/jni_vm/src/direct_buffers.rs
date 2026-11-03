//! INPUT: direct-buffer JNI calls and Java ByteBuffer allocation requests
//! OUTPUT: stable byte addresses with JNI-owned storage or explicitly caller-owned memory
//! POS: shared object heap; native writes use UnsafeCell, never an escaped Rust mutable reference
use super::{RefTarget, Session};
use std::cell::UnsafeCell;
use std::ffi::c_void;

pub(super) enum Buffer {
    Owned(Box<[UnsafeCell<u8>]>),
    // JNI NewDirectByteBuffer contract: the native caller keeps this memory
    // valid for every Java reference to the buffer and synchronizes native
    // writes with every JNI/Java read. This is not a Rust owner.
    Borrowed { address: usize, capacity: i64 },
}
impl Buffer {
    fn address(&self) -> *mut c_void {
        match self { Self::Owned(bytes) => bytes.as_ptr().cast_mut().cast(), Self::Borrowed { address, .. } => *address as *mut c_void }
    }
    fn capacity(&self) -> i64 {
        match self { Self::Owned(bytes) => bytes.len() as i64, Self::Borrowed { capacity, .. } => *capacity }
    }
}
impl Session {
    fn direct_buffer(&self, buffer: Buffer) -> *mut c_void {
        let Some(class) = self.find_class("java/nio/ByteBuffer") else { return std::ptr::null_mut(); };
        let object = self.alloc_object(class); self.delete_ref(class, false, "direct buffer");
        if let Some(RefTarget::Object(id)) = self.resolve(object) { self.objects.lock().unwrap().get_mut(&id).unwrap().direct = Some(buffer); }
        object
    }
    pub(super) fn allocate_direct_buffer(&self, capacity: i32) -> *mut c_void {
        if self.blocked_by_pending("ByteBuffer.allocateDirect") { return std::ptr::null_mut(); }
        let Ok(capacity) = usize::try_from(capacity) else { self.set_pending("java/lang/IllegalArgumentException", "negative direct buffer capacity"); return std::ptr::null_mut(); };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(capacity).is_err() { self.set_pending("java/lang/OutOfMemoryError", "direct buffer allocation failed"); return std::ptr::null_mut(); }
        bytes.resize_with(capacity, || UnsafeCell::new(0));
        self.direct_buffer(Buffer::Owned(bytes.into_boxed_slice()))
    }
    pub(super) fn direct_buffer_address(&self, object: *mut c_void) -> *mut c_void {
        if self.blocked_by_pending("GetDirectBufferAddress") { return std::ptr::null_mut(); }
        let Some(RefTarget::Object(id)) = self.resolve(object) else { return std::ptr::null_mut(); };
        self.objects.lock().unwrap().get(&id).and_then(|object| object.direct.as_ref()).map_or(std::ptr::null_mut(), Buffer::address)
    }
    pub(super) fn direct_buffer_capacity(&self, object: *mut c_void) -> i64 {
        if self.blocked_by_pending("GetDirectBufferCapacity") { return -1; }
        let Some(RefTarget::Object(id)) = self.resolve(object) else { return -1; };
        self.objects.lock().unwrap().get(&id).and_then(|object| object.direct.as_ref()).map_or(-1, Buffer::capacity)
    }
    pub(super) fn zero_owned_buffer(&self,object:*mut c_void){
        if self.blocked_by_pending("ByteBuffer._rudroidZeroOwned"){return;}
        let root=self.root_reference(object);
        let Some(RefTarget::Object(id))=root.as_ref().map(|r|r.target)else{self.set_pending("java/lang/IllegalArgumentException","not an owned direct buffer");return;};
        let objects=self.objects.lock().unwrap();
        let Some(Buffer::Owned(bytes))=objects.get(&id).and_then(|o|o.direct.as_ref())else{drop(objects);self.set_pending("java/lang/IllegalArgumentException","not an owned direct buffer");return;};
        // SAFETY: owned stable allocation, rooted throughout, VM access serialized by the
        // object lock. JNI/native consumers must synchronize raw buffer access with Java
        // operations; the FMOD producer calls this only before its synchronous native mix.
        for byte in bytes.iter(){unsafe{byte.get().write(0)};}
    }
}
pub(super) mod shims {
    use super::*;
    pub extern "C" fn allocate_direct(_env: *mut c_void, _class: *mut c_void, capacity: i32) -> *mut c_void {
        super::super::session().map_or(std::ptr::null_mut(), |s| s.allocate_direct_buffer(capacity))
    }
    pub extern "C" fn zero_owned(_env:*mut c_void,object:*mut c_void){if let Some(s)=super::super::session(){s.zero_owned_buffer(object);}}
    /// # Safety
    /// The caller keeps `capacity` bytes at `address` alive until every Java
    /// reference and native use has ended, and synchronizes writes with reads.
    pub unsafe extern "C" fn new_direct_byte_buffer(_env: *mut c_void, address: *mut c_void, capacity: i64) -> *mut c_void {
        let Some(s) = super::super::session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("NewDirectByteBuffer") { return std::ptr::null_mut(); }
        if capacity < 0 || capacity > i32::MAX as i64 || address.is_null() && capacity != 0 { s.set_pending("java/lang/IllegalArgumentException", "invalid direct buffer memory"); return std::ptr::null_mut(); }
        s.direct_buffer(Buffer::Borrowed { address: address as usize, capacity })
    }
    pub extern "C" fn get_direct_buffer_address(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
        super::super::session().map_or(std::ptr::null_mut(), |s| s.direct_buffer_address(object))
    }
    pub extern "C" fn get_direct_buffer_capacity(_env: *mut c_void, object: *mut c_void) -> i64 {
        super::super::session().map_or(-1, |s| s.direct_buffer_capacity(object))
    }
}

#[cfg(test)]
#[path="../tests/unit/direct_buffers.rs"]
mod tests;
