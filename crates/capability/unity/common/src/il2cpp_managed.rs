//! INPUT: checked IL2CPP metadata, attached runtime and rooted values
//! OUTPUT: managed invocation, allocation and GC operations
//! POS: shared managed-object boundary

use super::*;

impl Il2cpp {
    pub(super) fn reference_type(&self, ty: Type) -> bool {
        self.type_byref(ty) == Some(false)
            && self
                .type_kind(ty)
                .is_some_and(|kind| matches!(kind, 0x0e | 0x12 | 0x14 | 0x15 | 0x1c | 0x1d))
            && self
                .class_from_type(ty)
                .is_some_and(|class| call!(self.class_is_valuetype(class.as_ptr())) == Some(false))
    }

    fn managed_context(&self, gc: &GcHandles) -> Result<(), &'static str> {
        if gc.base != self.base {
            return Err("GC handles belong to another IL2CPP module");
        }
        if !self.thread_attached() {
            return Err("managed thread is not attached");
        }
        Ok(())
    }

    fn root_pointer(&self, root: &GcRoot) -> Result<Ptr, &'static str> {
        if root.handles.base != self.base {
            return Err("root belongs to another IL2CPP module");
        }
        if !self.thread_attached() {
            return Err("managed thread is not attached");
        }
        let object = root.target();
        if object.is_null() {
            Err("root target is null")
        } else {
            Ok(object)
        }
    }

    unsafe fn field_receiver(&self, object: &GcRoot, field: Field) -> Result<(), &'static str> {
        let parent = self
            .field_parent(field)
            .ok_or("field belongs to another IL2CPP module")?;
        let object = self.root_pointer(object)?;
        // SAFETY: module-matched strong root on this attached thread.
        let actual = unsafe { self.object_class(object) }.ok_or("object class unavailable")?;
        if self.is_assignable_from(parent, actual) {
            Ok(())
        } else {
            Err("field receiver type mismatch")
        }
    }

    unsafe fn checked_reference(
        &self,
        ty: Type,
        value: Option<&GcRoot>,
    ) -> Result<(), &'static str> {
        if !self.reference_type(ty) {
            return Err("reference type mismatch");
        }
        if let Some(root) = value {
            let object = self.root_pointer(root)?;
            // SAFETY: module-matched strong root on this attached thread.
            let actual = unsafe { self.object_class(object) }.ok_or("object class unavailable")?;
            let expected = self
                .class_from_type(ty)
                .ok_or("reference class unavailable")?;
            if !self.is_assignable_from(expected, actual) {
                return Err("reference object type mismatch");
            }
        }
        Ok(())
    }

    unsafe fn root_result(
        &self,
        value: Ptr,
        gc: &GcHandles,
    ) -> Result<Option<GcRoot>, &'static str> {
        self.managed_context(gc)?;
        if value.is_null() {
            return Ok(None);
        }
        // SAFETY: callers pass the live managed result of an IL2CPP operation;
        // this attached native frame keeps it reachable until the strong root.
        unsafe { gc.root(value) }
            .map(Some)
            .ok_or("managed root allocation failed")
    }

    /// Allocate without invoking an instance constructor, and root immediately.
    /// # Safety
    /// The calling Unity thread permits this allocation and class initialization;
    /// `class` is an allocatable, fully closed type from the trusted image.
    pub unsafe fn allocate_object(
        &self,
        class: Class,
        gc: &GcHandles,
    ) -> Result<GcRoot, &'static str> {
        self.managed_context(gc)?;
        if class.1 != self.base {
            return Err("class belongs to another IL2CPP module");
        }
        let object =
            call!(self.object_new(class.as_ptr())).ok_or("object allocation export unavailable")?;
        // SAFETY: the allocation just returned a managed reference on this thread.
        unsafe { self.root_result(object, gc) }?.ok_or("managed object allocation failed")
    }

    /// Box exactly one bounded value payload in runtime-owned storage.
    /// # Safety
    /// Every byte, including struct padding, is initialized and forms a valid
    /// value of `class`. Embedded managed references remain rooted through the
    /// call. Unity permits the allocation.
    pub unsafe fn box_value(
        &self,
        class: Class,
        bytes: &[u8],
        gc: &GcHandles,
    ) -> Result<GcRoot, &'static str> {
        self.managed_context(gc)?;
        if self.value_layout(class).map(|v| v.0) != Some(bytes.len()) {
            return Err("boxed value layout mismatch");
        }
        let mut value = ValueBytes::copy(bytes).ok_or("boxed value layout mismatch")?;
        let object = call!(self.value_box(class.as_ptr(), value.0.as_mut_ptr().cast()))
            .ok_or("value boxing export unavailable")?;
        // SAFETY: the boxing API just returned a live managed reference.
        unsafe { self.root_result(object, gc) }?.ok_or("managed boxing allocation failed")
    }

    /// Read a static reference and root it before any later managed operation.
    /// # Safety
    /// Calling Unity thread permits the field's original class initialization.
    pub unsafe fn static_reference_field(
        &self,
        field: Field,
        gc: &GcHandles,
    ) -> Result<Option<GcRoot>, &'static str> {
        self.managed_context(gc)?;
        let ty = self.field_type(field).ok_or("field type unavailable")?;
        if self.field_flags(field).is_none_or(|f| f & 0x10 == 0) || !self.reference_type(ty) {
            return Err("static reference field mismatch");
        }
        let mut value = std::ptr::null_mut();
        call!(self.field_static_get_value(field.as_ptr(), (&mut value as *mut Ptr).cast()))
            .ok_or("static field export unavailable")?;
        // SAFETY: the field accessor returned a live reference or null.
        let root = unsafe { self.root_result(value, gc) }?;
        // SAFETY: the newly returned value remains strongly rooted.
        unsafe { self.checked_reference(ty, root.as_ref()) }?;
        Ok(root)
    }

    /// Instance counterpart of `static_reference_field`, using FieldInfo only.
    /// # Safety
    /// The calling Unity thread permits managed access; owner/value roots stay live.
    pub unsafe fn read_reference_field(
        &self,
        object: &GcRoot,
        field: Field,
        gc: &GcHandles,
    ) -> Result<Option<GcRoot>, &'static str> {
        self.managed_context(gc)?;
        // SAFETY: caller permits access; the helper validates module and receiver.
        unsafe { self.field_receiver(object, field) }?;
        let ty = self.field_type(field).ok_or("field type unavailable")?;
        if self.field_flags(field).is_none_or(|f| f & 0x10 != 0) || !self.reference_type(ty) {
            return Err("instance reference field mismatch");
        }
        let mut value = std::ptr::null_mut();
        let owner = self.root_pointer(object)?;
        call!(self.field_get_value(owner, field.as_ptr(), (&mut value as *mut Ptr).cast()))
            .ok_or("instance field export unavailable")?;
        // SAFETY: the accessor returned a live reference or null, immediately rooted.
        let root = unsafe { self.root_result(value, gc) }?;
        // SAFETY: any nonnull returned value is now strongly rooted.
        unsafe { self.checked_reference(ty, root.as_ref()) }?;
        Ok(root)
    }

    /// Write a mutable instance reference through IL2CPP's write barrier.
    /// # Safety
    /// The calling Unity thread permits this mutation. It may affect game state.
    pub unsafe fn write_reference_field(
        &self,
        object: &GcRoot,
        field: Field,
        value: Option<&GcRoot>,
    ) -> Result<(), &'static str> {
        // SAFETY: caller permits mutation; helpers validate root ownership/types.
        unsafe { self.field_receiver(object, field) }?;
        let ty = self.field_type(field).ok_or("field type unavailable")?;
        if self.field_flags(field).is_none_or(|f| f & 0x70 != 0) {
            return Err("field is not a mutable instance");
        }
        // SAFETY: value is borrowed from a module-matched strong root or null.
        unsafe { self.checked_reference(ty, value) }?;
        let value = value
            .map(|root| self.root_pointer(root))
            .transpose()?
            .unwrap_or(std::ptr::null_mut());
        let owner = self.root_pointer(object)?;
        call!(self.field_set_value_object(owner, field.as_ptr(), value))
            .ok_or("reference write export unavailable")
    }

    /// Copy an exactly typed value field using its runtime payload width,
    /// without interpreting object headers. Padded structs normally require
    /// BoxedValue instead of copying their bytes.
    /// # Safety
    /// Calling Unity thread permits access. The field's entire payload, including
    /// any padding, must be initialized. Returned bytes may contain managed
    /// references; caller must not retain/use those beyond their rooted owners.
    pub unsafe fn value_field(
        &self,
        object: &GcRoot,
        field: Field,
    ) -> Result<Vec<u8>, &'static str> {
        // SAFETY: caller permits access; receiver/module identity is checked.
        unsafe { self.field_receiver(object, field) }?;
        if self.field_flags(field).is_none_or(|f| f & 0x10 != 0) {
            return Err("field is not an instance value");
        }
        let ty = self.field_type(field).ok_or("field type unavailable")?;
        if self.type_byref(ty) != Some(false) {
            return Err("byref field is unsupported");
        }
        let class = self.class_from_type(ty).ok_or("value class unavailable")?;
        let (size, _) = self
            .value_layout(class)
            .ok_or("field value layout unsupported")?;
        let mut bytes = ValueBytes([0; 256]);
        let owner = self.root_pointer(object)?;
        call!(self.field_get_value(owner, field.as_ptr(), bytes.0.as_mut_ptr().cast()))
            .ok_or("instance field export unavailable")?;
        Ok(bytes.0[..size].to_vec())
    }

    /// Copy a bounded boxed value's payload while its owner stays rooted.
    /// # Safety
    /// Calling Unity thread permits access. The entire boxed payload, including
    /// any padding, must be initialized. Embedded references have the same
    /// lifetime restrictions as `value_field`. Use BoxedValue for a padded
    /// runtime struct when this initialization guarantee is unavailable.
    pub unsafe fn copy_unboxed(
        &self,
        object: &GcRoot,
        expected: Class,
    ) -> Result<Vec<u8>, &'static str> {
        let (size, _) = self
            .value_layout(expected)
            .ok_or("boxed value layout unsupported")?;
        let pointer = self.root_pointer(object)?;
        // SAFETY: a module-matched strong root is live on this attached thread.
        if unsafe { self.object_class(pointer) } != Some(expected) {
            return Err("boxed value type mismatch");
        }
        let mut bytes = vec![0; size];
        // SAFETY: checked runtime class is the expected value type; root stays live.
        let value = unsafe { self.unbox(self.root_pointer(object)?) }
            .filter(|p| !p.is_null())
            .ok_or("boxed value payload unavailable")?;
        // SAFETY: class_value_size bounds the payload; the caller guarantees
        // every copied byte is initialized, including any struct padding.
        unsafe { std::ptr::copy_nonoverlapping(value.cast::<u8>(), bytes.as_mut_ptr(), size) };
        Ok(bytes)
    }

    /// Invoke a fully checked managed signature, keeping all references rooted.
    /// Value parameters use aligned copies, while reference parameters are the
    /// object pointer itself (not a pointer to that pointer).
    /// # Safety
    /// Unity permits the selected business operation on this thread. Value
    /// bytes, including any padding, are initialized valid instances of their
    /// types, with embedded references rooted.
    /// The trusted module and all metadata/code remain mapped through the call.
    pub unsafe fn invoke_checked(
        &self,
        method: &CheckedMethod,
        receiver: Option<&GcRoot>,
        args: &[ManagedArg<'_>],
        gc: &GcHandles,
    ) -> Result<Option<GcRoot>, &'static str> {
        self.managed_context(gc)?;
        if method.method.1 != self.base || args.len() != method.parameters.len() {
            return Err("managed invocation signature mismatch");
        }
        if method.static_ != receiver.is_none() {
            return Err("managed receiver staticness mismatch");
        }
        if let Some(root) = receiver {
            // SAFETY: module is checked before this object's runtime class lookup.
            let actual = unsafe { self.object_class(self.root_pointer(root)?) }
                .ok_or("receiver class unavailable")?;
            if !self.is_assignable_from(method.owner, actual) {
                return Err("managed receiver type mismatch");
            }
        }
        let mut values = Vec::with_capacity(args.len());
        for (arg, &expected) in args.iter().zip(&method.parameters) {
            match arg {
                ManagedArg::Reference(root) => {
                    // SAFETY: each argument is borrowed from a live strong root.
                    unsafe { self.checked_reference(expected, *root) }?;
                    values.push(None);
                }
                ManagedArg::Value(ty, bytes) => {
                    if ty.1 != self.base || self.type_byref(*ty) != Some(false) {
                        return Err("managed value type mismatch");
                    }
                    let actual = self
                        .class_from_type(*ty)
                        .ok_or("managed value type unavailable")?;
                    let class = self
                        .class_from_type(expected)
                        .ok_or("value class unavailable")?;
                    // FullName alone is not identity: distinct assemblies in
                    // one IL2CPP binding may declare the same named value type.
                    if actual != class {
                        return Err("managed value type mismatch");
                    }
                    if self.value_layout(class).map(|v| v.0) != Some(bytes.len()) {
                        return Err("managed value layout mismatch");
                    }
                    values.push(Some(
                        ValueBytes::copy(bytes).ok_or("managed value layout unsupported")?,
                    ));
                }
                ManagedArg::BoxedValue(root) => {
                    let expected_class = self
                        .class_from_type(expected)
                        .ok_or("value class unavailable")?;
                    self.value_layout(expected_class)
                        .ok_or("managed value layout unsupported")?;
                    // SAFETY: module-matched strong root is checked before access.
                    if unsafe { self.object_class(self.root_pointer(root)?) }
                        != Some(expected_class)
                    {
                        return Err("boxed argument type mismatch");
                    }
                    values.push(None);
                }
            }
        }
        let mut parameters = Vec::with_capacity(args.len());
        for (arg, value) in args.iter().zip(values.iter_mut()) {
            parameters.push(match arg {
                ManagedArg::Reference(root) => root
                    .map(|r| self.root_pointer(r))
                    .transpose()?
                    .unwrap_or(std::ptr::null_mut()),
                ManagedArg::Value(_, _) => value
                    .as_mut()
                    .ok_or("managed value storage unavailable")?
                    .0
                    .as_mut_ptr()
                    .cast(),
                ManagedArg::BoxedValue(root) => {
                    // SAFETY: argument's exact boxed value class is checked above;
                    // root remains borrowed throughout runtime invocation.
                    unsafe { self.unbox(self.root_pointer(root)?) }
                        .filter(|p| !p.is_null())
                        .ok_or("boxed argument payload unavailable")?
                }
            });
        }
        let mut object = receiver
            .map(|r| self.root_pointer(r))
            .transpose()?
            .unwrap_or(std::ptr::null_mut());
        if receiver.is_some() {
            match call!(self.class_is_valuetype(method.owner.as_ptr())) {
                Some(false) => {}
                Some(true) => {
                    // SAFETY: a rooted box of the declaring value type was checked above.
                    object = unsafe { self.unbox(object) }
                        .filter(|p| !p.is_null())
                        .ok_or("value receiver unavailable")?;
                }
                None => return Err("receiver value-type metadata unavailable"),
            }
        }
        // SAFETY: token/receiver/full typed arguments were checked above, and
        // all roots and aligned buffers remain alive until invocation returns.
        let value = unsafe {
            self.invoke(
                method.method,
                object,
                if parameters.is_empty() {
                    std::ptr::null_mut()
                } else {
                    parameters.as_mut_ptr()
                },
            )
        }
        .map_err(|_| "managed invocation failed")?;
        // SAFETY: runtime_invoke returned a managed reference/box or null.
        let root = unsafe { self.root_result(value, gc) }?;
        if self.type_kind(method.returns) == Some(TYPE_VOID) {
            if root.is_some() {
                return Err("void invocation returned an object");
            }
        } else if self.reference_type(method.returns) {
            // SAFETY: any nonnull result is strongly rooted before inspection.
            unsafe { self.checked_reference(method.returns, root.as_ref()) }?;
        } else {
            let expected = self
                .class_from_type(method.returns)
                .ok_or("return class unavailable")?;
            let root = root.as_ref().ok_or("value invocation returned null")?;
            // SAFETY: the returned value is rooted; exact class confirms the box.
            if unsafe { self.object_class(self.root_pointer(root)?) } != Some(expected) {
                return Err("boxed return type mismatch");
            }
        }
        Ok(root)
    }

    /// Create a managed String on an attached thread.
    ///
    /// # Safety
    /// The calling thread is attached. Keep the returned reference reachable
    /// through its final managed use; never retain it unrooted across frames.
    pub unsafe fn new_string(&self, text: &CStr) -> Option<Ptr> {
        if !self.thread_attached() {
            return None;
        }
        let new = self.f.string_new?;
        // SAFETY: text is a valid C string and the caller supplies thread context.
        let value = unsafe { new(text.as_ptr()) };
        (!value.is_null()).then_some(value)
    }

    /// Copy exact UTF-16 code units into a managed String, including embedded
    /// NULs and unpaired surrogates. The standard optional export owns the copy.
    /// # Safety
    /// The calling thread is attached and permits String allocation. Keep the
    /// returned reference reachable/rooted through its final managed use.
    pub unsafe fn new_string_utf16(&self, text: &[u16]) -> Option<Ptr> {
        let length = utf16_length(text.len())?;
        if !self.thread_attached() {
            return None;
        }
        let new = self.f.string_new_utf16?;
        // SAFETY: the slice is live for this call; checked i32 unit count and
        // the caller's attached/owner context match the standard constructor.
        let value = unsafe { new(text.as_ptr(), length) };
        (!value.is_null()).then_some(value)
    }

    /// Whether the calling thread is attached to the IL2CPP runtime.
    pub fn thread_attached(&self) -> bool {
        call!(self.thread_current()).is_some_and(|thread| !thread.is_null())
    }

    /// Stable identity of this attached runtime thread, never a dereferenced
    /// thread structure or permission to use objects from another thread.
    pub fn thread_id(&self) -> Option<usize> {
        call!(self.thread_current())
            .filter(|thread| !thread.is_null())
            .map(|thread| thread as usize)
    }

    pub fn gc_collect(&self, generation: i32) -> bool {
        self.thread_attached() && call!(self.gc_collect(generation)).is_some()
    }

    pub fn gc_used_size(&self) -> Option<i64> {
        call!(self.gc_get_used_size())
    }

    pub fn gc_heap_size(&self) -> Option<i64> {
        call!(self.gc_get_heap_size())
    }

    /// `Application.unityVersion` via its static getter, on an attached thread.
    pub fn unity_version(&self, application: Class) -> Result<String, &'static str> {
        if application.1 != self.base {
            return Err("Application belongs to another IL2CPP module");
        }
        if !self.thread_attached() {
            return Err("Application metadata/attached IL2CPP thread unavailable");
        }
        if self.find_class(c"UnityEngine", c"Application") != Some(application) {
            return Err("Unity version requires UnityEngine.Application");
        }
        let Some(method) = self.method(application, c"get_unityVersion", 0) else {
            return Err("Unity version getter unavailable");
        };
        if !self.method_shape(method, 0, true) {
            return Err("Unity version getter shape mismatch");
        }
        if !self.method_return_type(method).is_some_and(|ty| {
            self.type_kind(ty) == Some(TYPE_STRING) && self.type_byref(ty) == Some(false)
        }) {
            return Err("Unity version getter return type mismatch");
        }
        // SAFETY: a validated static, parameterless getter returning a string.
        let version = unsafe { self.invoke(method, std::ptr::null_mut(), std::ptr::null_mut()) };
        let version = version
            .ok()
            .filter(|v| !v.is_null())
            .ok_or("Unity version query failed")?;
        // SAFETY: `version` is the live string the getter just returned; it
        // stays reachable from this attached native stack during the copy.
        let units =
            unsafe { self.string_utf16(version, 128) }.ok_or("Unity version string invalid")?;
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
        if method.1 != self.base {
            return Err(());
        }
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
        self.f
            .object_unbox
            .map(|function| unsafe { function(object) })
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
        let length = usize::try_from(unsafe { length(string) })
            .ok()
            .filter(|l| (1..=max).contains(l))?;
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
            copy[..len]
                .copy_from_slice(unsafe { std::slice::from_raw_parts(chars.add(start), len) });
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
        !chars.is_null()
            && unsafe {
                let matches = std::slice::from_raw_parts(chars, length as usize)
                    .iter()
                    .copied()
                    .eq(expected);
                // Keep the managed owner visible until the character comparison
                // finishes, just as string_utf16 does for its copy.
                std::ptr::read_volatile(std::ptr::addr_of!(string));
                matches
            }
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
                [
                    self.f.gchandle_new.map(|f| f as usize),
                    self.f.gchandle_get_target.map(|f| f as usize),
                    self.f.gchandle_free.map(|f| f as usize),
                ],
            )
            .ok_or("Unity version getter unavailable; native GC-handle ABI unverified")?,
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
        let (new, target, free) = (
            self.f.gchandle_new?,
            self.f.gchandle_get_target?,
            self.f.gchandle_free?,
        );
        Some(GcHandles {
            base: self.base,
            exports: match abi {
                GcHandleAbi::PointerWidth => GcHandleExports::PointerWidth { new, target, free },
                // SAFETY: the caller verified that all three exports use u32;
                // the stored pointer-width prototypes have not been called.
                GcHandleAbi::U32 => unsafe {
                    GcHandleExports::U32 {
                        new: std::mem::transmute::<
                            unsafe extern "C-unwind" fn(Ptr, bool) -> usize,
                            unsafe extern "C-unwind" fn(Ptr, bool) -> u32,
                        >(new),
                        target: std::mem::transmute::<
                            unsafe extern "C-unwind" fn(usize) -> Ptr,
                            unsafe extern "C-unwind" fn(u32) -> Ptr,
                        >(target),
                        free: std::mem::transmute::<
                            unsafe extern "C-unwind" fn(usize),
                            unsafe extern "C-unwind" fn(u32),
                        >(free),
                    }
                },
            },
        })
    }
}
