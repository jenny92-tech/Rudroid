//! INPUT:  the shared IL2CPP binding over live exports and metadata, player-thread maintenance events
//! OUTPUT:  independently available low-memory notification, rooted unload and optional extra GC
//! POS:    preserves standard callbacks when collector ABI or optional exports are unavailable

use rudroid_common::il2cpp::{GcHandles, GcRoot, Il2cpp, Method, TYPE_BOOLEAN, TYPE_VOID};
use std::ffi::c_void;
use std::marker::PhantomData;

type Ptr = *mut c_void;

/// Exports the engine cannot run without; the GC-handle and GC exports bind
/// optionally and are checked where they are used.
pub const REQUIRED_EXPORTS: &[&str] = &[
    "il2cpp_domain_get",
    "il2cpp_domain_get_assemblies",
    "il2cpp_assembly_get_image",
    "il2cpp_class_from_name",
    "il2cpp_class_get_method_from_name",
    "il2cpp_method_get_flags",
    "il2cpp_method_get_param_count",
    "il2cpp_method_get_return_type",
    "il2cpp_method_get_param",
    "il2cpp_type_get_type",
    "il2cpp_class_from_type",
    "il2cpp_runtime_invoke",
    "il2cpp_object_unbox",
    "il2cpp_thread_current",
    "il2cpp_string_length",
    "il2cpp_string_chars",
];

/// Validated cleanup surface: the verified build's GC-handle API, static
/// `Resources.UnloadUnusedAssets()` and instance `AsyncOperation.get_isDone`.
struct Collector {
    handles: GcHandles,
    unload: Method,
    done: Method,
}

pub struct Engine {
    api: Il2cpp,
    collector: Option<Collector>,
    cleanup_unavailable: Option<String>,
    pub unity_version: String,
    notify: Option<(Method, bool)>,
    /// At most one rooted AsyncOperation; dropping the root frees its handle
    /// (completion, stop or engine drop). Engines are owned only by the
    /// attached player-thread controller; UnityStopping is delivered before
    /// nativeDone/unmapping.
    operation: Option<GcRoot>,
    /// Discovered on an attached player thread and confined to it (`!Send`):
    /// the managed calls below rely on that attachment.
    _thread: PhantomData<*const ()>,
}

