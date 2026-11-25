//! INPUT: title opt-in, live primary GLES context and native buffer calls
//! OUTPUT: whole-store invalidation hint for verified dynamic geometry maps
//! POS: the driver retains every buffer name, allocation, VAO and draw
//!
//! The whole-range distinction is based on PortMaster-Unity glarena.c,
//! Copyright (c) 2026 wild1walker (MIT, see THIRD-PARTY-NOTICES).

#![deny(unsafe_op_in_unsafe_fn)]
use super::{active, shader_gl, Graphics};
use rudroid_common::{
    host_api::HostApiV0, logging::diagnostics_enabled, registry::RudroidModuleEvent, ModuleKind,
};
use std::ffi::{c_void, CStr};
use std::ptr::null_mut;
use std::sync::{
    atomic::{AtomicU64, Ordering::Relaxed},
    Mutex, OnceLock,
};

const ARRAY: u32 = 0x8892;
const ELEMENT_ARRAY: u32 = 0x8893;
const COPY_WRITE: u32 = 0x8f37;
const BUFFER_SIZE: u32 = 0x8764;
const BUFFER_USAGE: u32 = 0x8765;
const ACCESS_FLAGS: u32 = 0x911f;
const MAP_POINTER: u32 = 0x88bd;
const STREAM_DRAW: i32 = 0x88e0;
const DYNAMIC_DRAW: i32 = 0x88e8;
const RANGE: u32 = 0x36; // write | invalidate range | explicit flush | unsynchronized
const WHOLE: u32 = 0x3a; // same flags with invalidate buffer instead of invalidate range
const TRACKED_LIMIT: usize = 8;

static ENABLED: OnceLock<bool> = OnceLock::new();

pub(crate) fn configure(api: &HostApiV0) -> bool {
    configure_with_diagnostics(api, diagnostics_enabled())
}

fn configure_with_diagnostics(api: &HostApiV0, diagnostics: bool) -> bool {
    let enabled = diagnostics && api.config_bool(c"gpu.glesWholeBufferInvalidate", false);
    let valid = *ENABLED.get_or_init(|| enabled) == enabled;
    if valid && enabled && diagnostics_enabled()
        && !api.on_module_loaded(ModuleKind::UnityStopping, report, null_mut())
    {
        rudroid_common::diagnostic!("[GLES-WHOLE-MAP] stopping report callback unavailable");
    }
    valid
}

extern "C" fn report(_: Option<&RudroidModuleEvent>, _: *mut c_void) {
    if let Some(g) = active() {
        if let Some(maps) = &g.geometry_maps {
            maps.report();
        }
    }
}

type Map = unsafe extern "C" fn(u32, isize, isize, u32) -> *mut c_void;
type Unmap = unsafe extern "C" fn(u32) -> u8;
type Query32 = unsafe extern "C" fn(u32, u32, *mut i32);
type Query64 = unsafe extern "C" fn(u32, u32, *mut i64);
type QueryPtr = unsafe extern "C" fn(u32, u32, *mut *mut c_void);
type GetString = unsafe extern "C" fn(u32) -> *const std::ffi::c_char;

struct Driver {
    map: Map,
    map_ext: Option<Map>,
    unmap: Unmap,
    unmap_oes: Option<Unmap>,
    get: Query32,
    get64: Query64,
    pointer: QueryPtr,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Mapping {
    pointer: usize,
    group: u64,
}

#[derive(Default)]
struct Counters {
    requests: AtomicU64,
    applied: AtomicU64,
    busy: AtomicU64,
    peak: AtomicU64,
    failed: AtomicU64,
}

pub(super) struct GeometryMaps {
    driver: Driver,
    // Track native mapping identities, including shared-context and
    // alternate-target queries, without emulating or owning a GL buffer.
    // Once full, leave further maps to the driver with their original flags.
    mappings: Mutex<Vec<Mapping>>,
    counters: Option<Counters>,
}

impl GeometryMaps {
    pub(super) fn configured(resolver: impl Fn(&CStr) -> usize) -> Option<Self> {
        if !ENABLED.get().copied().unwrap_or(false) {
            return None;
        }
        let maps = Self::load(resolver);
        rudroid_common::diagnostic!("[GLES-WHOLE-MAP] requested=true supported={}", maps.is_some());
        maps
    }

