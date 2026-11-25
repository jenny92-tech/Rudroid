//! INPUT: private opt-in and the selected guest GLES entry points
//! OUTPUT: bounded call and lookup counts at UnityStopping
//! POS: records actual calls before any geometry-policy gate; forwards exact arguments

#![deny(unsafe_op_in_unsafe_fn)]
use super::{active, geometry_alias::{Guard, Pending}, geometry_census::{Census, Use}, Graphics, DRAW};
use rudroid_common::{
    host_api::HostApiV0, logging::diagnostics_enabled, registry::RudroidModuleEvent, ModuleKind,
};
use std::collections::BTreeMap;
use std::ffi::{c_void, CStr};
use std::ptr::null_mut;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Mutex, OnceLock};

const ENV: &str = "RUDROID_DIAG_GL_GEOMETRY_ROUTE";
const CENSUS_ENV: &str = "RUDROID_DIAG_GL_OBJECT_CENSUS";
const ALIAS_ENV: &str = "RUDROID_DIAG_GL_ALIAS_GUARD";
const BUCKET_LIMIT: usize = 256;
static ENABLED: OnceLock<(bool, bool, bool)> = OnceLock::new();
#[cfg(test)]
thread_local! { static TEST_ALIAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[cfg(test)]
pub(super) fn enable_alias_fixture() { TEST_ALIAS.with(|slot| slot.set(true)); }

pub(crate) fn register(api: &HostApiV0) -> Result<(), &'static str> {
    if !diagnostics_enabled() { return Ok(()); }
    let enabled = |name| match std::env::var_os(name).as_deref() {
        None => Ok(false),
        Some(value) if value == "0" => Ok(false),
        Some(value) if value == "1" => Ok(true),
        _ => Err("GLES trace switches must be 0 or 1"),
    };
    let (route, census, alias) = (enabled(ENV)?, enabled(CENSUS_ENV)?, enabled(ALIAS_ENV)?);
    if !(route || census || alias) { return Ok(()); }
    if (census || alias) && api.config_bool(c"gpu.glesWholeBufferInvalidate", false) {
        return Err("GLES ownership diagnostics require gpu.glesWholeBufferInvalidate=false");
    }
    if !api.on_module_loaded(ModuleKind::UnityStopping, report, null_mut()) {
        return Err("could not register the GLES route stopping report");
    }
    ENABLED.set((route, census, alias)).map_err(|_| "GLES trace already registered")
}

extern "C" fn report(_: Option<&RudroidModuleEvent>, _: *mut c_void) {
    if let Some(g) = active() {
        if let Some(trace) = &g.geometry_trace {
            trace.report(g.swaps.load(Relaxed));
        }
    }
}

use super::Route;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Binding {
    NoContext,
    OtherContext,
    PrimaryNoSdlWindow,
    PrimaryOtherSdlWindow,
    PrimaryNoDraw,
    PrimarySplitSurfaces,
    Primary,
}