impl Engine {
    pub fn discover(api: Il2cpp) -> Result<Self, String> {
        if !api.thread_attached() {
            return Err("player thread is not attached to IL2CPP".into());
        }
        let Some(application) = api.find_class(c"UnityEngine", c"Application") else {
            return Err("Application metadata unavailable for handle ABI selection".into());
        };
        let unity_version = api.unity_version(application)?;
        if crate::STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            return Err("player stopping".into());
        }
        let verified_abi = rudroid_common::il2cpp_abi::require_gc_handle_abi(&unity_version);
        let mut notify = None;
        for argc in [0, 1] {
            let Some(method) = api.method(application, c"CallLowMemory", argc) else {
                continue;
            };
            if !api.method_shape(method, argc as u32, true) {
                continue;
            }
            if !api.method_return_type(method).is_some_and(|ty| {
                api.type_kind(ty) == Some(TYPE_VOID) && api.type_byref(ty) == Some(false)
            }) {
                continue;
            }
            if argc == 1 {
                // Critical=4 is confirmed by Unity's 6000.0 reference source
                // and our exact build evidence. Unknown enum ABI is skipped;
                // the validated zero-argument callback needs no GC handle.
                if verified_abi.is_err() {
                    continue;
                }
                let param = api.method_param(method, 0);
                let usage = api.find_class(c"UnityEngine", c"ApplicationMemoryUsage");
                let (Some(param), Some(usage)) = (param, usage) else {
                    continue;
                };
                if api.class_from_type(param) != Some(usage) || api.type_byref(param) != Some(false)
                {
                    continue;
                }
            }
            notify = Some((method, argc == 1));
            break;
        }
        let collector = (|| -> Result<_, String> {
            let abi = verified_abi?;
            // SAFETY: the exact Unity build/target was verified above.
            let handles = unsafe { api.gc_handles(abi) }.ok_or("GC handle exports unavailable")?;
            let resources = api.find_class(c"UnityEngine", c"Resources");
            let operation = api.find_class(c"UnityEngine", c"AsyncOperation");
            let (Some(resources), Some(operation)) = (resources, operation) else {
                return Err("Resources/AsyncOperation metadata unavailable".into());
            };
            let unload = api.method(resources, c"UnloadUnusedAssets", 0);
            let done = api.method(operation, c"get_isDone", 0);
            let (Some(unload), Some(done)) = (unload, done) else {
                return Err("UnloadUnusedAssets/isDone method shape unavailable".into());
            };
            if !api.method_shape(unload, 0, true) || !api.method_shape(done, 0, false) {
                return Err("UnloadUnusedAssets/isDone method shape unavailable".into());
            }
            let returns = api.method_return_type(unload);
            if returns.and_then(|ty| api.class_from_type(ty)) != Some(operation)
                || returns.and_then(|ty| api.type_byref(ty)) != Some(false)
                || !api.method_signature(done, false, &[], TYPE_BOOLEAN)
            {
                return Err("UnloadUnusedAssets/isDone return type mismatch".into());
            }
            Ok(Collector {
                handles,
                unload,
                done,
            })
        })();
        let (collector, cleanup_unavailable) = match collector {
            Ok(collector) => (Some(collector), None),
            Err(reason) => (None, Some(reason)),
        };
        if notify.is_none() && collector.is_none() {
            return Err(format!(
                "no validated low-memory notification; {}",
                cleanup_unavailable.as_deref().unwrap_or_default()
            ));
        }
        Ok(Self {
            api,
            collector,
            cleanup_unavailable,
            unity_version,
            notify,
            operation: None,
            _thread: PhantomData,
        })
    }

    /// # Safety
    /// As [`Il2cpp::invoke`]: `object` and `args` match the validated method.
    unsafe fn invoke(
        &self,
        method: Method,
        object: Ptr,
        args: *mut Ptr,
    ) -> Result<Ptr, &'static str> {
        if crate::STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            return Err("player stopping");
        }
        if !self.api.thread_attached() {
            return Err("player thread detached from IL2CPP");
        }
        // SAFETY: caller contract.
        let result = unsafe { self.api.invoke(method, object, args) }
            .map_err(|()| "managed exception from reclamation method");
        // A managed callback may reenter UnityStopping. Do not root, unbox,
        // dereference or chain another managed call after cancellation.
        if crate::STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            return Err("player stopping");
        }
        result
    }

    pub fn supports_notification(&self) -> bool {
        self.notify.is_some()
    }
    pub fn supports_cleanup(&self) -> bool {
        self.collector.is_some()
    }
    pub fn cleanup_unavailable(&self) -> Option<&str> {
        self.cleanup_unavailable.as_deref()
    }
    pub fn supports_gc(&self) -> bool {
        self.api.has("il2cpp_gc_collect")
    }
    pub fn pending(&self) -> bool {
        self.operation.is_some()
    }

    pub fn notify(&self) -> Result<bool, &'static str> {
        if let Some((method, has_usage)) = self.notify {
            let mut critical = 4i32;
            let mut args = [&mut critical as *mut i32 as Ptr];
            // SAFETY: a validated static void CallLowMemory, invoked on this
            // engine's attached thread; the one-argument form takes the
            // ApplicationMemoryUsage enum by pointer (verified builds only).
            unsafe {
                self.invoke(
                    method,
                    std::ptr::null_mut(),
                    if has_usage {
                        args.as_mut_ptr()
                    } else {
                        std::ptr::null_mut()
                    },
                )
            }?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn request(&mut self) -> Result<(), &'static str> {
        if self.operation.is_some() {
            return Err("an unload operation is already pending");
        }
        let collector = self
            .collector
            .as_ref()
            .ok_or("engine cleanup unavailable")?;
        // SAFETY: a validated static, parameterless method on the attached thread.
        let operation =
            unsafe { self.invoke(collector.unload, std::ptr::null_mut(), std::ptr::null_mut()) }?;
        if operation.is_null() {
            return Err("UnloadUnusedAssets returned null");
        }
        // SAFETY: the AsyncOperation just returned is live and still
        // referenced from this attached native stack.
        let root =
            unsafe { collector.handles.root(operation) }.ok_or("could not root AsyncOperation")?;
        self.operation = Some(root);
        Ok(())
    }
    pub fn poll(&mut self) -> Result<bool, &'static str> {
        let Some(root) = &self.operation else {
            return Err("no pending unload operation");
        };
        let collector = self
            .collector
            .as_ref()
            .ok_or("engine cleanup unavailable")?;
        let operation = root.target();
        if operation.is_null() {
            return Err("rooted AsyncOperation became unavailable");
        }
        // SAFETY: the validated parameterless instance getter on the rooted,
        // live AsyncOperation, on the attached thread.
        let done = unsafe { self.invoke(collector.done, operation, std::ptr::null_mut()) }?;
        if done.is_null() {
            return Err("AsyncOperation.isDone returned null");
        }
        // SAFETY: `done` is the boxed Boolean the validated getter just returned.
        let value = unsafe { self.api.unbox(done) }
            .filter(|value| !value.is_null())
            .ok_or("AsyncOperation.isDone could not be unboxed")?;
        // SAFETY: an unboxed Boolean is one readable byte.
        let complete = unsafe { value.cast::<u8>().read() } != 0;
        if complete {
            self.release();
        }
        Ok(complete)
    }
    pub fn release(&mut self) {
        self.operation = None;
    }
    pub fn collect(&self) -> bool {
        if crate::STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        // Unity 2021/6 use a non-generational collector. Generation zero
        // covers its single generation; no nonexistent max-generation export.
        self.api.gc_collect(0)
    }
    pub fn heap_bytes(&self) -> (Option<i64>, Option<i64>) {
        if crate::STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            return (None, None);
        }
        (self.api.gc_used_size(), self.api.gc_heap_size())
    }
}

#[cfg(test)]
#[path = "../tests/unit/engine.rs"]
mod tests;
