//! INPUT:  metadata-checked InControl array/state fields and callback-local managed roots
//! OUTPUT:  null-tolerant touch-control setter decisions with real Behaviour updates
//! POS:    preserves valid stock setters while replacing their unsafe null-array path
#![deny(unsafe_op_in_unsafe_fn)]
use crate::il2cpp::{Class, Field, GcHandles, GcRoot, Il2cpp, Method, NumericValue};
use std::{ffi::c_void, thread::ThreadId};
type Ptr = *mut c_void;
pub(crate) struct TouchControls {
    api: Il2cpp,
    roots: GcHandles,
    owner: ThreadId,
    controls: Field,
    enabled: Field,
    array: Class,
    int32: Class,
    behaviour: Class,
    length: Method,
    get_value: Method,
    set_enabled: Method,
}
impl TouchControls {
    pub(crate) fn discover(
        api: Il2cpp,
        roots: GcHandles,
        manager: Class,
    ) -> Result<Self, &'static str> {
        let controls = api
            .field(manager, c"touchControls")
            .ok_or("TouchManager.touchControls absent")?;
        let enabled = api
            .field(manager, c"_controlsEnabled")
            .ok_or("TouchManager._controlsEnabled absent")?;
        let array = api
            .find_class(c"System", c"Array")
            .ok_or("System.Array absent")?;
        let int32 = api
            .find_class(c"System", c"Int32")
            .ok_or("System.Int32 absent")?;
        let behaviour = api
            .find_class(c"UnityEngine", c"Behaviour")
            .ok_or("Behaviour absent")?;
        for (field, kind, writable) in [(controls, 0x1d, false), (enabled, 2, true)] {
            let ty = api
                .field_type(field)
                .ok_or("TouchManager field type absent")?;
            if api.type_kind(ty) != Some(kind)
                || api.type_byref(ty) != Some(false)
                || api
                    .field_flags(field)
                    .is_none_or(|f| f & if writable { 0x70 } else { 0x10 } != 0)
                || !api
                    .field_parent(field)
                    .is_some_and(|parent| api.derives_from(manager, parent))
            {
                return Err("TouchManager field kind/staticness/mutability/owner mismatch");
            }
        }
        if !api
            .field_type(controls)
            .and_then(|ty| api.class_from_type(ty))
            .is_some_and(|class| api.derives_from(class, array))
        {
            return Err("TouchManager.touchControls must be a managed Array");
        }
        let length = api
            .matching_method(array, c"get_Length", false, &[], 8)
            .ok_or("Array.Length signature mismatch")?;
        let get_value = api
            .matching_method(array, c"GetValue", false, &[(8, false)], 0x1c)
            .ok_or("Array.GetValue signature mismatch")?;
        let set_enabled = api
            .matching_method(behaviour, c"set_enabled", false, &[(2, false)], 1)
            .ok_or("Behaviour.enabled signature mismatch")?;
        if !api.has("il2cpp_field_get_value")
            || !api.has("il2cpp_field_set_value")
            || !api.has("il2cpp_runtime_invoke")
            || !api.has("il2cpp_object_unbox")
        {
            return Err("TouchManager runtime exports absent");
        }
        Ok(Self {
            api,
            roots,
            owner: std::thread::current().id(),
            controls,
            enabled,
            array,
            int32,
            behaviour,
            length,
            get_value,
            set_enabled,
        })
    }
    fn running(&self) -> Result<(), &'static str> {
        if self.owner != std::thread::current().id()
            || !self.api.thread_attached()
            || !crate::managed_hooks::active()
        {
            Err("TouchManager owner changed/detached/stopped")
        } else {
            Ok(())
        }
    }
    fn root(&self, value: Ptr) -> Result<GcRoot, &'static str> {
        if value.is_null() {
            return Err("TouchManager getter returned null");
        }
        // SAFETY: private caller supplies only a just-returned live managed reference.
        unsafe { self.roots.root(value) }.ok_or("TouchManager root creation failed")
    }
    fn verify(&self, root: &GcRoot, want: Class) -> Result<(), &'static str> {
        // SAFETY: callback-local root retains the live object throughout its class query.
        let class = unsafe { self.api.object_class(root.target()) }
            .ok_or("TouchManager object class absent")?;
        if self.api.derives_from(class, want) {
            Ok(())
        } else {
            Err("TouchManager object class mismatch")
        }
    }
    fn invoke(
        &self,
        method: Method,
        owner: &GcRoot,
        args: &mut [Ptr],
    ) -> Result<Ptr, &'static str> {
        self.running()?;
        // SAFETY: signature-checked methods, rooted exact receiver and private initialized value arguments.
        let value = unsafe { self.api.invoke(method, owner.target(), args.as_mut_ptr()) }
            .map_err(|_| "TouchManager managed call threw")?;
        self.running()?;
        Ok(value)
    }
    fn state(&self, owner: &GcRoot, value: bool) -> Result<(), &'static str> {
        self.running()?;
        // SAFETY: mutable bool field on the checked manager instance; owner stays rooted through the write.
        if unsafe {
            self.api
                .set_numeric_field(owner.target(), self.enabled, NumericValue::Bool(value))
        } {
            Ok(())
        } else {
            Err("TouchManager state write failed")
        }
    }
    /// true delegates to the stock setter; false means the safe fallback consumed the call.
    pub(crate) fn before(&self, owner: &GcRoot, enabled: bool) -> Result<bool, &'static str> {
        self.running()?;
        // SAFETY: metadata verifies exact bool field and dispatch roots the checked TouchManager receiver.
        let current = unsafe { self.api.numeric_field(owner.target(), self.enabled) };
        if current == Some(NumericValue::Bool(enabled)) {
            return Ok(false);
        }
        // SAFETY: exact SZARRAY reference field owned by this rooted manager.
        let value = unsafe { self.api.reference_field(owner.target(), self.controls) }
            .ok_or("TouchManager controls read failed")?;
        if value.is_null() {
            self.state(owner, enabled)?;
            return Ok(false);
        }
        let array = self.root(value)?;
        self.verify(&array, self.array)?;
        let length = self.invoke(self.length, &array, &mut [])?;
        let length = self.root(length)?;
        self.verify(&length, self.int32)?;
        // SAFETY: checked boxed Int32 remains rooted throughout unbox/read.
        let count = unsafe {
            self.api
                .unbox(length.target())
                .filter(|p| !p.is_null())
                .map(|p| p.cast::<i32>().read())
        }
        .ok_or("TouchManager array length unbox failed")?;
        if !(0..=128).contains(&count) {
            return Err("TouchManager array length outside bounded policy");
        }
        let mut controls = Vec::with_capacity(count as usize);
        let mut missing = false;
        for mut index in 0..count {
            let value = self.invoke(
                self.get_value,
                &array,
                &mut [(&mut index as *mut i32).cast()],
            )?;
            if value.is_null() {
                missing = true;
                continue;
            }
            let control = self.root(value)?;
            self.verify(&control, self.behaviour)?;
            controls.push(control);
        }
        if !missing {
            return Ok(true);
        }
        for control in controls {
            let mut flag = enabled;
            self.invoke(
                self.set_enabled,
                &control,
                &mut [(&mut flag as *mut bool).cast()],
            )?;
            self.running()?;
        }
        self.state(owner, enabled)?;
        Ok(false)
    }
}
