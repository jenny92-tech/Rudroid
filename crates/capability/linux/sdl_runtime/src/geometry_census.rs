//! INPUT: original GLES resolver decisions and native primary-context state
//! OUTPUT: bounded object/VAO evidence at UnityStopping
//! POS: observes bindings only; neither owns guest names nor proves GL coverage

#![deny(unsafe_op_in_unsafe_fn)]
use super::{geometry_trace::Op, Route, Graphics};
use std::collections::BTreeMap;
use std::ffi::{c_char, CStr};
use std::io::Write;
use std::sync::{atomic::{AtomicU64, Ordering::Relaxed}, Mutex};

const OBJECT_LIMIT: [usize; 2] = [4096, 16384];
const RAW_LIMIT: usize = 512;
const DRAW_SAMPLE: u64 = 31;
const VAO_BINDING: u32 = 0x85b5;
const ELEMENT: u32 = 0x8893;
const ELEMENT_BINDING: u32 = 0x8895;
const ATTRIB_ENABLED: u32 = 0x8622;
const ATTRIB_BUFFER: u32 = 0x889f;

type GetInteger = unsafe extern "C" fn(u32, *mut i32);
type GetAttrib = unsafe extern "C" fn(u32, u32, *mut i32);
type GetString = unsafe extern "C" fn(u32) -> *const c_char;

#[derive(Clone, Copy)]
pub(super) enum Use {
    Map(u32, u32, isize, isize),
    Target(u32),
    Bound(u32),
    Bind(u32, u32),
    Vao,
    Attrib(u32),
    Draw(bool),
    Indirect(bool),
    Copy(u32, u32),
    Lifecycle(u8),
    Delete(i32, usize),
    Alias(u32, u32),
}

impl Use {
    pub(super) fn post(self) -> bool {
        matches!(self, Self::Bound(_) | Self::Bind(..) | Self::Vao | Self::Attrib(_) | Self::Lifecycle(_) | Self::Delete(..))
    }

