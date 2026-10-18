//! INPUT:  active pinned GLES driver, immutable shader policy and caller-owned GL arguments
//! OUTPUT:  source rewrites and context-specific disabled-program draw suppression
//! POS:    native shader boundary; no guest strings or executable pointers outlive their call
use super::{active, Graphics};
use std::{
    cell::Cell,
    ffi::{c_char, c_void, CStr},
    sync::Mutex,
};
const MAX_SOURCE: usize = 1024 * 1024;
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static CHANGING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub(super) struct Change;
pub(super) fn begin_change() -> Change {
    CHANGING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    Change
}
impl Drop for Change {
    fn drop(&mut self) {
        EPOCH.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CHANGING.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
pub(super) fn skipped_draws() -> u64 {
    SKIPPED.load(std::sync::atomic::Ordering::Relaxed)
}
static STATE: Mutex<Option<super::super::shader_state::State>> = Mutex::new(None);
thread_local! {static BINDING:Cell<(bool,u64)>=const{Cell::new((false,0))};}
fn with_state<T>(f: impl FnOnce(&mut super::super::shader_state::State) -> T) -> T {
    let mut state = STATE.lock().unwrap();
    f(state.get_or_insert_with(Default::default))
}
macro_rules! native {
    ($g:expr,$name:expr,$ty:ty) => {{
        static FUNCTION: std::sync::OnceLock<$ty> = std::sync::OnceLock::new();
        if let Some(function) = FUNCTION.get() {
            *function
        } else {
            let address = $g.native_address($name);
            if address == 0 {
                return;
            }
            // SAFETY: exact private GLES prototype; driver code is process-pinned and
            // each wrapper requires the caller's current context and native arguments.
            let function = unsafe { std::mem::transmute::<usize, $ty>(address) };
            let _ = FUNCTION.set(function);
            function
        }
    }};
}
pub(super) fn symbol(name: &[u8]) -> usize {
    let policy = super::super::shader_policy::policy();
    if !policy.enabled() {
        return 0;
    }
    match name {
        b"glShaderSource" => shader_source as usize,
        b"glCompileShader" if policy.skip => compile_shader as usize,
        b"glDeleteShader" if policy.skip => delete_shader as usize,
        b"glShaderBinary" if policy.skip => shader_binary as usize,
        b"glLinkProgram" if policy.skip => link_program as usize,
        b"glUseProgram" if policy.skip => use_program as usize,
        b"glDeleteProgram" if policy.skip => delete_program as usize,
        b"glDrawArrays" if policy.skip => draw_arrays as usize,
        b"glDrawElements" if policy.skip => draw_elements as usize,
        b"glDrawArraysInstanced" if policy.skip => draw_arrays_instanced as usize,
        b"glDrawElementsInstanced" if policy.skip => draw_elements_instanced as usize,
        _ => 0,
    }
}
pub(super) fn add_context(context: usize, share: usize) {
    with_state(|s| s.add_context(context, share));
}
pub(super) fn context_group(context: usize) -> u64 {with_state(|s| s.context_group(context))}
pub(super) fn forget_context(context: usize) {
    if let Some(group)=with_state(|s| s.forget_context(context)) {super::texture_gl::retire(group);}
}
fn current_skip() -> bool {
    let skip = BINDING
        .try_with(|b| {
            let (skip, epoch) = b.get();
            skip && CHANGING.load(std::sync::atomic::Ordering::Acquire) == 0
                && epoch == EPOCH.load(std::sync::atomic::Ordering::Acquire)
        })
        .unwrap_or(false);
    if skip {
        let count = SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // Startup evidence only. Rendering must not periodically format/write
        // stderr merely because an optional optimization keeps succeeding.
        if count <= 3 {
            eprintln!("[GLES] disabled shader draws skipped={count}");
        }
    }
    skip
}
/// # Safety
/// Call after a successful native context bind, before forwarding guest drawing.
pub(super) unsafe fn refresh_binding(g: &Graphics, install: Option<u32>) {
    if !super::super::shader_policy::policy().skip {
        return;
    }
    // SAFETY: exact current-context and scalar query ABI on the active pinned driver.
    unsafe {
        let context = (g.driver.current_context)() as usize;
        if context == 0 {
            BINDING.with(|b| b.set((false, EPOCH.load(std::sync::atomic::Ordering::Acquire))));
            return;
        }
        let addr = g.native_address(c"glGetIntegerv");
        if addr == 0 {
            return;
        }
        let q: extern "C" fn(u32, *mut i32) = std::mem::transmute(addr);
        let mut program = 0;
        q(0x8b8d, &mut program);
        let epoch = EPOCH.load(std::sync::atomic::Ordering::Acquire);
        let skip =
            with_state(|s| s.binding(context, program as u32, install == Some(program as u32)));
        BINDING.with(|b| b.set((skip, epoch)));
    }
}
/// # Safety
/// Matches glShaderSource: count valid string pointers, optional count lengths; all
/// readable spans/NUL-terminated strings and the current context are caller-owned.
unsafe extern "C" fn shader_source(
    shader: u32,
    count: i32,
    strings: *const *const c_char,
    lengths: *const i32,
) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(
        g,
        c"glShaderSource",
        extern "C" fn(u32, i32, *const *const c_char, *const i32)
    );
    // SAFETY: caller's GL pointer contract. Bounds cap allocations; native glShaderSource
    // copies the owned replacement synchronously before its backing Vec is dropped.
    unsafe {
        let context = (g.driver.current_context)() as usize;
        with_state(|s| s.source(context, shader, false));
        if !(1..=1024).contains(&count) || strings.is_null() {
            call(shader, count, strings, lengths);
            return;
        }
        let mut source = Vec::new();
        for i in 0..count as usize {
            let text = *strings.add(i);
            if text.is_null() {
                continue;
            }
            let size = if lengths.is_null() {
                -1
            } else {
                *lengths.add(i)
            };
            let bytes = if size < 0 {
                CStr::from_ptr(text).to_bytes()
            } else {
                std::slice::from_raw_parts(text.cast::<u8>(), size as usize)
            };
            if source
                .len()
                .checked_add(bytes.len())
                .is_none_or(|len| len > MAX_SOURCE)
            {
                call(shader, count, strings, lengths);
                return;
            }
            source.extend_from_slice(bytes);
        }
        let Ok(source) = std::str::from_utf8(&source) else {
            call(shader, count, strings, lengths);
            return;
        };
        let address = g.native_address(c"glGetShaderiv");
        if address == 0 {
            call(shader, count, strings, lengths);
            return;
        }
        let q: extern "C" fn(u32, u32, *mut i32) = std::mem::transmute(address);
        let mut kind = 0;
        q(shader, 0x8b4f, &mut kind);
        if let Some(rewritten) = super::super::shader_policy::policy().rewrite(kind as u32, source)
        {
            let len = rewritten.source.len() as i32;
            let text = rewritten.source.as_ptr().cast::<c_char>();
            call(shader, 1, &text, &len);
            let context = (g.driver.current_context)() as usize;
            with_state(|s| s.source(context, shader, rewritten.skip));
        } else {
            call(shader, count, strings, lengths);
            let context = (g.driver.current_context)() as usize;
            with_state(|s| s.source(context, shader, false));
        }
    }
}
/// # Safety
/// Exact native program APIs on the caller's current context.
unsafe extern "C" fn link_program(program: u32) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glLinkProgram", extern "C" fn(u32));
    let changing = begin_change();
    call(program);
    let q = native!(g, c"glGetProgramiv", extern "C" fn(u32, u32, *mut i32));
    let mut linked = 0;
    q(program, 0x8b82, &mut linked);
    if linked != 0 {
        let attached = native!(
            g,
            c"glGetAttachedShaders",
            extern "C" fn(u32, i32, *mut i32, *mut u32)
        );
        let mut shaders = [0u32; 32];
        let mut written = 0;
        attached(program, 32, &mut written, shaders.as_mut_ptr());
        // SAFETY: successful link/current context; native output is bounded by the array capacity.
        unsafe {
            let context = (g.driver.current_context)() as usize;
            with_state(|s| s.linked(context, program, &shaders[..written.clamp(0, 32) as usize]));
            drop(changing);
            refresh_binding(&g, Some(program));
        }
    }
}
/// # Safety
/// Exact native use-program API on the caller's current context; the actual bound
/// program is queried so a rejected invalid program never changes cached draw behavior.
unsafe extern "C" fn use_program(program: u32) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glUseProgram", extern "C" fn(u32));
    call(program); // SAFETY: native binding completed, current context remains owned by this thread.
    unsafe {
        refresh_binding(&g, Some(program));
    }
}
/// # Safety
/// Native deletion may be deferred while bound. Keep the current executable's flag;
/// only discard name-cache entries, and refresh on the next bind/context switch.
unsafe extern "C" fn delete_program(program: u32) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glDeleteProgram", extern "C" fn(u32));
    call(program);
    // SAFETY: current-context query is valid; native deletion completed before metadata retirement.
    unsafe {
        with_state(|s| s.delete_program((g.driver.current_context)() as usize, program));
    }
}
/// # Safety
/// All draw arguments/client pointers obey the corresponding native GLES contract.
unsafe extern "C" fn draw_arrays(mode: u32, first: i32, count: i32) {
    if current_skip() {
        return;
    }
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glDrawArrays", extern "C" fn(u32, i32, i32));
    call(mode, first, count);
}
/// # Safety
/// Native element buffer/client indices remain readable for the native draw.
unsafe extern "C" fn draw_elements(mode: u32, count: i32, kind: u32, indices: *const c_void) {
    if current_skip() {
        return;
    }
    let Some(g) = active() else {
        return;
    };
    let call = native!(
        g,
        c"glDrawElements",
        extern "C" fn(u32, i32, u32, *const c_void)
    );
    call(mode, count, kind, indices);
}
/// # Safety
/// Exact native instanced array-draw contract and current context.
unsafe extern "C" fn draw_arrays_instanced(mode: u32, first: i32, count: i32, instances: i32) {
    if current_skip() {
        return;
    }
    let Some(g) = active() else {
        return;
    };
    let call = native!(
        g,
        c"glDrawArraysInstanced",
        extern "C" fn(u32, i32, i32, i32)
    );
    call(mode, first, count, instances);
}
/// # Safety
/// Exact native instanced element-draw contract and current context.
unsafe extern "C" fn draw_elements_instanced(
    mode: u32,
    count: i32,
    kind: u32,
    indices: *const c_void,
    instances: i32,
) {
    if current_skip() {
        return;
    }
    let Some(g) = active() else {
        return;
    };
    let call = native!(
        g,
        c"glDrawElementsInstanced",
        extern "C" fn(u32, i32, u32, *const c_void, i32)
    );
    call(mode, count, kind, indices, instances);
}