impl Binding {
    fn current(g: &Graphics) -> Self {
        // SAFETY: these are current-thread EGL/SDL state queries on the
        // pinned, live session; no context changes or error reads.
        unsafe {
            let context = (g.driver.current_context)() as usize;
            if context == 0 {
                return Self::NoContext;
            }
            if context != g.context {
                return Self::OtherContext;
            }
            let window = sdl2::sys::SDL_GL_GetCurrentWindow() as usize;
            if window == 0 {
                return Self::PrimaryNoSdlWindow;
            }
            if window != g.window {
                return Self::PrimaryOtherSdlWindow;
            }
            let draw = (g.driver.current_surface)(DRAW);
            if draw.is_null() {
                return Self::PrimaryNoDraw;
            }
            if draw != (g.driver.current_surface)(0x305a) {
                return Self::PrimarySplitSurfaces;
            }
            Self::Primary
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Span {
    Absent,
    Negative,
    Zero,
    Positive,
}

fn span(value: isize) -> Span {
    match value.cmp(&0) {
        std::cmp::Ordering::Less => Span::Negative,
        std::cmp::Ordering::Equal => Span::Zero,
        std::cmp::Ordering::Greater => Span::Positive,
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Arguments {
    target: u32,
    access: u32,
    offset: Span,
    length: Span,
}

impl Arguments {
    fn range(target: u32, access: u32, offset: isize, length: isize) -> Self {
        Self { target, access, offset: span(offset), length: span(length) }
    }

    fn whole(target: u32, access: u32, length: Span) -> Self {
        Self { target, access, offset: Span::Absent, length }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Key {
    operation: Op,
    binding: Binding,
    after_120_swaps: bool,
    args: Arguments,
}

#[derive(Default)]
struct Stats {
    lookups: [[[u64; 3]; 2]; Op::ROUTE_COUNT],
    calls: BTreeMap<Key, u64>,
    overflow: u64,
}

impl Stats {
    fn record(&mut self, key: Key) {
        if let Some(count) = self.calls.get_mut(&key) {
            *count = count.saturating_add(1);
        } else if self.calls.len() < BUCKET_LIMIT {
            self.calls.insert(key, 1);
        } else {
            self.overflow = self.overflow.saturating_add(1);
        }
    }
}

pub(super) struct Trace {
    originals: [OnceLock<usize>; Op::COUNT],
    stats: Mutex<Stats>,
    route: bool,
    census: Option<Census>,
    pub(super) alias: Option<Guard>,
}

impl Trace {
    pub(super) fn configured(resolve: impl Fn(&CStr) -> usize) -> Option<Self> {
        let flags = ENABLED.get().copied();
        #[cfg(test)]
        let flags = flags.or_else(|| TEST_ALIAS.with(|slot| slot.replace(false)).then_some((false, false, true)));
        let (route, census, alias) = flags?;
        let mut trace = Self::new();
        trace.route = route;
        trace.census = census.then(|| Census::load(&resolve)).flatten();
        trace.alias = alias.then(|| Guard::load(&resolve)).flatten();
        if route { rudroid_common::diagnostic!("[GLES-ROUTE] armed; report at UnityStopping"); }
        if census { rudroid_common::diagnostic!("[GLES-CENSUS] requested=true supported={}", trace.census.is_some()); }
        if alias { rudroid_common::diagnostic!("[GLES-ALIAS] requested=true supported={}", trace.alias.is_some()); }
        (trace.route || trace.census.is_some() || trace.alias.is_some()).then_some(trace)
    }

    pub(super) fn new() -> Self {
        Self { originals: std::array::from_fn(|_| OnceLock::new()), stats: Mutex::default(), route: true, census: None, alias: None }
    }

    pub(super) fn resolve(&self, key: &[u8], route: Route, address: usize) -> usize {
        let Some(op) = Op::for_name(key).filter(|op| self.census.is_some() || op.route() || self.alias.is_some() && op.alias()) else {
            if let Some(census) = &self.census { census.raw_lookup(key, route, address); }
            return self.raw(address);
        };
        // A census trampoline alone does not close a consumer. Only the
        // explicitly guarded alias/lifetime entries can precede owned use.
        if self.alias.is_some() && !op.alias() && self.raw(address) != address { return 0; }
        let (result, status) = if address == 0 {
            (0, 0) // missing
        } else if *self.originals[op as usize].get_or_init(|| address) == address {
            (op.wrapper(), 1) // instrumented
        } else {
            (address, 2) // honor a distinct resolution without substituting the first pointer
        };
        if self.route && op.route() {
            let mut stats = self.stats.lock().unwrap_or_else(|poison| poison.into_inner());
            let count = &mut stats.lookups[op as usize][route as usize][status];
            *count = count.saturating_add(1);
        }
        if let Some(census) = &self.census { census.lookup(op, route, status); }
        if status == 1 { result } else { self.raw(result) }
    }

    fn raw(&self, address: usize) -> usize {
        if let (Some(alias), Some(g)) = (&self.alias, active()) {
            if !alias.before_raw(&g) { return 0; }
        }
        address
    }

    pub(super) fn before_shared_context(&self, g: &Graphics) -> bool {
        self.alias.as_ref().map_or(true, |alias| alias.before_raw(g))
    }

    fn record(&self, g: &Graphics, operation: Op, args: Arguments) {
        if !self.route || !operation.route() { return; }
        let key = Key {
            operation,
            binding: Binding::current(g),
            after_120_swaps: g.swaps.load(Relaxed) >= 120,
            args,
        };
        self.stats.lock().unwrap_or_else(|poison| poison.into_inner()).record(key);
    }

    fn report(&self, swaps: u64) {
        if let Some(alias) = &self.alias { alias.report(); }
        if let Some(census) = &self.census { census.report(swaps); }
        if !self.route { return; }
        let stats = self.stats.lock().unwrap_or_else(|poison| poison.into_inner());
        rudroid_common::diagnostic!("[GLES-ROUTE] swaps={swaps} buckets={} overflow={}", stats.calls.len(), stats.overflow);
        for &op in &Op::ALL[..Op::ROUTE_COUNT] {
            for route in [Route::ElfSource, Route::EglProc] {
                let [missing, wrapped, other_address] = stats.lookups[op as usize][route as usize];
                if missing != 0 || wrapped != 0 || other_address != 0 {
                    rudroid_common::diagnostic!(
                        "[GLES-ROUTE] lookup op={} route={route:?} missing={missing} wrapped={wrapped} other_address={other_address}",
                        op.name()
                    );
                }
            }
        }
        for (key, count) in &stats.calls {
            rudroid_common::diagnostic!(
                "[GLES-ROUTE] call op={} target={:#x} access={:#x} offset={:?} length={:?} binding={:?} warm={} count={count}",
                key.operation.name(), key.args.target, key.args.access, key.args.offset,
                key.args.length, key.binding, key.after_120_swaps
            );
        }
    }
}

macro_rules! probes {
    ($($op:ident $wrapper:ident $name:literal ($($arg:ident : $ty:ty),*) -> $ret:ty,
        $fallback:expr, $args:expr, $usage:expr;)*) => {
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
        #[repr(usize)]
        pub(super) enum Op { $($op,)* }

        impl Op {
            const ALL: &'static [Self] = &[$(Self::$op,)*];
            const COUNT: usize = Self::ALL.len();
            const ROUTE_COUNT: usize = Self::DrawArraysInstanced as usize + 1;

            fn for_name(name: &[u8]) -> Option<Self> {
                $(if name == $name.as_bytes() { return Some(Self::$op); })*
                None
            }

            pub(super) fn name(self) -> &'static str {
                match self { $(Self::$op => $name,)* }
            }

            fn wrapper(self) -> usize {
                match self { $(Self::$op => $wrapper as usize,)* }
            }

            fn route(self) -> bool { self <= Self::DrawArraysInstanced }

            fn alias(self) -> bool {
                matches!(self, Self::BindBuffer | Self::DeleteBuffers) || self >= Self::TexBuffer
            }
        }

        $(unsafe extern "C" fn $wrapper($($arg: $ty),*) -> $ret {
            let Some(g) = active() else { return $fallback; };
            let Some(trace) = &g.geometry_trace else { return $fallback; };
            let Some(&address) = trace.originals[Op::$op as usize].get() else { return $fallback; };
            trace.record(&g, Op::$op, $args);
            let usage = $usage;
            let pending = match (&trace.alias, usage) {
                (Some(alias), Use::Alias(target, buffer)) => alias.before_alias(&g, target, buffer),
                (Some(alias), Use::Delete(count, address)) => alias.before_delete(&g, count, address),
                (Some(alias), Use::Bind(_, buffer)) => alias.before_bind(&g, buffer).then_some(Pending::Native),
                _ => Some(Pending::Native),
            };
            let Some(pending) = pending else { return $fallback; };
            if let Some(census) = &trace.census {
                if !usage.post() { census.record(&g, Op::$op, usage); }
            }
            // SAFETY: this slot contains the selected entry of this exact
            // symbol and C prototype, pinned through the guest session.
            let native = unsafe { std::mem::transmute::<usize, unsafe extern "C" fn($($ty),*) -> $ret>(address) };
            // SAFETY: Forward the guest arguments under this entry point’s native GLES contract.
            let result = unsafe { native($($arg),*) };
            if let Some(alias) = &trace.alias {
                if let Use::Bind(target, buffer) = usage { alias.bound(&g, target, buffer); }
                alias.done(pending);
            }
            if let Some(census) = &trace.census {
                if usage.post() { census.record(&g, Op::$op, usage); }
            }
            result
        })*
    };
}

probes! {
    MapRange map_range "glMapBufferRange" (t: u32, o: isize, n: isize, a: u32) -> *mut c_void,
        null_mut(), Arguments::range(t, a, o, n), Use::Map(t, a, o, n);
    MapRangeExt map_range_ext "glMapBufferRangeEXT" (t: u32, o: isize, n: isize, a: u32) -> *mut c_void,
        null_mut(), Arguments::range(t, a, o, n), Use::Map(t, a, o, n);
    MapWhole map_whole "glMapBuffer" (t: u32, a: u32) -> *mut c_void,
        null_mut(), Arguments::whole(t, a, Span::Absent), Use::Target(t);
    MapOes map_oes "glMapBufferOES" (t: u32, a: u32) -> *mut c_void,
        null_mut(), Arguments::whole(t, a, Span::Absent), Use::Target(t);
    BufferData buffer_data "glBufferData" (t: u32, n: isize, p: *const c_void, u: u32) -> (),
        (), Arguments::whole(t, u, span(n)), Use::Target(t);
    BufferSubData buffer_sub_data "glBufferSubData" (t: u32, o: isize, n: isize, p: *const c_void) -> (),
        (), Arguments::range(t, 0, o, n), Use::Target(t);
    DrawElements draw_elements "glDrawElements" (m: u32, n: i32, t: u32, p: *const c_void) -> (),
        (), Arguments::whole(m, t, span(n as isize)), Use::Draw(true);
    DrawRangeElements draw_range_elements "glDrawRangeElements"
        (m: u32, a: u32, b: u32, n: i32, t: u32, p: *const c_void) -> (),
        (), Arguments::whole(m, t, span(n as isize)), Use::Draw(true);
    DrawElementsInstanced draw_elements_instanced "glDrawElementsInstanced"
        (m: u32, n: i32, t: u32, p: *const c_void, instances: i32) -> (),
        (), Arguments::whole(m, t, span(n as isize)), Use::Draw(true);
    DrawArrays draw_arrays "glDrawArrays" (m: u32, first: i32, n: i32) -> (),
        (), Arguments::range(m, 0, first as isize, n as isize), Use::Draw(false);
    DrawArraysInstanced draw_arrays_instanced "glDrawArraysInstanced"
        (m: u32, first: i32, n: i32, instances: i32) -> (),
        (), Arguments::range(m, 0, first as isize, n as isize), Use::Draw(false);

    BindBuffer bind_buffer "glBindBuffer" (t: u32, b: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Bind(t, b);
    BindVertexArray bind_vao "glBindVertexArray" (v: u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Vao;
    BindVertexArrayOes bind_vao_oes "glBindVertexArrayOES" (v: u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Vao;
    GenBuffers gen_buffers "glGenBuffers" (n: i32, p: *mut u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Lifecycle(0);
    DeleteBuffers delete_buffers "glDeleteBuffers" (n: i32, p: *const u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Delete(n, p as usize);
    GenVertexArrays gen_vaos "glGenVertexArrays" (n: i32, p: *mut u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Lifecycle(0);
    DeleteVertexArrays delete_vaos "glDeleteVertexArrays" (n: i32, p: *const u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Lifecycle(if n > 0 { 2 } else { 0 });
    GenVertexArraysOes gen_vaos_oes "glGenVertexArraysOES" (n: i32, p: *mut u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Lifecycle(0);
    DeleteVertexArraysOes delete_vaos_oes "glDeleteVertexArraysOES" (n: i32, p: *const u32) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Lifecycle(if n > 0 { 2 } else { 0 });
    VertexAttribPointer vertex_attrib_pointer "glVertexAttribPointer"
        (i: u32, n: i32, t: u32, normalize: u8, stride: i32, p: *const c_void) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Attrib(i);
    VertexAttribIPointer vertex_attrib_i_pointer "glVertexAttribIPointer"
        (i: u32, n: i32, t: u32, stride: i32, p: *const c_void) -> (),
        (), Arguments::whole(0, 0, Span::Absent), Use::Attrib(i);
    FlushMappedBufferRange flush_mapped "glFlushMappedBufferRange" (t: u32, o: isize, n: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    FlushMappedBufferRangeExt flush_mapped_ext "glFlushMappedBufferRangeEXT" (t: u32, o: isize, n: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    UnmapBuffer unmap_buffer "glUnmapBuffer" (t: u32) -> u8,
        0, Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    UnmapBufferOes unmap_buffer_oes "glUnmapBufferOES" (t: u32) -> u8,
        0, Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    CopyBufferSubData copy_buffer_sub_data "glCopyBufferSubData" (r: u32, w: u32, a: isize, b: isize, n: isize) -> (),
        (), Arguments::whole(r, 0, Span::Absent), Use::Copy(r, w);
    BindBufferBase bind_buffer_base "glBindBufferBase" (t: u32, i: u32, b: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Bound(t);
    BindBufferRange bind_buffer_range "glBindBufferRange" (t: u32, i: u32, b: u32, o: isize, n: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Bound(t);
    BufferStorageExt buffer_storage_ext "glBufferStorageEXT" (t: u32, n: isize, p: *const c_void, a: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    GetBufferParameter get_buffer_parameter "glGetBufferParameteriv" (t: u32, k: u32, p: *mut i32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    GetBufferParameter64 get_buffer_parameter64 "glGetBufferParameteri64v" (t: u32, k: u32, p: *mut i64) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    GetBufferPointer get_buffer_pointer "glGetBufferPointerv" (t: u32, k: u32, p: *mut *mut c_void) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Target(t);
    DrawElementsBaseVertex draw_elements_base "glDrawElementsBaseVertex"
        (m: u32, n: i32, t: u32, p: *const c_void, b: i32) -> (),
        (), Arguments::whole(m, t, Span::Absent), Use::Draw(true);
    DrawElementsBaseVertexExt draw_elements_base_ext "glDrawElementsBaseVertexEXT"
        (m: u32, n: i32, t: u32, p: *const c_void, b: i32) -> (),
        (), Arguments::whole(m, t, Span::Absent), Use::Draw(true);
    DrawElementsBaseVertexOes draw_elements_base_oes "glDrawElementsBaseVertexOES"
        (m: u32, n: i32, t: u32, p: *const c_void, b: i32) -> (),
        (), Arguments::whole(m, t, Span::Absent), Use::Draw(true);
    DrawElementsIndirect draw_elements_indirect "glDrawElementsIndirect"
        (m: u32, t: u32, p: *const c_void) -> (),
        (), Arguments::whole(m, t, Span::Absent), Use::Indirect(true);
    DrawArraysIndirect draw_arrays_indirect "glDrawArraysIndirect" (m: u32, p: *const c_void) -> (),
        (), Arguments::whole(m, 0, Span::Absent), Use::Indirect(false);
    TexBuffer tex_buffer "glTexBuffer" (t: u32, format: u32, buffer: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
    TexBufferRange tex_buffer_range "glTexBufferRange" (t: u32, format: u32, buffer: u32, offset: isize, size: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
    TexBufferExt tex_buffer_ext "glTexBufferEXT" (t: u32, format: u32, buffer: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
    TexBufferRangeExt tex_buffer_range_ext "glTexBufferRangeEXT" (t: u32, format: u32, buffer: u32, offset: isize, size: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
    TexBufferOes tex_buffer_oes "glTexBufferOES" (t: u32, format: u32, buffer: u32) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
    TexBufferRangeOes tex_buffer_range_oes "glTexBufferRangeOES" (t: u32, format: u32, buffer: u32, offset: isize, size: isize) -> (),
        (), Arguments::whole(t, 0, Span::Absent), Use::Alias(t, buffer);
}

#[cfg(test)]
#[path = "../tests/unit/geometry_trace.rs"]
mod tests;
