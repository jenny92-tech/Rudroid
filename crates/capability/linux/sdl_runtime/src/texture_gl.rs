//! INPUT:  current pinned GLES driver, guest upload arguments and safe texture plans
//! OUTPUT:  texture allocation/upload facades, PBO read leases and scoped metadata cleanup
//! POS:    sole native texture boundary; no pointers retained after synchronous driver calls
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
use super::super::{
    texture_policy::{self as policy, Layout, Param, Plan, Unpack},
    texture_state::State,
};
use super::{active, Graphics};
use std::{
    ffi::{c_void, CStr},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};

const TEX: u32 = 0x0de1;
const PBO: u32 = 0x88ec;
type Image = extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
type Sub = extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
type Storage = extern "C" fn(u32, i32, u32, i32, i32);
type Compressed = extern "C" fn(u32, i32, u32, i32, i32, i32, i32, *const c_void);
type CompressedSub = extern "C" fn(u32, i32, i32, i32, i32, i32, u32, i32, *const c_void);
struct Functions {
    get: extern "C" fn(u32, *mut i32),
    string: extern "C" fn(u32) -> *const u8,
    error: extern "C" fn() -> u32,
    pack: extern "C" fn(u32, i32),
    bind: extern "C" fn(u32, u32),
    image: Image,
    sub: Sub,
    storage: Option<Storage>,
    storage_ext: Option<Storage>,
    compressed: Compressed,
    compressed_sub: CompressedSub,
    delete: extern "C" fn(i32, *const u32),
    copy: extern "C" fn(u32, i32, u32, i32, i32, i32, i32, i32),
    parameter_i: extern "C" fn(u32, u32, i32),
    parameter_f: extern "C" fn(u32, u32, f32),
    parameter_iv: extern "C" fn(u32, u32, *const i32),
    parameter_fv: extern "C" fn(u32, u32, *const f32),
    query_i: extern "C" fn(u32, u32, *mut i32),
    query_f: extern "C" fn(u32, u32, *mut f32),
    map: Option<extern "C" fn(u32, isize, isize, u32) -> *const c_void>,
    unmap: Option<extern "C" fn(u32) -> u8>,
    buffer: Option<extern "C" fn(u32, u32, *mut i32)>,
}
static FUNCTIONS: OnceLock<Option<Functions>> = OnceLock::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
static SCALES: AtomicU64 = AtomicU64::new(0);
static BYPASSES: AtomicU64 = AtomicU64::new(0);
static BEFORE: AtomicU64 = AtomicU64::new(0);
static AFTER: AtomicU64 = AtomicU64::new(0);
static UPLOAD_BEFORE: AtomicU64 = AtomicU64::new(0);
static UPLOAD_AFTER: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
    f(s.get_or_insert_with(Default::default))
}
pub(super) fn retire(group: u64) {
    state(|s| s.retire(group));
}
pub(super) fn report() {
    if policy::policy().stats {
        eprintln!("[GLES] texture caps: scales={} allocation={}->{} uploads={}->{} dropped_mips={} bypasses={}",SCALES.load(Ordering::Relaxed),BEFORE.load(Ordering::Relaxed),AFTER.load(Ordering::Relaxed),UPLOAD_BEFORE.load(Ordering::Relaxed),UPLOAD_AFTER.load(Ordering::Relaxed),DROPPED.load(Ordering::Relaxed),BYPASSES.load(Ordering::Relaxed));
    }
}
fn bypass() {
    let n = BYPASSES.fetch_add(1, Ordering::Relaxed) + 1;
    // A driver fallback can be normal. Bound evidence for the whole process,
    // rather than adding recurring upload-path I/O during gameplay.
    if n <= 3 {
        eprintln!("[GLES] texture resize bypasses={n}; forwarding original upload/error");
    }
}
pub(super) fn symbol(name: &[u8]) -> usize {
    if !policy::enabled() {
        return 0;
    }
    match name {
        b"glTexStorage2D" => storage as usize,
        b"glTexStorage2DEXT" => storage_ext as usize,
        b"glTexImage2D" => image as usize,
        b"glTexSubImage2D" => sub_image as usize,
        b"glCompressedTexImage2D" => compressed as usize,
        b"glCompressedTexSubImage2D" => compressed_sub as usize,
        b"glDeleteTextures" => delete as usize,
        b"glCopyTexImage2D" => copy_image as usize,
        b"glTexParameteri" => parameter_i as usize,
        b"glTexParameterf" => parameter_f as usize,
        b"glTexParameteriv" => parameter_iv as usize,
        b"glTexParameterfv" => parameter_fv as usize,
        b"glGetTexParameteriv" => query_i as usize,
        b"glGetTexParameterfv" => query_f as usize,
        _ => 0,
    }
}
/// # Safety
/// A caller-owned current context and the process-pinned native driver must be live.
unsafe fn functions(g: &Graphics) -> Option<&'static Functions> {
    FUNCTIONS.get_or_init(|| {
        macro_rules! entry {($name:expr,$ty:ty)=>{{let p=g.native_address($name);if p==0 {return None;}
            // SAFETY: exact private GLES prototype; Graphics pins the resolved driver code.
            unsafe {std::mem::transmute::<usize,$ty>(p)}
        }};}
        macro_rules! optional {($name:expr,$ty:ty)=>{{let p=g.native_address($name);if p==0 {None} else {
            // SAFETY: exact optional GLES prototype; resolved code stays process-pinned.
            Some(unsafe {std::mem::transmute::<usize,$ty>(p)})
        }}};}
        Some(Functions {
            get:entry!(c"glGetIntegerv",extern "C" fn(u32,*mut i32)),string:entry!(c"glGetString",extern "C" fn(u32)->*const u8),
            error:entry!(c"glGetError",extern "C" fn()->u32),pack:entry!(c"glPixelStorei",extern "C" fn(u32,i32)),bind:entry!(c"glBindBuffer",extern "C" fn(u32,u32)),
            image:entry!(c"glTexImage2D",Image),sub:entry!(c"glTexSubImage2D",Sub),
            storage:optional!(c"glTexStorage2D",Storage),storage_ext:optional!(c"glTexStorage2DEXT",Storage),
            compressed:entry!(c"glCompressedTexImage2D",Compressed),compressed_sub:entry!(c"glCompressedTexSubImage2D",CompressedSub),
            delete:entry!(c"glDeleteTextures",extern "C" fn(i32,*const u32)),
            copy:entry!(c"glCopyTexImage2D",extern "C" fn(u32,i32,u32,i32,i32,i32,i32,i32)),
            parameter_i:entry!(c"glTexParameteri",extern "C" fn(u32,u32,i32)),parameter_f:entry!(c"glTexParameterf",extern "C" fn(u32,u32,f32)),
            parameter_iv:entry!(c"glTexParameteriv",extern "C" fn(u32,u32,*const i32)),parameter_fv:entry!(c"glTexParameterfv",extern "C" fn(u32,u32,*const f32)),
            query_i:entry!(c"glGetTexParameteriv",extern "C" fn(u32,u32,*mut i32)),query_f:entry!(c"glGetTexParameterfv",extern "C" fn(u32,u32,*mut f32)),
            map:optional!(c"glMapBufferRange",extern "C" fn(u32,isize,isize,u32)->*const c_void),
            unmap:optional!(c"glUnmapBuffer",extern "C" fn(u32)->u8),buffer:optional!(c"glGetBufferParameteriv",extern "C" fn(u32,u32,*mut i32)),
        })
    }).as_ref()
}
struct Call {
    g: Arc<Graphics>,
    f: &'static Functions,
    context: usize,
    key: (u64, u32),
}
struct Reservation {
    key: (u64, u32),
    live: bool,
}
impl Reservation {
    fn new(key: (u64, u32)) -> Option<Self> {
        state(|s| s.reserve(key)).then_some(Self { key, live: true })
    }
    fn commit(mut self, plan: Plan, parameters: Option<[Param; 4]>) {
        state(|s| {
            s.release(self.key);
            s.set(self.key, Some(plan));
            if let Some(p) = parameters {
                s.set_parameters(self.key, p);
            }
        });
        self.live = false;
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.live {
            state(|s| s.release(self.key));
        }
    }
}
impl Call {
    /// # Safety
    /// Enter from a native GLES wrapper on the caller's owning current context.
    unsafe fn enter(target: u32) -> Option<Self> {
        let g = active()?;
        // SAFETY: caller's GLES context contract and process-pinned driver resolution.
        let f = unsafe { functions(&g) }?;
        let context = unsafe { (g.driver.current_context)() } as usize;
        if context == 0 {
            return None;
        }
        let mut name = 0;
        if target == TEX {
            (f.get)(0x8069, &mut name);
        }
        let group = super::shader_gl::context_group(context);
        Some(Self {
            g,
            f,
            context,
            key: (group, name as u32),
        })
    }
    fn preserve_errors(&self) -> bool {
        let mut good = true;
        for _ in 0..16 {
            let e = (self.f.error)();
            if e == 0 {
                break;
            }
            good = false;
            self.g.remember_gl_error(self.context, e);
        }
        good
    }
    fn checked(&self, run: impl FnOnce()) -> bool {
        self.preserve_errors();
        run();
        self.preserve_errors()
    }
    fn plan(&self) -> Option<Plan> {
        state(|s| s.get(self.key))
    }
    fn clear(&self) {
        let parameters = state(|s| s.parameters(self.key));
        self.checked(|| {
            if let Some(values) = parameters {
                for (key, v) in policy::PARAMETERS.into_iter().zip(values) {
                    if matches!(key, 0x813c | 0x813d) {
                        (self.f.parameter_i)(TEX, key, v.integer());
                    } else {
                        (self.f.parameter_f)(TEX, key, v.float());
                    }
                }
            }
        });
        state(|s| s.set(self.key, None));
    }
    /// # Safety
    /// Native context current; only ES3 owns these four scalar texture parameters.
    unsafe fn parameters(&self) -> Option<[Param; 4]> {
        if let Some(p) = state(|s| s.parameters(self.key)) {
            return Some(p);
        }
        let raw = (self.f.string)(0x1f02);
        // SAFETY: live driver-owned string is inspected synchronously, with no retained pointer.
        if raw.is_null()
            || !unsafe { CStr::from_ptr(raw.cast()) }
                .to_bytes()
                .windows(11)
                .any(|s| s == b"OpenGL ES 3")
        {
            return None;
        }
        let mut values = [Param::Int(0); 4];
        for (key, out) in policy::PARAMETERS.into_iter().zip(&mut values) {
            if matches!(key, 0x813c | 0x813d) {
                let mut v = 0;
                (self.f.query_i)(TEX, key, &mut v);
                *out = Param::Int(v);
            } else {
                let mut v = 0.0;
                (self.f.query_f)(TEX, key, &mut v);
                *out = Param::Float(v);
            }
        }
        Some(values)
    }
    fn apply_parameters(&self, plan: Plan, parameters: Option<[Param; 4]>) {
        if let Some(values) = parameters {
            for (key, v) in policy::PARAMETERS.into_iter().zip(values) {
                if matches!(key, 0x813c | 0x813d) {
                    (self.f.parameter_i)(TEX, key, policy::parameter(plan, key, v).integer());
                } else {
                    (self.f.parameter_f)(TEX, key, policy::parameter(plan, key, v).float());
                }
            }
        }
    }
    /// # Safety
    /// Native driver strings are borrowed only while the caller's context remains current.
    unsafe fn unpack(&self) -> Unpack {
        let q = |k| {
            let mut v = 0;
            (self.f.get)(k, &mut v);
            v
        };
        let raw = (self.f.string)(0x1f02);
        // SAFETY: live driver-owned NUL-terminated string; never retained.
        let es3 = !raw.is_null()
            && unsafe { CStr::from_ptr(raw.cast()) }
                .to_bytes()
                .windows(11)
                .any(|s| s == b"OpenGL ES 3");
        let raw = if es3 {
            std::ptr::null()
        } else {
            (self.f.string)(0x1f03)
        };
        // SAFETY: same live driver string contract; extension words are inspected synchronously.
        let ext = if raw.is_null() {
            ""
        } else {
            unsafe { CStr::from_ptr(raw.cast()) }.to_str().unwrap_or("")
        };
        let row_controls = es3
            || ext
                .split_ascii_whitespace()
                .any(|s| s == "GL_EXT_unpack_subimage");
        let has_pbo = es3
            || ext
                .split_ascii_whitespace()
                .any(|s| s == "GL_NV_pixel_buffer_object");
        Unpack {
            alignment: q(0x0cf5),
            row: if row_controls { q(0x0cf2) } else { 0 },
            rows: if row_controls { q(0x0cf3) } else { 0 },
            pixels: if row_controls { q(0x0cf4) } else { 0 },
            buffer: if has_pbo { q(0x88ef) } else { 0 },
            row_controls,
        }
    }
    /// # Safety
    /// CPU data covers the checked unpack span. With a bound PBO, data is an offset,
    /// never a CPU address. The checked read-only native mapping owns the borrowed span.
    unsafe fn pixels(
        &self,
        data: *const c_void,
        w: i32,
        h: i32,
        nw: i32,
        nh: i32,
        format: u32,
        kind: u32,
        u: Unpack,
    ) -> Option<Vec<u8>> {
        let l = Layout::new(w, h, format, kind, u)?;
        let run =
            |source: &[u8]| policy::resample(source, l, w, h, nw, nh, policy::policy().filter);
        let result = if u.buffer == 0 {
            if data.is_null() || (data as usize).checked_add(l.span).is_none() {
                return None;
            }
            // SAFETY: caller's readable CPU upload span, checked length <=128MiB; no slice escapes.
            run(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), l.span) })
        } else {
            let (map, unmap, query) = (self.f.map?, self.f.unmap?, self.f.buffer?);
            let mut size = 0;
            let mut mapped = 0;
            query(PBO, 0x8764, &mut size);
            query(PBO, 0x88bc, &mut mapped);
            let offset = data as usize;
            let component = match kind {
                0x1403 | 0x140b | 0x8363 | 0x8033 | 0x8034 => 2,
                0x1405 | 0x1406 | 0x8368 => 4,
                _ => 1,
            };
            if offset % component != 0 {
                return None;
            }
            if mapped != 0
                || size < 0
                || offset.checked_add(l.span)? > size as usize
                || offset > isize::MAX as usize
            {
                return None;
            }
            let pointer = map(PBO, offset as isize, l.span as isize, 1);
            if pointer.is_null() {
                return None;
            }
            struct Mapping {
                unmap: extern "C" fn(u32) -> u8,
                live: bool,
            }
            impl Drop for Mapping {
                fn drop(&mut self) {
                    if self.live {
                        (self.unmap)(PBO);
                    }
                }
            }
            let mut lease = Mapping { unmap, live: true };
            // SAFETY: successful bounded READ mapping, kept mapped until resampling finishes.
            let out = run(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), l.span) });
            let valid = unmap(PBO) != 0;
            lease.live = false;
            if !valid {
                return None;
            }
            out
        };
        if let Some(out) = &result {
            UPLOAD_BEFORE.fetch_add((w as u64) * (h as u64) * (l.bpp as u64), Ordering::Relaxed);
            UPLOAD_AFTER.fetch_add(out.len() as u64, Ordering::Relaxed);
        }
        result
    }
    fn tight(&self, u: Unpack, run: impl FnOnce()) -> bool {
        // A CPU upload must not be interpreted as a PBO offset. Restore every changed binding/state.
        struct Restore<'a> {
            f: &'a Functions,
            u: Unpack,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if self.u.row_controls {
                    for (k, v) in [
                        (0x0cf2, self.u.row),
                        (0x0cf3, self.u.rows),
                        (0x0cf4, self.u.pixels),
                    ] {
                        (self.f.pack)(k, v);
                    }
                }
                (self.f.pack)(0x0cf5, self.u.alignment);
                if self.u.buffer != 0 {
                    (self.f.bind)(PBO, self.u.buffer as u32);
                }
            }
        }
        self.preserve_errors();
        let restore = Restore { f: self.f, u };
        if u.buffer != 0 {
            (self.f.bind)(PBO, 0);
        }
        (self.f.pack)(0x0cf5, 1);
        if u.row_controls {
            for k in [0x0cf2, 0x0cf3, 0x0cf4] {
                (self.f.pack)(k, 0);
            }
        }
        run();
        let ok = self.preserve_errors();
        drop(restore);
        ok && self.preserve_errors()
    }
}
fn record(p: Plan) {
    let n = SCALES.fetch_add(1, Ordering::Relaxed) + 1;
    BEFORE.fetch_add(p.bytes(true), Ordering::Relaxed);
    AFTER.fetch_add(p.bytes(false), Ordering::Relaxed);
    if n == 1 || (policy::policy().stats && n % 128 == 0) {
        eprintln!(
            "[GLES] texture cap {:?}->{:?} levels={} bias={}",
            p.original, p.size, p.levels, p.bias
        );
    }
}
/// # Safety
/// Corresponding GLES allocation contract; current context belongs to this thread.
unsafe fn allocate(target: u32, levels: i32, format: u32, w: i32, h: i32, ext: bool) {
    // SAFETY: forwarded current-context contract, no guest pointers are inspected.
    let Some(c) = (unsafe { Call::enter(target) }) else {
        return;
    };
    let Some(call) = (if ext {
        c.f.storage_ext.or(c.f.storage)
    } else {
        c.f.storage
    }) else {
        return;
    };
    let plan = if target == TEX && state(|s| s.room(c.key)) {
        policy::policy().plan(format, w, h, levels, true, c.g.dimensions)
    } else {
        None
    };
    let reservation = plan.and_then(|_| Reservation::new(c.key));
    let plan = plan.filter(|_| reservation.is_some());
    // SAFETY: current context; owned query buffers and preserved previous guest parameter values.
    let parameters = if plan.is_some() {
        unsafe { c.parameters() }
    } else {
        None
    };
    let (nw, nh, nl) = plan.map_or((w, h, levels), |p| (p.size.0, p.size.1, p.levels));
    // Failed/repeated immutable allocation never replaces a working plan.
    if c.checked(|| call(target, nl, format, nw, nh)) {
        if let (Some(p), Some(r)) = (plan, reservation) {
            c.checked(|| c.apply_parameters(p, parameters));
            r.commit(p, parameters);
            record(p);
        } else {
            c.clear();
        }
    }
}
/// # Safety
/// Exact glTexStorage2D contract.
unsafe extern "C" fn storage(t: u32, l: i32, f: u32, w: i32, h: i32) {
    // SAFETY: native wrapper forwards the caller's allocation contract.
    unsafe {
        allocate(t, l, f, w, h, false);
    }
}
/// # Safety
/// Exact glTexStorage2DEXT contract.
unsafe extern "C" fn storage_ext(t: u32, l: i32, f: u32, w: i32, h: i32) {
    // SAFETY: identical EXT allocation prototype.
    unsafe {
        allocate(t, l, f, w, h, true);
    }
}
/// # Safety
/// Native CPU pixels satisfy checked unpack bounds, or are a PBO offset; context is current.
unsafe extern "C" fn image(
    t: u32,
    l: i32,
    internal: i32,
    w: i32,
    h: i32,
    b: i32,
    format: u32,
    kind: u32,
    data: *const c_void,
) {
    // SAFETY: only this boundary reads caller CPU pixels; native calls copy synchronously.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        let call = c.f.image;
        let old = c.plan();
        let u = c.unpack();
        let p = if t != TEX || l < 0 || b != 0 || old.is_some_and(|p| p.immutable) {
            None
        } else if l == 0 && state(|s| s.room(c.key)) {
            policy::policy()
                .plan(internal as u32, w, h, 1, false, c.g.dimensions)
                .map(|mut p| {
                    p.levels = policy::mip_count(p.size.0, p.size.1);
                    p
                })
        } else {
            old
        };
        let reservation = if l == 0 {
            p.and_then(|_| Reservation::new(c.key))
        } else {
            None
        };
        let p = p.filter(|_| l != 0 || reservation.is_some());
        let Some(p) = p else {
            if c.checked(|| call(t, l, internal, w, h, b, format, kind, data)) && t == TEX && l == 0
            {
                c.clear();
            }
            return;
        };
        let parameters = if l == 0 { c.parameters() } else { None };
        let Some((nw, nh)) = p.level_size(l) else {
            if p.old_size(l) == Some((w, h)) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            c.checked(|| call(t, l, internal, w, h, b, format, kind, data));
            return;
        };
        if l != 0 && p.old_size(l) != Some((w, h)) {
            c.checked(|| call(t, l, internal, w, h, b, format, kind, data));
            return;
        }
        if !policy::upload_compatible(internal as u32, format, kind) {
            bypass();
            if c.checked(|| call(t, l, internal, w, h, b, format, kind, data)) && l == 0 {
                c.clear();
            }
            return;
        }
        let bytes = if data.is_null() && u.buffer == 0 {
            None
        } else {
            match c.pixels(data, w, h, nw, nh, format, kind, u) {
                Some(v) => Some(v),
                None => {
                    bypass();
                    if c.checked(|| call(t, l, internal, w, h, b, format, kind, data)) && l == 0 {
                        c.clear();
                    }
                    return;
                }
            }
        };
        let ok = if let Some(v) = bytes {
            c.tight(u, || {
                call(t, l, internal, nw, nh, b, format, kind, v.as_ptr().cast())
            })
        } else {
            c.checked(|| call(t, l, internal, nw, nh, b, format, kind, data))
        };
        if ok && l == 0 {
            c.checked(|| c.apply_parameters(p, parameters));
            reservation
                .expect("base image reservation")
                .commit(p, parameters);
            record(p);
        }
    }
}
/// # Safety
/// Exact native subimage pointer/current-context contract, including PBO offsets.
unsafe extern "C" fn sub_image(
    t: u32,
    l: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    format: u32,
    kind: u32,
    data: *const c_void,
) {
    // SAFETY: checked CPU/PBO span exists only until synchronous owned upload completes.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        let call = c.f.sub;
        let Some(p) = c.plan().filter(|p| p.bias == 0 && t == TEX) else {
            call(t, l, x, y, w, h, format, kind, data);
            return;
        };
        if !policy::upload_compatible(p.format, format, kind) {
            bypass();
            call(t, l, x, y, w, h, format, kind, data);
            return;
        }
        let Some((nx, ny, nw, nh)) = p.region(l, x, y, w, h) else {
            if l >= p.levels
                && l < if p.immutable {
                    p.original_levels
                } else {
                    policy::mip_count(p.original.0, p.original.1)
                }
                && p.old_size(l).is_some_and(|(ow, oh)| {
                    x >= 0
                        && y >= 0
                        && w > 0
                        && h > 0
                        && x.checked_add(w).is_some_and(|v| v <= ow)
                        && y.checked_add(h).is_some_and(|v| v <= oh)
                })
            {
                DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            call(t, l, x, y, w, h, format, kind, data);
            return;
        };
        let u = c.unpack();
        let Some(v) = c.pixels(data, w, h, nw, nh, format, kind, u) else {
            bypass();
            call(t, l, x, y, w, h, format, kind, data);
            return;
        };
        c.tight(u, || {
            call(t, l, nx, ny, nw, nh, format, kind, v.as_ptr().cast())
        });
    }
}
/// # Safety
/// Native compressed data/PBO offset contract; this wrapper never reads its bytes.
unsafe extern "C" fn compressed_sub(
    t: u32,
    l: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    format: u32,
    size: i32,
    data: *const c_void,
) {
    // SAFETY: native current context; compressed memory forwarded unchanged.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        let call = c.f.compressed_sub;
        let Some(p) = c.plan().filter(|p| t == TEX && p.bias > 0) else {
            call(t, l, x, y, w, h, format, size, data);
            return;
        };
        if l >= 0 && l < p.bias {
            let valid = p.old_size(l).is_some_and(|(ow, oh)| {
                x >= 0
                    && y >= 0
                    && w > 0
                    && h > 0
                    && x.checked_add(w).is_some_and(|v| v <= ow)
                    && y.checked_add(h).is_some_and(|v| v <= oh)
            });
            if valid
                && format == p.format
                && policy::block(format).is_some_and(|(bw, bh, b)| {
                    x % bw == 0
                        && y % bh == 0
                        && ((w + bw - 1) / bw) * ((h + bh - 1) / bh) * b == size
                })
            {
                DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        call(
            t,
            l.checked_sub(p.bias).unwrap_or(-1),
            x,
            y,
            w,
            h,
            format,
            size,
            data,
        );
    }
}
/// # Safety
/// Exact native direct-compressed upload contract; direct uploads have no proven mip chain.
unsafe extern "C" fn compressed(
    t: u32,
    l: i32,
    format: u32,
    w: i32,
    h: i32,
    b: i32,
    size: i32,
    data: *const c_void,
) {
    // SAFETY: no CPU dereference; successful base redefinition retires only this namespace's metadata.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if c.checked(|| (c.f.compressed)(t, l, format, w, h, b, size, data)) && t == TEX && l == 0 {
            c.clear();
        }
    }
}
/// # Safety
/// count names are readable under the native glDeleteTextures contract.
unsafe extern "C" fn delete(n: i32, names: *const u32) {
    // SAFETY: caller's name slice, bounded bookkeeping; driver deletion precedes metadata retirement.
    unsafe {
        let Some(c) = Call::enter(TEX) else {
            return;
        };
        if c.checked(|| (c.f.delete)(n, names)) && n > 0 && !names.is_null() {
            // Process in bounded chunks, retaining no pointer or lock across a driver call.
            for i in 0..n as usize {
                let name = *names.add(i);
                state(|s| s.set((c.key.0, name), None));
            }
        }
    }
}
/// # Safety
/// Exact native framebuffer-copy allocation contract; no CPU pointers.
unsafe extern "C" fn copy_image(t: u32, l: i32, f: u32, x: i32, y: i32, w: i32, h: i32, b: i32) {
    // SAFETY: synchronous native call with caller-owned context/framebuffer; preserves failed allocation metadata.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if c.checked(|| (c.f.copy)(t, l, f, x, y, w, h, b)) && t == TEX && l == 0 {
            c.clear();
        }
    }
}
fn mapped_parameter(c: &Call, name: u32, value: Param, call: impl FnOnce(Param)) {
    let index = policy::PARAMETERS.iter().position(|p| *p == name);
    let existing = state(|s| s.parameters(c.key));
    let mapped = if let (Some(p), Some(_), Some(_)) = (c.plan(), existing, index) {
        policy::parameter(p, name, value)
    } else {
        value
    };
    if c.checked(|| call(mapped)) {
        if let (Some(index), Some(mut values)) = (index, existing) {
            values[index] = if matches!(name, 0x813c | 0x813d) {
                Param::Int(value.integer())
            } else {
                Param::Float(value.float())
            };
            state(|s| s.set_parameters(c.key, values));
        }
    }
}
/// # Safety
/// Exact scalar glTexParameteri contract and caller's current context.
unsafe extern "C" fn parameter_i(t: u32, name: u32, value: i32) {
    // SAFETY: native scalar call; no guest pointer or callback is retained.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        mapped_parameter(&c, name, Param::Int(value), |v| {
            (c.f.parameter_i)(t, name, v.integer())
        });
    }
}
/// # Safety
/// Exact scalar glTexParameterf contract and caller's current context.
unsafe extern "C" fn parameter_f(t: u32, name: u32, value: f32) {
    // SAFETY: native scalar call; only the known mip/LOD parameters are transformed.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        mapped_parameter(&c, name, Param::Float(value), |v| {
            (c.f.parameter_f)(t, name, v.float())
        });
    }
}
/// # Safety
/// Native parameter array covers the pname's required components; known mip pnames use one.
unsafe extern "C" fn parameter_iv(t: u32, name: u32, value: *const i32) {
    // SAFETY: read exactly one element only for one-component mip/LOD pnames; other arrays forward unchanged.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if policy::PARAMETERS.contains(&name) && !value.is_null() {
            mapped_parameter(&c, name, Param::Int(*value), |v| {
                (c.f.parameter_i)(t, name, v.integer())
            });
        } else {
            (c.f.parameter_iv)(t, name, value);
        }
    }
}
/// # Safety
/// Native float parameter array covers the pname's required components.
unsafe extern "C" fn parameter_fv(t: u32, name: u32, value: *const f32) {
    // SAFETY: one-component mip pnames read one element; unrelated multi-component arrays are untouched.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if policy::PARAMETERS.contains(&name) && !value.is_null() {
            mapped_parameter(&c, name, Param::Float(*value), |v| {
                (c.f.parameter_f)(t, name, v.float())
            });
        } else {
            (c.f.parameter_fv)(t, name, value);
        }
    }
}
fn guest_parameter(c: &Call, name: u32) -> Option<Param> {
    let index = policy::PARAMETERS.iter().position(|p| *p == name)?;
    state(|s| s.parameters(c.key)).map(|v| v[index])
}
/// # Safety
/// Native output covers pname's required components. Mip queries write exactly one integer.
unsafe extern "C" fn query_i(t: u32, name: u32, value: *mut i32) {
    // SAFETY: native scalar query contract; never extend a one-component write into caller's canary.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if let Some(v) = guest_parameter(&c, name).filter(|_| !value.is_null()) {
            *value = v.integer();
        } else {
            (c.f.query_i)(t, name, value);
        }
    }
}
/// # Safety
/// Native output covers pname's required components. Mip queries write exactly one float.
unsafe extern "C" fn query_f(t: u32, name: u32, value: *mut f32) {
    // SAFETY: one valid writable scalar for supported query; all other output handling stays native.
    unsafe {
        let Some(c) = Call::enter(t) else {
            return;
        };
        if let Some(v) = guest_parameter(&c, name).filter(|_| !value.is_null()) {
            *value = v.float();
        } else {
            (c.f.query_f)(t, name, value);
        }
    }
}
