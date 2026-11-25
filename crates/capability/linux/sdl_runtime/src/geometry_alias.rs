//! INPUT: actual guest resolver results, current EGL identity and typed buffer operations
//! OUTPUT: guarded texture-buffer calls and bounded native lifetime evidence
//! POS: guest stores stay native unless an independent owner proves closed residency

#![deny(unsafe_op_in_unsafe_fn)]
use super::{shader_gl, Graphics};
use rudroid_common::mapped_memory::MappedMemory;
use std::{collections::BTreeMap, ffi::{c_char, CStr}, sync::Mutex};

const NAME_LIMIT: usize = 4096;
const DELETE_LIMIT: usize = 64;
const ALIAS_LIMIT: usize = 256;
const TEXTURE_BUFFER: u32 = 0x8c2a;
const TEXTURE_BINDING_BUFFER: u32 = 0x8c2c;
const TEXTURE_BUFFER_DATA_STORE_BINDING: u32 = 0x8c2d;

type IsBuffer = unsafe extern "C" fn(u32) -> u8;
type GetInteger = unsafe extern "C" fn(u32, *mut i32);
type GetTexture = unsafe extern "C" fn(u32, i32, u32, *mut i32);
type GetString = unsafe extern "C" fn(u32) -> *const c_char;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Name { group: u64, buffer: u32 }

#[derive(Clone, Copy, Debug)]
struct Object { generation: u64, aliased: bool, resident: bool }

#[derive(Default)]
struct State {
    // Production has no closed arena owner. Raw publication and shared-context
    // escape close even the fixture's admission gate for the rest of the scope.
    closed: bool,
    generation: u64,
    objects: BTreeMap<Name, Object>,
    retired: u64,
    attempted: u64,
    confirmed: u64,
    changes: u64,
    materialized: u64,
    uncovered: u64,
    open_boundaries: u64,
    aliases: BTreeMap<(Name, u64), u64>,
    alias_overflow: u64,
}

impl State {
    fn find_or_create(&mut self, name: Name) -> Option<&mut Object> {
        if !self.objects.contains_key(&name) {
            if self.objects.len() >= NAME_LIMIT {
                self.uncovered += 1;
                self.closed = false;
                return None;
            }
            self.generation += 1;
            self.objects.insert(name, Object { generation: self.generation, aliased: false, resident: false });
        }
        self.objects.get_mut(&name)
    }
}

pub(super) struct Guard {
    is_buffer: IsBuffer,
    get: GetInteger,
    get_texture: GetTexture,
    get_string: GetString,
    primary_es3: bool,
    memory: Option<MappedMemory>,
    state: Mutex<State>,
    #[cfg(test)]
    materializer: Mutex<Option<fn(u32) -> bool>>,
}

#[derive(Default)]
pub(super) enum Pending {
    #[default]
    Native,
    Alias { group: u64, target: u32, buffer: u32, generation: Option<u64>, previous: Option<u32> },
    Delete { group: u64, names: Vec<(u32, u64)> },
}

impl Guard {
    pub(super) fn load(resolve: impl Fn(&CStr) -> usize) -> Option<Self> {
        macro_rules! entry {
            ($name:literal, $ty:ty) => {{
                let address = resolve($name);
                // SAFETY: these exact GLES prototypes come from the pinned native resolver.
                (address != 0).then(|| unsafe { std::mem::transmute::<usize, $ty>(address) })?
            }};
        }
        let get_string = entry!(c"glGetString", GetString);
        Some(Self {
            is_buffer: entry!(c"glIsBuffer", IsBuffer), get: entry!(c"glGetIntegerv", GetInteger),
            get_texture: entry!(c"glGetTexLevelParameteriv", GetTexture),
            get_string, primary_es3: Self::version_starts(get_string, b"OpenGL ES 3."), memory: MappedMemory::open().ok(),
            state: Mutex::default(), #[cfg(test)] materializer: Mutex::default(),
        })
    }

    fn context(g: &Graphics) -> u64 {
        // SAFETY: reads thread-local EGL state; a missing current context is valid.
        shader_gl::context_group(unsafe { (g.driver.current_context)() } as usize)
    }