    // Called with the verified SDL GLES context current, before guest execution.
    fn load(resolve: impl Fn(&CStr) -> usize) -> Option<Self> {
        macro_rules! entry {
            ($name:literal, $ty:ty) => {{
                let address = resolve($name);
                (address != 0).then(|| {
                    // SAFETY: these exact native GLES prototypes are resolved on
                    // the live owning context; driver code remains process-pinned.
                    unsafe { std::mem::transmute::<usize, $ty>(address) }
                })
            }};
        }
        let get_string = entry!(c"glGetString", GetString)?;
        let string = |key| {
            // SAFETY: The session pins this driver entry and the caller owns the current context.
            let ptr = unsafe { get_string(key) };
            // SAFETY: native GL strings are NUL-terminated and remain borrowed
            // for the lifetime of the current context.
            (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) })
        };
        if !string(0x1f02)?.to_bytes().starts_with(b"OpenGL ES 3.")
            || string(0x1f03)?.to_bytes().split(|&b| b == b' ').any(|ext| {
                matches!(ext, b"GL_EXT_direct_state_access" | b"GL_ARB_direct_state_access")
            })
        {
            return None;
        }
        let driver = Driver {
            map: entry!(c"glMapBufferRange", Map)?,
            map_ext: entry!(c"glMapBufferRangeEXT", Map),
            unmap: entry!(c"glUnmapBuffer", Unmap)?,
            unmap_oes: entry!(c"glUnmapBufferOES", Unmap),
            get: entry!(c"glGetBufferParameteriv", Query32)?,
            get64: entry!(c"glGetBufferParameteri64v", Query64)?,
            pointer: entry!(c"glGetBufferPointerv", QueryPtr)?,
        };
        Some(Self {
            driver,
            mappings: Mutex::new(Vec::with_capacity(TRACKED_LIMIT)),
            counters: diagnostics_enabled().then(Counters::default),
        })
    }

    pub(super) fn symbol(&self, name: &[u8]) -> usize {
        match name {
            b"glMapBufferRange" => map::<false> as usize,
            b"glMapBufferRangeEXT" if self.driver.map_ext.is_some() => map::<true> as usize,
            b"glUnmapBuffer" => unmap::<false> as usize,
            b"glUnmapBufferOES" if self.driver.unmap_oes.is_some() => unmap::<true> as usize,
            b"glGetBufferParameteriv" => query32 as usize,
            b"glGetBufferParameteri64v" => query64 as usize,
            _ => 0,
        }
    }

    fn mapping_count(&self) -> usize {
        self.mappings.lock().unwrap_or_else(|poison| poison.into_inner()).len()
    }

    fn identity(g: &Graphics, pointer: *mut c_void) -> Option<Mapping> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: EGL's current-context query has no preconditions.
        let context = unsafe { (g.driver.current_context)() } as usize;
        (context != 0).then(|| Mapping {
            pointer: pointer as usize,
            group: shader_gl::context_group(context),
        })
    }

    fn observed_mapping(&self, g: &Graphics, target: u32) -> Option<Mapping> {
        let mut pointer = null_mut();
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.driver.pointer)(target, MAP_POINTER, &mut pointer) };
        Self::identity(g, pointer)
    }

    fn update_mapping(&self, identity: Mapping, translated: bool) {
        let mut mappings = self.mappings.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(index) = mappings.iter().position(|&item| item == identity) {
            mappings.swap_remove(index);
        }
        // Only the current owner of the primary EGL context may add a
        // translated mapping. Other threads can only retire existing entries.
        if translated {
            mappings.push(identity);
            if let Some(counters) = &self.counters {
                counters.peak.fetch_max(mappings.len() as u64, Relaxed);
            }
        }
    }

    fn translated(&self, g: &Graphics, target: u32) -> bool {
        if let Some(mapping) = self.observed_mapping(g, target) {
            self.mappings.lock().unwrap_or_else(|poison| poison.into_inner())
                .contains(&mapping)
        } else {
            false
        }
    }

    fn record(&self, field: impl FnOnce(&Counters) -> &AtomicU64) {
        if let Some(counters) = &self.counters {
            field(counters).fetch_add(1, Relaxed);
        }
    }

    fn report(&self) {
        if let Some(c) = &self.counters {
            rudroid_common::diagnostic!(
                "[GLES-WHOLE-MAP] requests={} applied={} busy={} peak={} native_failures={}",
                c.requests.load(Relaxed), c.applied.load(Relaxed), c.busy.load(Relaxed),
                c.peak.load(Relaxed), c.failed.load(Relaxed)
            );
        }
    }
}

fn candidate(target: u32, offset: isize, len: isize, access: u32) -> bool {
    matches!(target, ARRAY | ELEMENT_ARRAY | COPY_WRITE) && offset == 0 && len > 0 && access == RANGE
}