    fn needs_vao(self) -> bool {
        match self {
            Self::Map(t, ..) | Self::Target(t) | Self::Bound(t) | Self::Bind(t, _) => t == ELEMENT,
            Self::Copy(r, w) => r == ELEMENT || w == ELEMENT,
            Self::Vao | Self::Attrib(_) | Self::Draw(_) | Self::Indirect(_) => true,
            Self::Lifecycle(_) | Self::Delete(..) | Self::Alias(..) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Buffer,
    ZeroOffsetWriteMap,
    CopyRead,
    CopyWrite,
    Vao,
    AttribBinding,
    DrawIndex,
    DrawVertexSample,
    UnsupportedIndirect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    op: Op,
    role: Role,
    vao: u32,
    object: u32,
    target: u32,
    detail: u32,
    warm: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Epoch {
    buffer: u64,
    vao: u64,
}

struct Observation {
    count: u64,
    first: Epoch,
    last: Epoch,
    slots: u32,
}

#[derive(Default)]
struct State {
    epoch: Epoch,
    objects: BTreeMap<Key, Observation>,
    object_rows: [usize; 2],
    object_overflow: [u64; 2],
    covered: BTreeMap<(Op, usize), [u64; 3]>,
    raw: BTreeMap<Vec<u8>, [[u64; 2]; 2]>,
    raw_overflow: u64,
    calls: BTreeMap<Op, Calls>,
}

#[derive(Default)]
struct Calls {
    primary: u64,
    outside: u64,
    unattributed: u64,
    vertex_samples: u64,
}

impl State {
    fn observe(&mut self, mut key: Key) {
        let phase = usize::from(key.warm);
        let slots = if matches!(key.role, Role::AttribBinding | Role::DrawVertexSample) {
            let bit = 1u32 << key.detail;
            key.detail = 0;
            bit
        } else { 0 };
        if let Some(observation) = self.objects.get_mut(&key) {
            observation.count = observation.count.saturating_add(1);
            observation.last = self.epoch;
            observation.slots |= slots;
        } else if self.object_rows[phase] < OBJECT_LIMIT[phase] {
            self.objects.insert(key, Observation { count: 1, first: self.epoch, last: self.epoch, slots });
            self.object_rows[phase] += 1;
        } else {
            self.object_overflow[phase] = self.object_overflow[phase].saturating_add(1);
        }
    }

    fn retire(&mut self, change: u8) {
        // GL owns the guest pointer and validates the call. Do not inspect the
        // array. Any positive delete advances a conservative namespace epoch.
        if change == 1 { self.epoch.buffer = self.epoch.buffer.saturating_add(1); }
        if change == 2 { self.epoch.vao = self.epoch.vao.saturating_add(1); }
    }

    fn raw(&mut self, name: &[u8], route: Route, resolved: bool) {
        let route = route as usize;
        let status = usize::from(resolved);
        if let Some(count) = self.raw.get_mut(name) {
            count[route][status] = count[route][status].saturating_add(1);
        } else if name.len() <= 256 && self.raw.len() < RAW_LIMIT {
            let mut count = [[0; 2]; 2];
            count[route][status] = 1;
            self.raw.insert(name.to_vec(), count);
        } else {
            self.raw_overflow = self.raw_overflow.saturating_add(1);
        }
    }
}

pub(super) struct Census {
    integer: GetInteger,
    attrib: GetAttrib,
    max_attribs: u32,
    es31: bool,
    draws: AtomicU64,
    state: Mutex<State>,
}

impl Census {
    pub(super) fn load(resolve: impl Fn(&CStr) -> usize) -> Option<Self> {
        // This runs with the verified primary context current, before guest entry.
        macro_rules! entry {
            ($name:literal, $ty:ty) => {{
                let address = resolve($name);
                // SAFETY: exact GLES prototype from the pinned native resolver.
                (address != 0).then(|| unsafe { std::mem::transmute::<usize, $ty>(address) })?
            }};
        }
        let get_string = entry!(c"glGetString", GetString);
        // SAFETY: The session pins this driver entry and the caller owns the current context.
        let version = unsafe { get_string(0x1f02) };
        // SAFETY: the current driver's GL_VERSION string is NUL-terminated and
        // borrowed only here; a failed query returns null.
        let version = (!version.is_null()).then(|| unsafe { CStr::from_ptr(version) })?;
        let minor = version.to_bytes().strip_prefix(b"OpenGL ES 3.")?.first().copied()?;
        let integer = entry!(c"glGetIntegerv", GetInteger);
        let attrib = entry!(c"glGetVertexAttribiv", GetAttrib);
        let mut max_attribs = 0;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { integer(0x8869, &mut max_attribs) };
        (1..=32).contains(&max_attribs).then_some(Self {
            integer, attrib, max_attribs: max_attribs as u32, es31: matches!(minor, b'1' | b'2'),
            draws: AtomicU64::new(0), state: Mutex::default(),
        })
    }

    pub(super) fn lookup(&self, op: Op, route: Route, status: usize) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let count = &mut state.covered.entry((op, route as usize)).or_default()[status];
        *count = count.saturating_add(1);
    }

    pub(super) fn raw_lookup(&self, name: &[u8], route: Route, address: usize) {
        if name.get(2).is_some_and(u8::is_ascii_uppercase) && name.starts_with(b"gl")
            && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            self.state.lock().unwrap_or_else(|p| p.into_inner()).raw(name, route, address != 0);
        }
    }

    fn integer(&self, key: u32) -> u32 {
        let mut value = 0;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.integer)(key, &mut value) };
        value as u32
    }

    fn attrib(&self, index: u32, key: u32) -> u32 {
        let mut value = 0;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.attrib)(index, key, &mut value) };
        value as u32
    }

    fn binding(&self, target: u32) -> Option<u32> {
        let name = match target {
            0x8892 => 0x8894, // ARRAY
            ELEMENT => ELEMENT_BINDING,
            0x8f36 | 0x8f37 => target, // COPY_READ / COPY_WRITE
            0x88eb => 0x88ed, // PIXEL_PACK
            0x88ec => 0x88ef, // PIXEL_UNPACK
            0x8a11 => 0x8a28, // UNIFORM
            0x8c8e => 0x8c8f, // TRANSFORM_FEEDBACK
            0x90d2 if self.es31 => 0x90d3, // SHADER_STORAGE
            0x92c0 if self.es31 => 0x92c1, // ATOMIC_COUNTER
            0x8f3f if self.es31 => 0x8f43, // DRAW_INDIRECT
            0x90ee if self.es31 => 0x90ef, // DISPATCH_INDIRECT
            _ => return None,
        };
        Some(self.integer(name))
    }

    pub(super) fn record(&self, g: &Graphics, op: Op, usage: Use) {
        let primary = g.primary_current();
        let warm = g.swaps.load(Relaxed) >= 120;
        let mut unattributed = false;
        let mut vertex_sample = false;
        // All query enums below belong to the verified current core ES version.
        // Driver calls stay outside the evidence lock and never take GL errors.
        let mut observations = [None; 34]; // <=32 vertex slots plus index and indirect
        let mut length = 0;
        if primary && !matches!(usage, Use::Lifecycle(_) | Use::Delete(..) | Use::Alias(..)) {
            let vao = if usage.needs_vao() { self.integer(VAO_BINDING) } else { 0 };
            let mut add = |role, target, detail, object: Option<u32>| {
                if let Some(object) = object {
                    // Generic bindings do not belong to a VAO. Keep the VAO for
                    // captured inputs/draws and ELEMENT, never an unrelated
                    // active VAO alongside a COPY_WRITE/ARRAY/UBO operation.
                    let vao = if target == ELEMENT || matches!(role,
                        Role::Vao | Role::AttribBinding | Role::DrawIndex | Role::DrawVertexSample
                        | Role::UnsupportedIndirect
                    ) { vao } else { 0 };
                    observations[length] = Some(Key { op, role, vao, object, target, detail, warm });
                    length += 1;
                } else {
                    unattributed = true;
                }
            };
            match usage {
                Use::Map(target, flags, offset, size) => {
                    let role = if flags == 0x36 && offset == 0 && size > 0 {
                        Role::ZeroOffsetWriteMap
                    } else { Role::Buffer };
                    add(role, target, flags, self.binding(target));
                }
                Use::Target(t) | Use::Bound(t) | Use::Bind(t, _) => add(Role::Buffer, t, 0, self.binding(t)),
                Use::Vao => add(Role::Vao, 0, 0, Some(0)),
                Use::Attrib(index) => {
                    let buffer = (index < self.max_attribs).then(|| self.attrib(index, ATTRIB_BUFFER));
                    add(Role::AttribBinding, 0, index, buffer);
                }
                Use::Copy(read, write) => {
                    add(Role::CopyRead, read, 0, self.binding(read));
                    add(Role::CopyWrite, write, 0, self.binding(write));
                }
                Use::Draw(indexed) | Use::Indirect(indexed) => {
                    if matches!(usage, Use::Indirect(_)) {
                        // The command can read GPU-written arguments; the census
                        // only captures the driver buffer name, never its bytes.
                        add(Role::UnsupportedIndirect, 0x8f3f, 0, self.binding(0x8f3f));
                    }
                    if indexed { add(Role::DrawIndex, ELEMENT, 0, self.binding(ELEMENT)); }
                    vertex_sample = self.draws.fetch_add(1, Relaxed) % DRAW_SAMPLE == 0;
                    if vertex_sample {
                        for slot in 0..self.max_attribs {
                            if self.attrib(slot, ATTRIB_ENABLED) != 0 {
                                add(Role::DrawVertexSample, 0, slot, Some(self.attrib(slot, ATTRIB_BUFFER)));
                            }
                        }
                    }
                }
                Use::Lifecycle(_) | Use::Delete(..) | Use::Alias(..) => {}
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Use::Lifecycle(change) = usage { state.retire(change); }
        if let Use::Delete(n, _) = usage { state.retire(u8::from(n > 0)); }
        let calls = state.calls.entry(op).or_default();
        if primary { calls.primary = calls.primary.saturating_add(1); }
        else { calls.outside = calls.outside.saturating_add(1); }
        if unattributed { calls.unattributed = calls.unattributed.saturating_add(1); }
        if vertex_sample { calls.vertex_samples = calls.vertex_samples.saturating_add(1); }
        for key in observations.into_iter().take(length).flatten() { state.observe(key); }
    }

    pub(super) fn report(&self, swaps: u64) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut output = std::io::stderr().lock();
        // Other native DSOs can log to the same fd. Hand over one complete
        // short buffer per row to avoid torn metadata on normal shutdown.
        macro_rules! line {
            ($($arg:tt)*) => {
                if output.write_all(format!("[GLES-CENSUS] {}\n", format_args!($($arg)*)).as_bytes()).is_err() { return; }
            }
        }
        line!(
            "swaps={swaps} rows={} cold_rows={} warm_rows={} cold_overflow={} warm_overflow={} raw_names={} raw_overflow={} buffer_epoch={} vao_epoch={} vertex_sample_every={DRAW_SAMPLE} coverage=observations-only",
            state.objects.len(), state.object_rows[0], state.object_rows[1], state.object_overflow[0], state.object_overflow[1],
            state.raw.len(), state.raw_overflow, state.epoch.buffer, state.epoch.vao
        );
        for (&(op, route), &[missing, wrapped, distinct]) in &state.covered {
            line!("lookup op={} route={route} missing={missing} wrapped={wrapped} distinct={distinct}", op.name());
        }
        for (name, &[[elf_missing, elf], [egl_missing, egl]]) in &state.raw {
            line!("untracked_lookup op={} elf={elf} egl={egl} elf_missing={elf_missing} egl_missing={egl_missing}", String::from_utf8_lossy(name));
        }
        for (op, calls) in &state.calls {
            line!(
                "calls op={} primary={} outside={} unattributed={} vertex_samples={}",
                op.name(), calls.primary, calls.outside, calls.unattributed, calls.vertex_samples
            );
        }
        for (key, row) in &state.objects {
            line!(
                "use op={} role={:?} vao={} buffer={} target={:#x} detail={} warm={} count={} attrib_slots={:#x} buffer_epochs={}-{} vao_epochs={}-{}",
                key.op.name(), key.role, key.vao, key.object, key.target, key.detail, key.warm, row.count,
                row.slots, row.first.buffer, row.last.buffer, row.first.vao, row.last.vao,
            );
        }
        line!("complete rows={}", state.objects.len());
    }
}

#[cfg(test)]
#[path = "../tests/unit/geometry_census.rs"]
mod tests;