    fn with<R>(&self, update: impl FnOnce(&mut State) -> R) -> R {
        update(&mut self.state.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn materialize(&self, g: &Graphics, name: Name) -> bool {
        let resident = self.with(|s| s.objects.get(&name).is_some_and(|object| object.resident));
        if !resident { return true; }
        if !g.primary_current() || Self::context(g) != name.group { return false; }
        #[cfg(test)]
        if let Some(copy) = *self.materializer.lock().unwrap_or_else(|p| p.into_inner()) {
            if copy(name.buffer) {
                self.with(|s| {
                    s.objects.get_mut(&name).unwrap().resident = false;
                    s.materialized += 1;
                });
                return true;
            }
        }
        false
    }

    pub(super) fn before_raw(&self, g: &Graphics) -> bool {
        let resident = self.with(|s| {
            s.closed = false;
            s.open_boundaries += 1;
            s.objects.iter().filter_map(|(name, object)| object.resident.then_some(*name)).collect::<Vec<_>>()
        });
        resident.into_iter().all(|name| self.materialize(g, name))
    }

    fn version_starts(get_string: GetString, prefix: &[u8]) -> bool {
        // SAFETY: The session pins this driver entry and the caller owns the current context.
        let address = unsafe { get_string(0x1f02) };
        // SAFETY: the current driver owns NUL-terminated GL strings; do not retain them.
        !address.is_null() && unsafe { CStr::from_ptr(address) }.to_bytes().starts_with(prefix)
    }

    fn texture_buffer(&self) -> bool {
        Self::version_starts(self.get_string, b"OpenGL ES 3.2")
    }

    fn attached(&self, target: u32) -> Option<u32> {
        if target != TEXTURE_BUFFER || !self.texture_buffer() { return None; }
        let mut texture = 0;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.get)(TEXTURE_BINDING_BUFFER, &mut texture) };
        if texture == 0 { return None; }
        let mut buffer = -1;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.get_texture)(target, 0, TEXTURE_BUFFER_DATA_STORE_BINDING, &mut buffer) };
        (buffer != -1).then_some(buffer as u32)
    }

    pub(super) fn before_alias(&self, g: &Graphics, target: u32, buffer: u32) -> Option<Pending> {
        let group = Self::context(g);
        if group == 0 || target != TEXTURE_BUFFER { return Some(Pending::Native); }
        let name = Name { group, buffer };
        // SAFETY: The session pins this driver entry and the caller owns the current context.
        let generation = if buffer != 0 && unsafe { (self.is_buffer)(buffer) } != 0 {
            if !self.materialize(g, name) { return None; }
            self.with(|s| s.find_or_create(name).map(|object| {
                object.aliased = true;
                object.generation
            }))
        } else { None };
        self.with(|s| s.attempted += 1);
        Some(Pending::Alias { group, target, buffer, generation, previous: self.attached(target) })
    }

    pub(super) fn bound(&self, g: &Graphics, target: u32, buffer: u32) {
        let group = Self::context(g);
        let key = match target {
            0x8892 => 0x8894, 0x8893 => 0x8895,
            0x8f36 | 0x8f37 if self.primary_es3 && g.primary_current() => target,
            _ => return,
        };
        if group == 0 || buffer == 0 { return; }
        let mut actual = 0;
        // SAFETY: The query writes to the local output of the declared GLES type.
        unsafe { (self.get)(key, &mut actual) };
        // SAFETY: The session pins this driver entry and the caller owns the current context.
        if actual as u32 == buffer && unsafe { (self.is_buffer)(buffer) } != 0 {
            self.with(|s| { s.find_or_create(Name { group, buffer }); });
        }
    }

    pub(super) fn before_bind(&self, g: &Graphics, buffer: u32) -> bool {
        buffer == 0 || self.materialize(g, Name { group: Self::context(g), buffer })
    }

    pub(super) fn before_delete(&self, g: &Graphics, count: i32, address: usize) -> Option<Pending> {
        let group = Self::context(g);
        if group == 0 || count <= 0 { return Some(Pending::Native); }
        let mut bytes = [0; DELETE_LIMIT * 4];
        let len = count as usize;
        if address == 0 || len > DELETE_LIMIT || !self.memory.as_ref().is_some_and(|reader| {
            reader.read_exact(address, &mut bytes[..len * 4]).is_ok()
        }) {
            self.with(|s| s.uncovered += 1);
            return self.before_raw(g).then_some(Pending::Native);
        }
        let mut names = Vec::new();
        for entry in bytes[..len * 4].chunks_exact(4) {
            let buffer = u32::from_ne_bytes(entry.try_into().unwrap());
            let name = Name { group, buffer };
            if let Some(generation) = self.with(|s| s.objects.get(&name).map(|obj| obj.generation)) {
                if !self.materialize(g, name) { return None; }
                names.push((buffer, generation));
            }
        }
        Some(Pending::Delete { group, names })
    }

    pub(super) fn done(&self, pending: Pending) {
        match pending {
            Pending::Alias { group, target, buffer, generation, previous } => {
                if self.attached(target) == Some(buffer) {
                    self.with(|s| {
                        s.confirmed += 1;
                        s.changes += u64::from(previous.is_some_and(|old| old != buffer));
                        if let Some(generation) = generation {
                            let key = (Name { group, buffer }, generation);
                            if let Some(count) = s.aliases.get_mut(&key) { *count += 1; }
                            else if s.aliases.len() < ALIAS_LIMIT { s.aliases.insert(key, 1); }
                            else { s.alias_overflow += 1; }
                        }
                    });
                }
            }
            Pending::Delete { group, names } => {
                for (buffer, generation) in names {
                    // SAFETY: The session pins this driver entry and the caller owns the current context.
                    if unsafe { (self.is_buffer)(buffer) } != 0 { continue; }
                    self.with(|s| {
                        let name = Name { group, buffer };
                        if s.objects.get(&name).is_some_and(|object| object.generation == generation) {
                            s.objects.remove(&name);
                            s.retired += 1;
                        }
                    });
                }
            }
            _ => {}
        }
    }

    pub(super) fn report(&self) {
        self.with(|s| {
            rudroid_common::diagnostic!(
                "[GLES-ALIAS] generations={} retired={} live={} attempts={} confirmed_state={} observed_changes={} materialized={} uncovered={} open_boundaries={} alias_overflow={} residency={}",
                s.generation, s.retired, s.objects.len(), s.attempted, s.confirmed, s.changes, s.materialized, s.uncovered,
                s.open_boundaries, s.alias_overflow, if s.closed { "owned" } else { "native" },
            );
            for (&(name, generation), count) in &s.aliases {
                rudroid_common::diagnostic!("[GLES-ALIAS] attachment group={} buffer={} generation={generation} confirmed_state={count}", name.group, name.buffer);
            }
        });
    }
}

#[cfg(test)]
#[path = "../tests/unit/geometry_alias.rs"]
pub(crate) mod tests;