/// # Safety
/// Exact native compile API and current context; only success publishes executable metadata.
unsafe extern "C" fn compile_shader(shader: u32) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glCompileShader", extern "C" fn(u32));
    call(shader);
    let q = native!(g, c"glGetShaderiv", extern "C" fn(u32, u32, *mut i32));
    let mut compiled = 0;
    q(shader, 0x8b81, &mut compiled);
    if compiled != 0 {
        // SAFETY: current native context remains live across synchronous compile.
        unsafe {
            with_state(|s| s.compiled((g.driver.current_context)() as usize, shader));
        }
    }
}
/// # Safety
/// Native delete may defer reclamation while attached; retain its compiled marker until gone.
unsafe extern "C" fn delete_shader(shader: u32) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(g, c"glDeleteShader", extern "C" fn(u32));
    call(shader);
    let exists = native!(g, c"glIsShader", extern "C" fn(u32) -> u8);
    if exists(shader) == 0 {
        // SAFETY: current context; driver confirms the shader name has been reclaimed.
        unsafe {
            with_state(|s| s.forget_shader((g.driver.current_context)() as usize, shader));
        }
    }
}
/// # Safety
/// Exact native binary arguments: count readable names and length readable binary bytes.
unsafe extern "C" fn shader_binary(
    count: i32,
    shaders: *const u32,
    format: u32,
    binary: *const c_void,
    length: i32,
) {
    let Some(g) = active() else {
        return;
    };
    let call = native!(
        g,
        c"glShaderBinary",
        extern "C" fn(i32, *const u32, u32, *const c_void, i32)
    );
    call(count, shaders, format, binary, length);
    // SAFETY: caller's readable name array; bounded bookkeeping never reads beyond count.
    unsafe {
        if (1..=4096).contains(&count) && !shaders.is_null() {
            let context = (g.driver.current_context)() as usize;
            with_state(|s| {
                for id in std::slice::from_raw_parts(shaders, count as usize) {
                    s.shader_binary(context, *id);
                }
            });
        }
    }
}
/// # Safety
/// Call only after native ProgramBinary; successful binary loads are never source-derived skips.
pub(super) unsafe fn program_binary_loaded(g: &Graphics, program: u32) {
    let q = native!(g, c"glGetProgramiv", extern "C" fn(u32, u32, *mut i32));
    let mut linked = 0;
    q(program, 0x8b82, &mut linked);
    if linked != 0 {
        // SAFETY: current context and successful native binary operation.
        unsafe {
            with_state(|s| s.set_program((g.driver.current_context)() as usize, program, false));
            EPOCH.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            refresh_binding(g, Some(program));
        }
    }
}