fn whole_dynamic(len: isize, size: i64, usage: i32) -> bool {
    len as i64 == size && matches!(usage, STREAM_DRAW | DYNAMIC_DRAW)
}

extern "C" fn map<const EXT: bool>(target: u32, offset: isize, len: isize, access: u32) -> *mut c_void {
    let Some(g) = active() else { return null_mut(); };
    let Some(maps) = &g.geometry_maps else { return null_mut(); };
    let Some(native) = (if EXT { maps.driver.map_ext } else { Some(maps.driver.map) }) else {
        return null_mut();
    };
    let mut hint = access;
    let previous = maps.mapping_count();
    if candidate(target, offset, len, access) && g.primary_current() {
        maps.record(|c| &c.requests);
        if previous == TRACKED_LIMIT {
            maps.record(|c| &c.busy);
        } else {
            // The driver resolves the current bound name, including the
            // active VAO's element buffer. Query the actual store on this
            // same context immediately before mapping; offset zero alone
            // permits a partial mapping and must not invalidate its tail.
            let mut size = -1;
            // SAFETY: The query writes to the local output of the declared GLES type.
            unsafe { (maps.driver.get64)(target, BUFFER_SIZE, &mut size) };
            if len as i64 == size {
                let mut usage = -1;
                // SAFETY: The query writes to the local output of the declared GLES type.
                unsafe { (maps.driver.get)(target, BUFFER_USAGE, &mut usage) };
                if whole_dynamic(len, size, usage) {
                    hint = WHOLE;
                }
            }
        }
    }
    // SAFETY: Forward the guest arguments under this entry point’s native GLES contract.
    let pointer = unsafe { native(target, offset, len, hint) };
    if hint != access {
        if let Some(identity) = GeometryMaps::identity(&g, pointer) {
            maps.update_mapping(identity, true);
            maps.record(|c| &c.applied);
        } else {
            // The native call owns error reporting. Repeating it would clear
            // or replace an observable resource failure.
            maps.record(|c| &c.failed);
        }
    } else if previous != 0 {
        // BufferData/DeleteBuffers can implicitly unmap. A later successful
        // mapping that reuses the native pointer is always a distinct map.
        if let Some(identity) = GeometryMaps::identity(&g, pointer) {
            maps.update_mapping(identity, false);
        }
    }
    pointer
}

extern "C" fn unmap<const OES: bool>(target: u32) -> u8 {
    let Some(g) = active() else { return 0; };
    let Some(maps) = &g.geometry_maps else { return 0; };
    let Some(native) = (if OES { maps.driver.unmap_oes } else { Some(maps.driver.unmap) }) else {
        return 0;
    };
    let marker = (maps.mapping_count() != 0).then(|| maps.observed_mapping(&g, target)).flatten();
    // SAFETY: Forward the guest arguments under this entry point’s native GLES contract.
    let result = unsafe { native(target) };
    if let Some(identity) = marker {
        // Corruption and a rejected unmap both return false. Only retire the
        // former: the driver invalidates its pointer when it ends the map.
        if result != 0 || maps.observed_mapping(&g, target) != Some(identity) {
            maps.update_mapping(identity, false);
        }
    }
    result
}

macro_rules! buffer_query {
    ($name:ident, $field:ident, $int:ty) => {
        unsafe extern "C" fn $name(target: u32, pname: u32, output: *mut $int) {
            let Some(g) = active() else { return; };
            let Some(maps) = &g.geometry_maps else { return; };
            let native = maps.driver.$field;
            if pname != ACCESS_FLAGS || output.is_null() || maps.mapping_count() == 0 {
                // SAFETY: The guest supplies the writable output required by this native query.
                unsafe { native(target, pname, output) };
                return;
            }
            let mut flags = -1;
            // SAFETY: The query writes to the local output of the declared GLES type.
            unsafe { native(target, pname, &mut flags) };
            if flags == -1 {
                return; // GL leaves the output untouched on error.
            }
            if flags == WHOLE as $int && maps.translated(&g, target) {
                flags = RANGE as $int;
            }
            // SAFETY: caller supplied the output for this one-element native
            // query; the native query succeeded for the same target and pname.
            unsafe { output.write(flags) };
        }
    };
}

buffer_query!(query32, get, i32);
buffer_query!(query64, get64, i64);

#[cfg(test)]
#[path = "../tests/unit/geometry_map.rs"]
pub(crate) mod tests;
