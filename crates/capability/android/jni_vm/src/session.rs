//! INPUT:  shared HostApi catalog callbacks, JNI handles and registered guest native methods
//! OUTPUT:  shared class/method/reference/thread authority, handle-safe JNI shims and checked native dispatch
//! POS:    caches one method record for fixed replies/native bindings and calls guest code outside locks
//! The shared JNI session state and its table shims (ticket 52).
//!
//! One process-wide session behind the global [`SESSION`], created by the
//! `android_jni_vm` early-init phase with a copy of the loader's Host API
//! table: method resolution consults the registered catalog provider
//! THROUGH that table (registry -> `reply_catalog` .so), so a guest call
//! crossing the JNI function table reaches the same catalog ecosystem as
//! every capability — no static links (tickets 43/45).
//!
//! Semantics (the ticket-52 minimum, all explicit):
//! - `FindClass` auto-registers any well-formed name (jnivm parity); empty
//!   or null names fail with a pending `NoClassDefFoundError`.
//! - `GetMethodID`/`GetStaticMethodID` resolve `(class, method, sig)`
//!   against registered natives or the catalog once (bool channel first, then int), then reuse
//!   the session's exact-key index. A miss — unknown
//!   method OR wrong signature; the v1 registry API cannot distinguish
//!   them — returns null with a pending `NoSuchMethodError`-style
//!   exception, exactly the spec outcome for both.
//! - `CallStaticIntMethodA`/`CallStaticBooleanMethodA` answer the captured
//!   fixed reply or exact supported native shape for the matching kind. Calling through the WRONG kind is
//!   a type mismatch: zero result plus a pending exception, never a fake
//!   default.
//! - While an exception is pending, non-exception entries fail with a zero
//!   result and the exception stays pending until `ExceptionClear`
//!   (calls blocked, not silently served).
//! - References: every guest-visible `jobject` is a tagged handle into the
//!   session's local/global ref tables (never a raw pointer), so
//!   local/global lifetime semantics are session-wide and shared by every
//!   library holding the same `JavaVM*`. Deleted handles fail explicitly.
//! - Local frames (Push/PopLocalFrame, libjnivm parity): every local
//!   reference belongs to the calling thread's innermost frame; popping a
//!   frame releases pins in place and re-registers the `result` in the
//!   outer frame. Popping with no pushed frame clears the base frame.
//! - Pending state and observed/rethrown handles share one immutable
//!   exception payload. Reference tables reuse small buffers and shrink
//!   large bursts once no more than 256 entries remain.
//! - Serials never wrap: handle payloads are 56-bit and allocation fails
//!   explicitly (null + pending) once exhausted, so a live id is never
//!   reissued.
//! - Monitors are reentrant per object identity and owner-checked;
//!   RegisterNatives updates the same method index as GetMethodID; boolean
//!   static calls preserve exact boolean native prototypes, including (I)Z.
//!   Unregister revokes cached addresses; other native call shapes fail explicitly.
//! - Initialization attaches its calling thread; both Attach entries
//!   create or reuse thread state. GetEnv only queries that attachment,
//!   validates the supported JNI versions, and clears failed output.
//! - Thread state has one owner record shared with its TLS cache; local
//!   operations lock only that record. The session registry is held only
//!   for attach/detach or a TLS cache miss, never across a local operation.
//! - Unimplemented table entries (see `abi`) log their name, set the
//!   pending exception and return a zero default.
//!
//! The session never frees while guests may hold handles: it lives for the
//! process (the registry's shared-VM slot is first-write-wins), mirroring
//! the kit-wide process-pinned ownership model (ticket 46).

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use rudroid_common::host_api::HostApiV0;
use rudroid_common::jni_abi::JValue;

use super::abi;
/// Descriptor-selected native calls share this one audited cast boundary.
/// The enclosing dispatcher and its JNI caller establish the exact ABI,
/// process-pinned code and readable jvalue argument storage.
macro_rules! native_call {
    ($address:expr, fn($env_ty:ty, $receiver_ty:ty $(, $ty:ty)*) $(-> $ret:ty)?;
        $env:expr, $receiver:expr $(, $arg:expr)* $(,)?) => {{
        // SAFETY: validated registered shape and the enclosing native caller contract.
        let function: unsafe extern "C" fn($env_ty, $receiver_ty $(, $ty)*) $(-> $ret)? = std::mem::transmute($address);
        crate::session::session().expect("checked native session")
            .with_native_frame(|session| Ok(function($env,
                session.native_argument::<$receiver_ty>($receiver)?
                $(, session.native_argument::<$ty>($arg)?)*)))
    }};
}
#[path = "scalar_calls.rs"]
mod scalar_calls;
#[path = "nonvirtual_calls.rs"]
mod nonvirtual_calls;
#[path = "native_frames.rs"]
mod native_frames;
#[path = "objects.rs"]
mod objects;
#[path = "streams.rs"]
mod streams;
#[path = "stream_buffer.rs"]
mod stream_buffer;
#[path = "bytes.rs"]
mod bytes;
#[path = "primitive_arrays.rs"]
mod primitive_arrays;
#[path = "direct_buffers.rs"]
mod direct_buffers;
#[path = "strings.rs"]
mod strings;
#[path = "fields.rs"]
mod fields;
#[cfg(test)]
#[path = "../tests/unit/session_object_tests.rs"]
mod object_tests;
#[cfg(test)]
#[path = "../tests/unit/native_frames.rs"]
mod native_frame_tests;

// ------------------------------------------------------------------ model

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    Static,
    Virtual,
    Nonvirtual(*mut c_void),
}
impl From<bool> for Dispatch {
    fn from(instance: bool) -> Self {
        if instance { Self::Virtual } else { Self::Static }
    }
}

/// One method authority: fixed catalog replies or a guest native binding.
/// Native shapes are exact prototypes, not a guessed generic FFI call.
/// The caller keeps guest code mapped until unregister and all in-flight
/// callbacks return; unregister cannot cancel an already-snapshotted address.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MethodValue {
    Bool(bool),
    Int(i64),
    NativeBoolString(usize),
    NativeBoolNoArgs(usize),
    NativeBoolInt(usize),
    NativeVoid(usize, VoidShape),
    NativeInt(usize,IntShape),
    NativeScalar(usize, scalar_calls::Return, scalar_calls::Argument),
    NativeLongNoArgs(usize),
    NativeLongObjectLong(usize),
    NativeFloatNoArgs(usize),
    NativeFloatInt(usize),
    NativeFloatTwoInts(usize),
    NativeFloatObjectFloat(usize),
    NativeBoolObjectLong(usize),
    NativeBoolObjectBoolean(usize),
    NativeObject(usize, ObjectShape),
    NativeOther(usize),
    NativeUnbound,
}

/// Exact native prototypes used by Unity lifecycle methods. Unsupported
/// descriptors stay unavailable rather than being cast to a guessed ABI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoidShape { NoArgs, Boolean, Int, Float, Long, IntInt, Context, ContextInt, IntSurface, IntReference, ReferenceTwoInts, LongThreeReferences, TwoReferences, ThreeReferences, FieldDeclaration, TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean, ReferenceIntFourBooleansReferenceIntTwoBooleans }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObjectShape { TwoInts, ThreeInts, NoArgs, String, StringBoolObject, TwoObjects, ThreeObjects, LongObject, LongTwoObjects, LongThreeObjects, ObjectLongObject, LongBoolean, StringInt, StringLong, StringFloat, StringBoolean, Int, ClassStringsBoolean }

// Select a pointer ABI only after parsing the complete all-reference descriptor.
// Mixed primitive arguments never enter this bounded family.
fn reference_native(signature:&str,address:usize)->Option<MethodValue>{
    fn reference(text:&str)->Option<&str>{
        let mut value=text;
        let array=value.starts_with('[');
        while value.starts_with('['){value=&value[1..];}
        if let Some(object)=value.strip_prefix('L'){
            let end=object.find(';')?;if end==0||object[..end].contains(['(',')','[']){return None;}
            Some(&object[end+1..])
        }else if array&&matches!(value.as_bytes().first(),Some(b'Z'|b'B'|b'C'|b'S'|b'I'|b'J'|b'F'|b'D')){Some(&value[1..])}else{None}
    }
    let mut args=signature.strip_prefix('(')?;let mut count=0;
    while !args.starts_with(')'){args=reference(args)?;count+=1;if count>3{return None;}}
    let result=&args[1..];
    if result=="V" {return match count{1=>Some(MethodValue::NativeVoid(address,VoidShape::Context)),2=>Some(MethodValue::NativeVoid(address,VoidShape::TwoReferences)),3=>Some(MethodValue::NativeVoid(address,VoidShape::ThreeReferences)),_=>None};}
    if result=="Z"&&count==1{return Some(MethodValue::NativeBoolString(address));}
    if !reference(result)?.is_empty(){return None;}
    match count{0=>Some(MethodValue::NativeObject(address,ObjectShape::NoArgs)),1=>Some(MethodValue::NativeObject(address,ObjectShape::String)),2=>Some(MethodValue::NativeObject(address,ObjectShape::TwoObjects)),3=>Some(MethodValue::NativeObject(address,ObjectShape::ThreeObjects)),_=>None}
}

// Layout classification is independent of a vendor namespace. Every reference
// (including primitive arrays) occupies a JNI pointer; primitive widths stay exact.
fn mixed_void_native(signature:&str,address:usize)->Option<MethodValue>{
    let mut rest=signature.strip_prefix('(')?;let mut layout=Vec::new();
    while !rest.starts_with(')') {
        let mut value=rest;let mut array=false;
        while let Some(next)=value.strip_prefix('['){array=true;value=next;}
        let first=*value.as_bytes().first()?;
        if first==b'L' {
            let end=value.find(';')?;
            if end<=1||value[1..end].contains(['(',')','[']){return None;}
            rest=&value[end+1..];layout.push(b'L');
        }else if matches!(first,b'Z'|b'B'|b'C'|b'S'|b'I'|b'J'|b'F'|b'D') {
            rest=&value[1..];layout.push(if array{b'L'}else{first});
        }else{return None;}
        if layout.len()>255{return None;}
    }
    if rest!=")V"{return None;}
    let shape=match layout.as_slice(){
        b"LI"=>VoidShape::ContextInt,
        b"IL"=>VoidShape::IntReference,
        b"LII"=>VoidShape::ReferenceTwoInts,
        b"JLLL"=>VoidShape::LongThreeReferences,
        b"LLZZLLZLZ"=>VoidShape::TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean,
        b"LIZZZZLIZZ"=>VoidShape::ReferenceIntFourBooleansReferenceIntTwoBooleans,
        _=>return None,
    };Some(MethodValue::NativeVoid(address,shape))
}
#[derive(Debug,Clone,Copy,PartialEq)]
pub enum IntShape {NoArgs,Int,TwoInts,ThreeInts,Reference,ReferenceInt,ReferenceIntInt,ThreeReferences}

/// What a reference handle points at (identity for `IsSameObject`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RefTarget {
    Class(u64),
    ReflectedMethod(u64),
    ReflectedField(u64),
    Str(u64),
    Throwable(u64),
    Object(u64),
    Array(u64),
}

/// One resolved method: identity plus fixed reply or revocable native binding.
#[derive(Debug, Clone)]
struct Method {
    class: u64,
    name: Box<str>,
    sig: Box<str>,
    value: MethodValue,
    /// Next id with the same key fingerprint; exact comparisons preserve
    /// correctness even on a hash collision. Zero is never a live serial.
    next_collision: u64,
}

#[derive(Default)]
struct MethodTable {
    by_id: HashMap<u64, Method>,
    /// Index fingerprints only: names/signatures remain owned once by
    /// `by_id`, rather than copied into a second lookup table.
    by_key: HashMap<u64, u64>,
    /// Inherited resolutions retain method IDs, not another copy of replies or keys.
    aliases: HashMap<u64, Vec<u64>>,
}

impl MethodTable {
    fn find(&self, class: u64, name: &str, sig: &str) -> Option<u64> {
        let key = self.by_key.hasher().hash_one((class, name, sig));
        self.find_from(*self.by_key.get(&key)?, class, name, sig)
    }

    fn find_cached(&self, class: u64, name: &str, sig: &str) -> Option<u64> {
        self.find(class, name, sig).or_else(|| self.aliases.get(&class)?.iter().copied().find(|id| {
            self.by_id.get(id).is_some_and(|m| m.name.as_ref() == name && m.sig.as_ref() == sig)
        }))
    }

    fn cache_alias(&mut self, class: u64, method: u64) {
        if self.by_id[&method].class != class {
            let aliases = self.aliases.entry(class).or_default();
            if !aliases.contains(&method) { aliases.push(method); }
        }
    }

    fn find_from(&self, mut serial: u64, class: u64, name: &str, sig: &str) -> Option<u64> {
        while serial != 0 {
            let method = self.by_id.get(&serial)?;
            if method.class == class && method.name.as_ref() == name && method.sig.as_ref() == sig {
                return Some(serial);
            }
            serial = method.next_collision;
        }
        None
    }

    fn insert(&mut self, serial: u64, mut method: Method) {
        let key = self.by_key.hasher().hash_one((method.class, method.name.as_ref(), method.sig.as_ref()));
        method.next_collision = self.by_key.insert(key, serial).unwrap_or(0);
        self.by_id.insert(serial, method);
    }
}

#[derive(Default)]
struct ClassTable {
    by_id: HashMap<u64, Arc<str>>,
    by_name: HashMap<Arc<str>, u64>,
    parents: HashMap<u64, u64>,
    interfaces: HashMap<u64, Vec<u64>>,
    visits: HashMap<u64,u64>,
    visit_epoch:u64,
}

/// Keep small reference buffers warm; a large burst need not become fully
/// empty before releasing its buckets. Rehash only when at most 256 live
/// entries remain in a table larger than 4096, not on every deletion.
fn trim_reference_table<K: Eq + std::hash::Hash, V>(table: &mut HashMap<K, V>) {
    if table.len() <= 256 && table.capacity() > 4096 {
        if table.is_empty() {
            // Free the large allocation before reserving the small reusable one.
            *table = HashMap::new();
            *table = HashMap::with_capacity(256);
        } else {
            table.shrink_to(256);
        }
    }
}

/// Immutable exception identity and payload shared by its JNI owners.
#[derive(Debug)]
struct Pending {
    class: String,
    message: String,
    /// Identity so every `ExceptionOccurred` handle to the same pending
    /// exception compares equal under `IsSameObject`.
    id: u64,
}

/// `JNINativeMethod` from the oracle `jni.h` (RegisterNatives input).
#[repr(C)]
pub struct JniNativeMethod {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub fn_ptr: *mut c_void,
}

/// Handle tag nibbles (top byte of the handle word). Handles are NEVER raw
/// pointers — a guest dereferencing one cannot corrupt host memory.
const TAG_GLOBAL: u64 = 0x31;
const TAG_LOCAL: u64 = 0x32;
const TAG_METHOD: u64 = 0x33;
const TAG_FIELD: u64 = 0x34;
const TAG_WEAK: u64 = 0x35;

/// Serials live in the low 56 bits of a handle word (the top byte is the
/// tag); the allocator refuses to go past this instead of wrapping.
const SERIAL_LIMIT: u64 = 1 << 56;

fn make_handle(tag: u64, serial: u64) -> *mut c_void {
    ((tag << 56) | serial) as *mut c_void
}

fn handle_parts(handle: *mut c_void) -> (u64, u64) {
    let word = handle as u64;
    (word >> 56, word & (SERIAL_LIMIT - 1))
}

// ---------------------------------------------------------------- session

pub struct Session {
    /// Distinguishes local/test sessions even when their addresses are reused.
    id: u64,
    /// Copy of the loader's Host API table: catalog queries and structured
    /// logs cross the C ABI into the host; the fn pointers stay valid for
    /// the process life (the loader's table is a static).
    host: OnceLock<HostApiV0>,
    /// Every Java stream uses this session's single optional-buffer budget.
    stream_buffers: OnceLock<Arc<stream_buffer::BufferBudget>>,
    serial: AtomicU64,
    classes: Mutex<ClassTable>,
    methods: Mutex<MethodTable>,
    /// References, pending exceptions and UTF borrows pin one shared
    /// entity. Releases inspect this count, never other threads' tables.
    /// String and throwable entity ids share the non-reusing serial space.
    liveness: Mutex<HashMap<u64, usize>>,
    /// Strings include their trailing NUL once, so every UTF borrow can
    /// point to the same stable immutable allocation.
    strings: Mutex<HashMap<u64, strings::StringData>>,
    /// AllocObject identities and their class, reclaimed after the last JNI pin.
    objects: Mutex<HashMap<u64, fields::Object>>,
    arrays: Mutex<HashMap<u64, objects::ObjectArray>>,
    fields: Mutex<fields::Fields>,
    primitive_leases: Mutex<HashMap<usize,primitive_arrays::Lease>>,
    class_loader: OnceLock<usize>,
    builtins: OnceLock<()>,
    trace_natives: std::sync::atomic::AtomicBool,
    trace_calls: std::sync::atomic::AtomicBool,
    /// ptr -> (entity id, outstanding borrow count). Repeated borrows may
    /// return the same pointer; each requires one ReleaseStringUTFChars.
    string_chars: Mutex<HashMap<usize, (u64, usize)>>,
    globals: Mutex<HashMap<u64, RefTarget>>,
    weaks: Mutex<HashMap<u64, RefTarget>>,
    /// One immutable entity shared by pending state, observed handles and
    /// cross-thread rethrows. Its payload is not copied by ExceptionOccurred
    /// or Throw; the index drops it after its final JNI pin is released.
    throwables: Mutex<HashMap<u64, Arc<Pending>>>,
    /// Held monitors: object identity -> (owner attach key, entry count).
    monitors: Mutex<HashMap<RefTarget, (u64, u32)>>,
    monitor_released: Condvar,
    /// Per-thread state, keyed by attach key (ticket 88). Auto-attach on
    /// first JNI use; DetachCurrentThread (or thread exit) removes it.
    threads: Mutex<HashMap<u64, Arc<Mutex<ThreadState>>>>,
}

/// A temporary JNI root. No raw payload escapes; Drop releases even when a
/// checked operation returns early or Rust unwinds before a C boundary.
struct ReferenceRoot<'a> {
    session: &'a Session,
    target: RefTarget,
}
impl Drop for ReferenceRoot<'_> {
    fn drop(&mut self) { self.session.release_target(self.target); }
}

/// Per-thread JNI state (ticket 88): local references and the pending
/// exception belong to the thread that created them — the reference model
/// of the real JNI. Globals stay session-wide (cross-thread by contract).
#[derive(Default)]
struct ThreadState {
    locals: HashMap<u64, Local>,
    /// Number of frames pushed above the base frame (0 = base frame).
    frame: usize,
    /// The automatic native frame cannot be popped by guest PopLocalFrame.
    /// Nested calls restore the previous boundary without a stack allocation.
    native_floor: Option<usize>,
    pending: Option<Arc<Pending>>,
}

/// One local reference and the frame that owns it.
#[derive(Debug, Clone, Copy)]
struct Local {
    target: RefTarget,
    frame: usize,
}

static THREAD_KEY_ALLOC: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);
static SESSION_ID_ALLOC: AtomicU64 = AtomicU64::new(1);

struct CachedThread {
    session: u64,
    key: u64,
    /// Same record as the session index: no copied locals or exceptions.
    state: Arc<Mutex<ThreadState>>,
}

/// The calling thread's attach key (0 = not attached). Its destructor runs
/// the detach cleanup when a thread exits without DetachCurrentThread.
/// The last cell tags the cached attachment, so GetEnv need not borrow or
/// lock the mutable state (it remains queryable during a local operation).
struct ThreadKey(
    std::cell::Cell<u64>,
    std::cell::RefCell<Option<CachedThread>>,
    std::cell::Cell<u64>,
);

impl Drop for ThreadKey {
    fn drop(&mut self) {
        let key = self.0.get();
        if key == 0 {
            return;
        }
        // The global session is a never-dropped static, so it outlives
        // every thread's TLS. Local (test) sessions are not reachable from
        // here; their records die with them. No logging: stderr may not be
        // usable while TLS is being torn down.
        if let Some(session) = SESSION.get() {
            session.detach_key(key);
        }
    }
}

thread_local! {
    static THREAD_KEY: ThreadKey = const {
        ThreadKey(std::cell::Cell::new(0), std::cell::RefCell::new(None), std::cell::Cell::new(0))
    };
}

#[cfg(test)]
thread_local! {
    static SWEEP_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Session {
    /// A fresh, empty session. Catalog access activates once a host table
    /// is installed (`install_host`); without one, method resolution fails
    /// explicitly (no catalog provider -> no such method).
    fn new() -> Session {
        Session {
            id: SESSION_ID_ALLOC.fetch_add(1, Ordering::Relaxed),
            host: OnceLock::new(),
            stream_buffers: OnceLock::new(),
            serial: AtomicU64::new(1),
            classes: Mutex::new(ClassTable::default()),
            methods: Mutex::new(MethodTable::default()),
            liveness: Mutex::new(HashMap::new()),
            strings: Mutex::new(HashMap::new()),
            objects: Mutex::new(HashMap::new()),
            arrays: Mutex::new(HashMap::new()),
            fields: Mutex::new(fields::Fields::default()),
            primitive_leases: Mutex::new(HashMap::new()),
            class_loader: OnceLock::new(),
            builtins: OnceLock::new(),
            trace_natives: std::sync::atomic::AtomicBool::new(false),
            trace_calls: std::sync::atomic::AtomicBool::new(false),
            string_chars: Mutex::new(HashMap::new()),
            globals: Mutex::new(HashMap::new()),
            weaks: Mutex::new(HashMap::new()),
            throwables: Mutex::new(HashMap::new()),
            monitors: Mutex::new(HashMap::new()),
            monitor_released: Condvar::new(),
            threads: Mutex::new(HashMap::new()),
        }
    }

    /// The calling thread's attach key — allocated on first JNI use and
    /// stable for the thread's lifetime (until DetachCurrentThread). `None`
    /// once the thread's TLS is being destroyed.
    fn thread_key() -> Option<u64> {
        THREAD_KEY
            .try_with(|k| {
                if k.0.get() == 0 {
                    k.0.set(THREAD_KEY_ALLOC.fetch_add(1, std::sync::atomic::Ordering::AcqRel));
                }
                k.0.get()
            })
            .ok()
    }

    /// Attach without creating local references or pending exceptions.
    /// All threads share the immutable env table, but own their state.
    fn attach_current_thread(&self) -> bool {
        if self.current_thread_is_attached() { return true; }
        let Some(key) = Self::thread_key() else {
            return false;
        };
        THREAD_KEY.with(|thread| {
            self.cache_thread_state(key, &mut thread.1.borrow_mut());
            thread.2.set(self.id);
        });
        true
    }

    fn cache_thread_state(&self, key: u64, cached: &mut Option<CachedThread>) {
        if cached.as_ref().is_some_and(|entry| entry.session == self.id && entry.key == key) {
            return;
        }
        let state = self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(ThreadState::default())))
            .clone();
        *cached = Some(CachedThread { session: self.id, key, state });
    }

    /// Read the existing TLS key without allocating one. A key belonging
    /// to another Session is not an attachment to this session.
    fn current_thread_is_attached(&self) -> bool {
        THREAD_KEY.try_with(|thread| {
            let key = thread.0.get();
            key != 0 && (thread.2.get() == self.id || self.threads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&key))
        }).unwrap_or(false)
    }

    /// Route one operation through the calling thread's own state
    /// (ticket 88). The closure must not re-enter `with_thread` (its state
    /// lock is held across it). A thread whose TLS is already gone gets a
    /// throwaway state: nothing it creates can outlive the call.
    fn with_thread<R>(&self, f: impl FnOnce(&mut ThreadState) -> R) -> R {
        let Some(key) = Self::thread_key() else {
            let mut state = ThreadState::default();
            let result = f(&mut state);
            for local in state.locals.into_values() { self.release_target(local.target); }
            if let Some(pending) = state.pending { self.release_target(RefTarget::Throwable(pending.id)); }
            return result;
        };
        THREAD_KEY.with(|thread| {
            let mut cached = thread.1.borrow_mut();
            self.cache_thread_state(key, &mut cached);
            thread.2.set(self.id);
            let mut state = cached.as_ref().unwrap().state
                .lock().unwrap_or_else(|p| p.into_inner());
            f(&mut state)
        })
    }

    /// DetachCurrentThread (ticket 88): drop the thread's locals and
    /// pending exception and withdraw its attach key.
    fn detach_current_thread(&self) -> c_int {
        if self.current_thread_is_attached() && self.with_thread(|s| s.native_floor.is_some()) {
            return abi::JNI_ERR;
        }
        let key = THREAD_KEY.try_with(|thread| {
            let key = thread.0.replace(0);
            thread.2.set(0);
            thread.1.borrow_mut().take();
            key
        }).unwrap_or(0);
        if key != 0 {
            self.detach_key(key);
        }
        0
    }

    /// Detach cleanup for attach key `key`, shared by DetachCurrentThread
    /// and the thread-exit destructor: drops the thread record (locals,
    /// frames, pending), releases the monitors it still holds, and
    /// reclaims entities only its locals were pinning. Never logs and never
    /// touches thread-local storage.
    fn detach_key(&self, key: u64) {
        let removed = self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        {
            let mut monitors = self.monitors.lock().unwrap_or_else(|p| p.into_inner());
            let before = monitors.len();
            monitors.retain(|_, (owner, _)| *owner != key);
            if monitors.len() != before {
                self.monitor_released.notify_all();
            }
        }
        if let Some(state) = removed {
            let state = std::mem::take(&mut *state.lock().unwrap_or_else(|p| p.into_inner()));
            for local in state.locals.into_values() { self.release_target(local.target); }
            if let Some(pending) = state.pending { self.release_target(RefTarget::Throwable(pending.id)); }
        }
    }

    /// Test-only interleaving point: when armed on the calling thread, runs
    /// detach path another thread's exit would run (a key no thread owns).
    /// Reclamation is now driven by released pins, so an unrelated detach
    /// cannot scan or reclaim an entity being published.
    #[cfg(test)]
    fn sweep_probe(&self) {
        if SWEEP_PROBE.with(|armed| armed.get()) {
            self.detach_key(u64::MAX);
        }
    }

    #[cfg(not(test))]
    #[inline(always)]
    fn sweep_probe(&self) {}

    /// Attached-thread count (ticket 88 test/diagnostic surface).
    #[cfg_attr(not(test), allow(dead_code))]
    fn attached_thread_count(&self) -> usize {
        self.threads.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The current thread's pending exception (test helper).
    #[cfg(test)]
    fn current_pending(&self) -> Option<Arc<Pending>> {
        self.with_thread(|s| s.pending.clone())
    }

    fn install_host(&self, host: HostApiV0) {
        if self.host.set(host).is_ok() {
            self.trace_natives.store(host.config_bool(c"jni.trace_native_registration", false), Ordering::Relaxed);
            self.trace_calls.store(host.config_bool(c"jni.trace_native_calls", false), Ordering::Relaxed);
        }
    }

    /// The next entity/handle serial; `None` once the 56-bit handle space
    /// is exhausted (never wraps onto a live id).
    fn next_serial(&self) -> Option<u64> {
        self.serial
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |serial| {
                (serial < SERIAL_LIMIT).then_some(serial + 1)
            })
            .ok()
    }

    /// Explicit failure for serial exhaustion: logged, pending
    /// OutOfMemoryError, and the caller returns null.
    fn serial_exhausted(&self, entry: &str) {
        self.log(&format!("{entry}: JNI handle serials exhausted — refusing to reuse a live id"));
        self.set_pending("java/lang/OutOfMemoryError", "JNI handle serials exhausted");
    }

    /// Structured log through the host table when installed, stderr
    /// otherwise (unit tests drive sessions without a host).
    fn log(&self, message: &str) {
        if let Some(host) = self.host.get() {
            host.log_line(c"android_jni_vm", message);
            return;
        }
        eprintln!("[android_jni_vm] {message}");
    }

    fn host_api(&self) -> Option<&HostApiV0> {
        self.host.get()
    }

    // -- catalog resolution (the registry/catalog ecosystem interop)

    fn catalog_query(&self, class: &str, method: &str, sig: &str) -> Option<MethodValue> {
        let host = self.host_api()?;
        let read = |v: &str| CString::new(v.replace('\0', "\\0")).ok();
        let (c, m, s) = (read(class)?, read(method)?, read(sig)?);
        if let Some(b) = host.catalog_bool(&c, &m, &s) {
            return Some(MethodValue::Bool(b));
        }
        host.catalog_int(&c, &m, &s).map(MethodValue::Int)
    }

    // -- pending exception helpers

    /// Raise a NEW exception on the calling thread, replacing any pending
    /// one (the reference's ThrowNew creates a fresh Throwable): it gets its
    /// own identity, so a handle to the replaced exception keeps that
    /// exception's class and message for a later `Throw`.
    fn set_pending(&self, class: &str, message: &str) {
        // Exhausted serials leave id 0: no handle can be created for it
        // anyway (ExceptionOccurred fails the same way).
        let id = self.next_serial().unwrap_or(0);
        let pending = Arc::new(Pending {
            class: class.to_owned(),
            message: message.to_owned(),
            id,
        });
        self.replace_pending(pending);
    }

    /// Install `pending` and release the throwable it replaced, if that one
    /// is no longer referenced.
    fn replace_pending(&self, pending: Arc<Pending>) {
        let id = pending.id;
        self.pin_target(RefTarget::Throwable(id));
        let replaced = self.with_thread(|state| state.pending.replace(pending).map(|p| p.id));
        if let Some(old) = replaced {
            self.release_target(RefTarget::Throwable(old));
        }
    }

    /// Returns true (and logs) when `entry` must not execute because an
    /// exception is pending — the explicit "blocked, not silently served"
    /// semantic.
    fn blocked_by_pending(&self, entry: &str) -> bool {
        let pending = self.with_thread(|s| s.pending.clone());
        if let Some(pending) = pending {
            self.log(&format!(
                "{entry}: call blocked while an exception is pending \
                 (clear it with ExceptionClear): {}: {}", pending.class, pending.message
            ));
            return true;
        }
        false
    }

    // -- handle <-> entity resolution

    fn resolve(&self, handle: *mut c_void) -> Option<RefTarget> {
        if handle.is_null() {
            return None;
        }
        let (tag, serial) = handle_parts(handle);
        match tag {
            TAG_GLOBAL => self.globals.lock().unwrap().get(&serial).copied(),
            TAG_WEAK => { let target=self.weaks.lock().unwrap().get(&serial).copied()?; self.target_alive(target).then_some(target) },
            TAG_LOCAL => self.with_thread(|s| s.locals.get(&serial).map(|local| local.target)),
            TAG_METHOD => {
                let methods = self.methods.lock().unwrap();
                methods.by_id.get(&serial).map(|m| RefTarget::Class(m.class))
            }
            _ => None,
        }
    }

    fn target_alive(&self,target:RefTarget)->bool {
        match target {
            RefTarget::Class(_) | RefTarget::ReflectedMethod(_) | RefTarget::ReflectedField(_) => true,
            RefTarget::Str(id)=>self.strings.lock().unwrap().contains_key(&id),
            RefTarget::Throwable(id)=>self.throwables.lock().unwrap().contains_key(&id),
            RefTarget::Object(id)=>self.objects.lock().unwrap().contains_key(&id),
            RefTarget::Array(id)=>self.arrays.lock().unwrap().contains_key(&id),
        }
    }
    /// Resolve and retain in one critical section. A numeric handle snapshot
    /// alone cannot prevent the last global owner from being deleted.
    fn root_reference(&self, handle: *mut c_void) -> Option<ReferenceRoot<'_>> {
        let (tag, id) = handle_parts(handle);
        let target = match tag {
            TAG_LOCAL => self.with_thread(|state| {
                let target = state.locals.get(&id)?.target;
                self.pin_target(target);
                Some(target)
            }),
            TAG_GLOBAL => {
                let globals = self.globals.lock().unwrap();
                let target = globals.get(&id).copied()?;
                // DeleteGlobalRef cannot remove the source until it is pinned.
                self.pin_target(target);
                Some(target)
            }
            TAG_WEAK => {
                let mut live = self.liveness.lock().unwrap();
                let target = self.weaks.lock().unwrap().get(&id).copied()?;
                if !self.target_alive(target) { return None; }
                if let RefTarget::Str(id)|RefTarget::Throwable(id)|RefTarget::Object(id)|RefTarget::Array(id) = target {
                    let count = live.entry(id).or_default();
                    *count = count.checked_add(1).expect("JNI pin overflow");
                }
                Some(target)
            }
            TAG_METHOD => self.resolve(handle),
            _ => None,
        }?;
        Some(ReferenceRoot { session: self, target })
    }

    fn promote_reference(&self, handle: *mut c_void, global: bool) -> *mut c_void {
        if handle.is_null() { return std::ptr::null_mut(); }
        let Some(root) = self.root_reference(handle) else {
            if handle_parts(handle).0 != TAG_WEAK {
                self.set_pending("java/lang/IllegalArgumentException", "invalid reference");
            }
            return std::ptr::null_mut();
        };
        self.new_ref(root.target, global)
    }

    /// Resolve a handle to the CLASS identity it references (null +
    /// pending on any mismatch — explicit, never a guessed default).
    fn resolve_class(&self, handle: *mut c_void, entry: &str) -> Option<u64> {
        match self.resolve(handle) {
            Some(RefTarget::Class(id)) => Some(id),
            other => {
                self.log(&format!(
                    "{entry}: argument is not a live class reference (got {other:?})"
                ));
                self.set_pending("java/lang/NoClassDefFoundError", "not a live class reference");
                None
            }
        }
    }

    fn new_ref(&self, target: RefTarget, global: bool) -> *mut c_void {
        if !global && Self::thread_key().is_none() { return std::ptr::null_mut(); }
        let Some(serial) = self.next_serial() else {
            self.serial_exhausted(if global { "NewGlobalRef" } else { "NewLocalRef" });
            return std::ptr::null_mut();
        };
        self.pin_target(target);
        if global {
            self.globals.lock().unwrap().insert(serial, target);
        } else {
            // Ticket 88: locals are owned by the creating thread, in its
            // innermost local frame.
            self.with_thread(|s| {
                let frame = s.frame;
                s.locals.insert(serial, Local { target, frame })
            });
        }
        make_handle(if global { TAG_GLOBAL } else { TAG_LOCAL }, serial)
    }

    fn delete_ref(&self, handle: *mut c_void, global: bool, entry: &str) {
        if handle.is_null() {
            return;
        }
        let (tag, serial) = handle_parts(handle);
        let expected = if global { TAG_GLOBAL } else { TAG_LOCAL };
        let removed = if tag == expected {
            if global {
                let mut globals = self.globals.lock().unwrap();
                let removed = globals.remove(&serial);
                trim_reference_table(&mut globals);
                removed
            } else {
                // Ticket 88: locals resolve against the calling thread.
                self.with_thread(|s| {
                    let removed = s.locals.remove(&serial).map(|local| local.target);
                    trim_reference_table(&mut s.locals);
                    removed
                })
            }
        } else {
            None
        };
        if removed.is_none() {
            self.log(&format!(
                "{entry}: deleting an unknown or already-deleted reference"
            ));
            self.set_pending(
                "java/lang/NoSuchFieldError",
                "deleting an unknown or already-deleted reference",
            );
            return;
        }
        // Ticket 88 Phase B: deleting the last reference of a string or
        // throwable entity reclaims it.
        if let Some(target) = removed {
            self.release_target(target);
        }
    }

    // -- local frames (libjnivm PushLocalFrame/PopLocalFrame parity)

    fn push_local_frame(&self, capacity: c_int) -> c_int {
        if capacity < 0 {
            self.log(&format!("PushLocalFrame: negative capacity {capacity}"));
            self.set_pending("java/lang/OutOfMemoryError", "negative local frame capacity");
            return abi::JNI_ERR;
        }
        let pushed = self.with_thread(|s| s.frame.checked_add(1).map(|next| s.frame = next).is_some());
        if pushed { abi::JNI_OK } else {
            self.set_pending("java/lang/OutOfMemoryError", "local frame depth exhausted");
            abi::JNI_ERR
        }
    }

    /// Free every local of the innermost frame and hand `result` back as a
    /// fresh local of the now-current frame. With no pushed frame the base
    /// frame is cleared (the reference recreates its top-level frame).
    fn pop_local_frame(&self, result: *mut c_void) -> *mut c_void {
        let (depth, floor) = self.with_thread(|s| (s.frame, s.native_floor));
        if floor.is_some_and(|floor| depth <= floor) {
            self.set_pending("java/lang/IllegalStateException", "cannot pop the automatic native frame");
            return std::ptr::null_mut();
        }
        let output = self.pop_frames_from(depth, depth.saturating_sub(1), result, None);
        if depth == 0 {
            self.log("PopLocalFrame: no pushed frame — cleared the base local frame");
        }
        output
    }

    /// Shared release/promotion path: ordinary Pop removes its top frame;
    /// native return removes all nested native-owned frames and restores the
    /// caller's boundary. The result gains its new pin before any old release.
    fn pop_frames_from(&self, from: usize, caller: usize, result: *mut c_void,
        restore_floor: Option<Option<usize>>) -> *mut c_void {
        let root = if result.is_null() {
            None
        } else {
            match self.root_reference(result) {
                Some(root) => Some(root),
                None => {
                    self.log("PopLocalFrame: result is not a live reference; returning null");
                    None
                }
            }
        };
        // Source promotion retains under the local/global/weak authority
        // lock, closing the concurrent last-global deletion window. This
        // temporary pin survives all frame releases and drops after the
        // destination local has its own pin (or the failed transfer finishes).
        let kept = root.as_ref().map(|root| root.target);
        let serial = match kept {
            Some(_) => match self.next_serial() {
                Some(serial) => Some(serial),
                None => {
                    self.serial_exhausted("PopLocalFrame");
                    None
                }
            },
            None => None,
        };
        // The result gains its outer-frame pin before any removed local
        // releases its pin, including when both reference the same entity.
        self.with_thread(|s| {
            if let (Some(target), Some(_)) = (kept, serial) {
                self.pin_target(target);
            }
            // Lock order is thread state -> liveness -> entity storage.
            // Reclaim never enters thread state or invokes callbacks, so
            // pins can be released directly without a temporary Vec.
            s.locals.retain(|_, local| {
                let keep = local.frame < from;
                if !keep {
                    self.release_target(local.target);
                }
                keep
            });
            s.frame = caller;
            if let Some(floor) = restore_floor { s.native_floor = floor; }
            if let (Some(target), Some(serial)) = (kept, serial) {
                s.locals.insert(serial, Local { target, frame: s.frame });
            }
            trim_reference_table(&mut s.locals);
        });
        self.sweep_probe();
        match serial {
            Some(serial) => make_handle(TAG_LOCAL, serial),
            None => std::ptr::null_mut(),
        }
    }

    fn ensure_local_capacity(&self, capacity: c_int) -> c_int {
        if capacity < 0 {
            self.log(&format!("EnsureLocalCapacity: negative capacity {capacity}"));
            self.set_pending("java/lang/OutOfMemoryError", "negative local capacity");
            return abi::JNI_ERR;
        }
        // The local table grows on demand; nothing to reserve.
        abi::JNI_OK
    }

    // -- monitors (libjnivm: a recursive mutex per object)

    fn monitor_enter(&self, object: *mut c_void) -> c_int {
        let Some(target) = self.resolve(object) else {
            self.log("MonitorEnter: not a live reference");
            self.set_pending("java/lang/NullPointerException", "MonitorEnter on a dead reference");
            return abi::JNI_ERR;
        };
        let Some(key) = Self::thread_key() else {
            return abi::JNI_ERR;
        };
        let mut monitors = self.monitors.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            match monitors.get_mut(&target) {
                None => {
                    monitors.insert(target, (key, 1));
                    return abi::JNI_OK;
                }
                Some((owner, count)) if *owner == key => {
                    let Some(next) = count.checked_add(1) else {
                        drop(monitors);
                        self.log("MonitorEnter: reentry count overflow");
                        self.set_pending("java/lang/IllegalMonitorStateException", "reentry overflow");
                        return abi::JNI_ERR;
                    };
                    *count = next;
                    return abi::JNI_OK;
                }
                Some(_) => {}
            }
            monitors = self
                .monitor_released
                .wait(monitors)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    fn monitor_exit(&self, object: *mut c_void) -> c_int {
        let Some(target) = self.resolve(object) else {
            self.log("MonitorExit: not a live reference");
            self.set_pending("java/lang/NullPointerException", "MonitorExit on a dead reference");
            return abi::JNI_ERR;
        };
        let key = Self::thread_key();
        let mut monitors = self.monitors.lock().unwrap_or_else(|p| p.into_inner());
        match monitors.get_mut(&target) {
            Some((owner, count)) if Some(*owner) == key => {
                *count -= 1;
                if *count == 0 {
                    monitors.remove(&target);
                    self.monitor_released.notify_all();
                }
                abi::JNI_OK
            }
            _ => {
                drop(monitors);
                self.log("MonitorExit: the calling thread does not own this monitor");
                self.set_pending(
                    "java/lang/IllegalMonitorStateException",
                    "MonitorExit without owning the monitor",
                );
                abi::JNI_ERR
            }
        }
    }

    // -- natives (libjnivm RegisterNatives/UnregisterNatives)

    /// Record `(class, name, sig) -> fnPtr` for every entry; rebinding a
    /// name/signature replaces its function. A null class is logged and
    /// answers JNI_OK like the reference.
    ///
    /// # Safety
    /// `methods` must point to `count` readable JNINativeMethod entries
    /// whose strings are NUL-terminated. Each callback has the exact ABI
    /// declared by its descriptor, stays mapped for the process and obeys
    /// JNI reference, thread and buffer lifetime contracts.
    unsafe fn register_natives(
        &self,
        class: *mut c_void,
        methods: *const JniNativeMethod,
        count: c_int,
    ) -> c_int {
        if class.is_null() {
            self.log("RegisterNatives failed, class is null");
            return abi::JNI_OK;
        }
        let Some(class_id) = self.resolve_class(class, "RegisterNatives") else {
            return abi::JNI_ERR;
        };
        if count < 0 || (count > 0 && methods.is_null()) {
            self.log(&format!(
                "RegisterNatives: invalid method table (count {count}, null table: {})",
                methods.is_null()
            ));
            return abi::JNI_ERR;
        }
        let mut bound = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            // SAFETY: caller contract — `count` readable entries.
            let method = unsafe { &*methods.add(index) };
            if method.name.is_null() || method.signature.is_null() || method.fn_ptr.is_null() {
                self.log("RegisterNatives: null method name, signature or function");
                self.set_pending("java/lang/NoSuchMethodError", "null native binding");
                return abi::JNI_ERR;
            }
            // Our VM tokens are intentionally not executable addresses. Catch
            // an accidentally shadowed callback before any unsafe native call.
            if matches!(handle_parts(method.fn_ptr).0,TAG_LOCAL|TAG_GLOBAL|TAG_WEAK|TAG_METHOD|TAG_FIELD){
                self.log("RegisterNatives: JNI handle supplied as a native callback");
                self.set_pending("java/lang/NoSuchMethodError","JNI handle is not a native callback");
                return abi::JNI_ERR;
            }
            // SAFETY: non-null NUL-terminated strings per the ABI.
            let (name, sig) = unsafe {
                (
                    CStr::from_ptr(method.name).to_string_lossy().into_owned(),
                    CStr::from_ptr(method.signature).to_string_lossy().into_owned(),
                )
            };
            let Some(serial)=self.next_serial() else {
                self.serial_exhausted("RegisterNatives");
                return abi::JNI_ERR;
            };
            let address=method.fn_ptr as usize;
            if self.trace_natives.load(Ordering::Relaxed) {
                self.log(&format!("native binding {}/{}{}", self.class_name(class_id), name, sig));
            }
            let value=match sig.as_str() {
                s if s.starts_with("(II)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::TwoInts),
                "(III)Lorg/rudroid/runtime/AudioOutput;" => MethodValue::NativeObject(address,ObjectShape::ThreeInts),
                "()[I" => MethodValue::NativeObject(address,ObjectShape::NoArgs),
                "()F" => MethodValue::NativeFloatNoArgs(address),
                "(I)F" => MethodValue::NativeFloatInt(address),
                "(II)F" => MethodValue::NativeFloatTwoInts(address),
                "(Ljava/lang/String;F)F" => MethodValue::NativeFloatObjectFloat(address),
                "()J" => MethodValue::NativeLongNoArgs(address),
                "(Ljava/lang/String;J)J" => MethodValue::NativeLongObjectLong(address),
                "(Ljava/lang/String;Z)Z" => MethodValue::NativeBoolObjectBoolean(address),
                "(Ljava/lang/Runnable;J)Z" | "(Landroid/os/Message;J)Z" => MethodValue::NativeBoolObjectLong(address),
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I" => MethodValue::NativeInt(address,IntShape::ThreeReferences),
                s if (s.starts_with("(I)L") || s.starts_with("(I)["))&&s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::Int),
                "(Ljava/lang/Throwable;)Ljava/lang/String;" => MethodValue::NativeObject(address,ObjectShape::String),
                "(III)I" => MethodValue::NativeInt(address,IntShape::ThreeInts),
                "(I)I" => MethodValue::NativeInt(address,IntShape::Int),
                "(II)I" => MethodValue::NativeInt(address,IntShape::TwoInts),
                "(Ljava/nio/ByteBuffer;)I" => MethodValue::NativeInt(address,IntShape::Reference),
                "(Ljava/nio/ByteBuffer;I)I" => MethodValue::NativeInt(address,IntShape::ReferenceInt),
                "()I" => MethodValue::NativeInt(address,IntShape::NoArgs),
                "([B)I" | "(Ljava/lang/String;)I" | "(Ljava/lang/Object;)I" => MethodValue::NativeInt(address,IntShape::Reference),
                "(Ljava/lang/String;I)I" => MethodValue::NativeInt(address,IntShape::ReferenceInt),
                "([BII)I" => MethodValue::NativeInt(address,IntShape::ReferenceIntInt),
                s if s.starts_with("(Ljava/lang/String;I)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::StringInt),
                s if s.starts_with("(Ljava/lang/String;J)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::StringLong),
                s if s.starts_with("(Ljava/lang/String;F)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::StringFloat),
                s if s.starts_with("(Ljava/lang/String;Z)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::StringBoolean),
                "(Ljava/lang/String;)Z" | "(Ljava/lang/Object;)Z" | "(Ljava/lang/Runnable;)Z" | "(Landroid/os/Message;)Z" => MethodValue::NativeBoolString(address),
                "()Z" => MethodValue::NativeBoolNoArgs(address),
                "(I)Z" => MethodValue::NativeBoolInt(address),
                "()V" => MethodValue::NativeVoid(address, VoidShape::NoArgs),
                "(Z)V" => MethodValue::NativeVoid(address, VoidShape::Boolean),
                "(I)V" => MethodValue::NativeVoid(address, VoidShape::Int),
                "(F)V" => MethodValue::NativeVoid(address, VoidShape::Float),
                "(J)V" => MethodValue::NativeVoid(address, VoidShape::Long),
                "(II)V" => MethodValue::NativeVoid(address, VoidShape::IntInt),
                "(Landroid/content/Context;)V" => MethodValue::NativeVoid(address, VoidShape::Context),
                "(Landroid/content/Context;I)V" => MethodValue::NativeVoid(address, VoidShape::ContextInt),
                "(ILandroid/view/Surface;)V" => MethodValue::NativeVoid(address, VoidShape::IntSurface),
                "(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)V" => MethodValue::NativeVoid(address, VoidShape::FieldDeclaration),
                "([BLjava/lang/String;)V" => MethodValue::NativeVoid(address,VoidShape::TwoReferences),
                "(Ljava/io/InputStream;Ljava/lang/String;)V" => MethodValue::NativeVoid(address,VoidShape::TwoReferences),
                "(Ljava/lang/Class;Ljava/lang/Class;)V" => MethodValue::NativeVoid(address, VoidShape::TwoReferences),
                "(Landroid/os/Looper;Landroid/os/Handler$Callback;)V" => MethodValue::NativeVoid(address, VoidShape::TwoReferences),
                s if (s.starts_with("()L") || s.starts_with("()[")) && s.ends_with(';') => MethodValue::NativeObject(address, ObjectShape::NoArgs),
                s if s.starts_with("(Ljava/lang/String;)L") && s.ends_with(';') => MethodValue::NativeObject(address, ObjectShape::String),
                s if s.starts_with("(Ljava/lang/String;Ljava/lang/String;)L") && s.ends_with(';') => MethodValue::NativeObject(address,ObjectShape::TwoObjects),
                "(Ljava/lang/Class;[Ljava/lang/Class;)Ljava/lang/Class;" => MethodValue::NativeObject(address,ObjectShape::TwoObjects),
                "(Ljava/lang/Class;Ljava/lang/String;)Ljava/lang/reflect/Constructor;" => MethodValue::NativeObject(address,ObjectShape::TwoObjects),
                "(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;" => MethodValue::NativeObject(address,ObjectShape::ClassStringsBoolean),
                "(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;" => MethodValue::NativeObject(address,ObjectShape::ClassStringsBoolean),
                "(Ljava/lang/reflect/Field;)Ljava/lang/String;" => MethodValue::NativeObject(address,ObjectShape::String),
                "(Lcom/unity3d/player/UnityPlayer;Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;" => MethodValue::NativeObject(address, ObjectShape::TwoObjects),
                "(JLjava/lang/Class;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;" => MethodValue::NativeObject(address,ObjectShape::LongThreeObjects),
                "(JLjava/lang/String;[Ljava/lang/Object;)Ljava/lang/Object;" => MethodValue::NativeObject(address,ObjectShape::LongTwoObjects),
                "(Lcom/unity3d/player/UnityPlayer;JLjava/lang/Class;)Ljava/lang/Object;" => MethodValue::NativeObject(address,ObjectShape::ObjectLongObject),
                "(JZ)Ljava/lang/Object;" => MethodValue::NativeObject(address,ObjectShape::LongBoolean),
                "(Lcom/unity3d/player/UnityPlayer;J[Ljava/lang/Class;)Ljava/lang/Object;" => MethodValue::NativeObject(address,ObjectShape::ObjectLongObject),
                "(J[Ljava/lang/Class;)Ljava/lang/Object;" => MethodValue::NativeObject(address, ObjectShape::LongObject),
                s if s.starts_with("(L") && s.ends_with(";)V") && s.matches(';').count() == 1 => MethodValue::NativeVoid(address, VoidShape::Context),
                "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;" => MethodValue::NativeObject(address, ObjectShape::StringBoolObject),
                "(Landroid/hardware/display/DisplayManager$DisplayListener;Landroid/os/Handler;)V" => MethodValue::NativeVoid(address, VoidShape::TwoReferences),
                _ => scalar_calls::classify(&sig,address).or_else(||reference_native(&sig,address)).or_else(||mixed_void_native(&sig,address)).unwrap_or(MethodValue::NativeOther(address)),
            };
            bound.push((serial,name.into_boxed_str(),sig.into_boxed_str(),value));
        }
        // Validate/reserve the complete table before publishing anything.
        // Replacement preserves existing jmethodIDs and updates their single
        // authority. Guest calls snapshot addresses, then release the lock.
        {
            let mut methods=self.methods.lock().unwrap_or_else(|p| p.into_inner());
            methods.aliases.clear();
            for (serial,name,sig,value) in bound {
                if let Some(existing)=methods.find(class_id,&name,&sig) {
                    methods.by_id.get_mut(&existing).unwrap().value=value;
                } else {
                    methods.insert(serial,Method { class:class_id,name,sig,value,next_collision:0 });
                }
            }
        }
        self.log(&format!(
            "RegisterNatives: {count} native method(s) bound on {}",
            self.class_name(class_id)
        ));
        abi::JNI_OK
    }

    fn unregister_natives(&self, class: *mut c_void) -> c_int {
        if class.is_null() {
            self.log("UnregisterNatives failed, class is null");
            return abi::JNI_OK;
        }
        let Some(class_id) = self.resolve_class(class, "UnregisterNatives") else {
            return abi::JNI_ERR;
        };
        let mut methods=self.methods.lock().unwrap_or_else(|p| p.into_inner());
        for method in methods.by_id.values_mut() {
            if method.class==class_id && matches!(method.value,
                MethodValue::NativeBoolString(_) | MethodValue::NativeBoolNoArgs(_) | MethodValue::NativeBoolInt(_) | MethodValue::NativeVoid(_, _) | MethodValue::NativeObject(_, _) | MethodValue::NativeInt(_,_) | MethodValue::NativeScalar(_,_,_) | MethodValue::NativeLongNoArgs(_) | MethodValue::NativeLongObjectLong(_) | MethodValue::NativeFloatNoArgs(_) | MethodValue::NativeFloatInt(_) | MethodValue::NativeFloatTwoInts(_) | MethodValue::NativeFloatObjectFloat(_) | MethodValue::NativeBoolObjectLong(_) | MethodValue::NativeBoolObjectBoolean(_) | MethodValue::NativeOther(_)) {
                method.value=MethodValue::NativeUnbound;
            }
        }
        abi::JNI_OK
    }

    /// The function a guest bound to `class.name(sig)` through
    /// RegisterNatives, if any.
    pub fn registered_native(&self, class: &str, name: &str, sig: &str) -> Option<usize> {
        let class_id = self
            .classes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .by_name
            .get(class)
            .copied()?;
        let methods=self.methods.lock().unwrap_or_else(|p| p.into_inner());
        let serial=methods.find(class_id,name,sig)?;
        match methods.by_id.get(&serial)?.value {
            MethodValue::NativeBoolString(address) | MethodValue::NativeBoolNoArgs(address) | MethodValue::NativeBoolInt(address)
            | MethodValue::NativeVoid(address, _)
            | MethodValue::NativeObject(address, _)
            | MethodValue::NativeScalar(address,_,_)
            | MethodValue::NativeInt(address,_) | MethodValue::NativeLongNoArgs(address) | MethodValue::NativeLongObjectLong(address) | MethodValue::NativeFloatNoArgs(address) | MethodValue::NativeFloatInt(address) | MethodValue::NativeFloatTwoInts(address) | MethodValue::NativeFloatObjectFloat(address) | MethodValue::NativeBoolObjectLong(address) | MethodValue::NativeBoolObjectBoolean(address)
            | MethodValue::NativeOther(address) => Some(address),
            _ => None,
        }
    }

    // -- throwables

    /// A handle to the calling thread's pending exception; records the
    /// throwable's class/message so `Throw` can re-raise the same object.
    fn exception_occurred(&self) -> *mut c_void {
        let pending = self.with_thread(|s| s.pending.clone());
        let Some(pending) = pending else {
            return std::ptr::null_mut();
        };
        self.throwables
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(pending.id)
            .or_insert_with(|| Arc::clone(&pending));
        self.new_ref(RefTarget::Throwable(pending.id), false)
    }

    /// Throw(obj): the referenced throwable becomes the pending exception
    /// with its own identity, class and message. A non-throwable or dead
    /// reference fails without raising anything.
    fn throw(&self, object: *mut c_void) -> c_int {
        let id = match self.resolve(object) {
            Some(RefTarget::Throwable(id)) => id,
            other => {
                self.log(&format!("Throw: not a live throwable reference (got {other:?})"));
                return abi::JNI_ERR;
            }
        };
        let pending = self
            .throwables
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Arc::new(Pending {
                class: "java/lang/Throwable".to_owned(), message: "thrown object".to_owned(), id,
            }));
        self.replace_pending(pending);
        abi::JNI_OK
    }

    /// Drop the calling thread's pending exception (Clear/Describe).
    fn take_pending(&self) -> Option<Arc<Pending>> {
        let pending = self.with_thread(|s| s.pending.take());
        if let Some(pending) = &pending {
            self.release_target(RefTarget::Throwable(pending.id));
        }
        pending
    }

    /// Borrow the entity's immutable NUL-terminated data. References and
    /// borrows share a pin count; removing every jobject cannot free this
    /// allocation until all matching releases have returned.
    fn borrow_string_chars(&self, string: *mut c_void) -> *const c_char {
        self.borrow_string_data(string, false).cast()
    }

    fn release_string_chars(&self, chars: *const c_char) {
        if chars.is_null() {
            self.log("ReleaseStringUTFChars: null chars pointer");
            return;
        }
        let mut borrows = self.string_chars.lock().unwrap_or_else(|p| p.into_inner());
        let Some((entity_id, count)) = borrows.get_mut(&(chars as usize)) else {
            drop(borrows);
            self.log("ReleaseStringUTFChars: chars pointer is not an outstanding borrow");
            return;
        };
        let entity_id = *entity_id;
        *count -= 1;
        if *count == 0 { borrows.remove(&(chars as usize)); }
        trim_reference_table(&mut borrows);
        drop(borrows);
        self.release_target(RefTarget::Str(entity_id));
    }

    fn pin_target(&self, target: RefTarget) {
        let (RefTarget::Str(id) | RefTarget::Throwable(id) | RefTarget::Object(id) | RefTarget::Array(id)) = target else { return; };
        let mut live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        let count = live.entry(id).or_insert(0);
        *count = count.checked_add(1).expect("JNI entity pin count overflow");
    }

    /// Release exactly one reference, pending-exception owner or UTF borrow.
    /// Holding the count lock through removal prevents another pin racing
    /// with reclaim; no globals or other threads need to be scanned.
    fn release_target(&self, target: RefTarget) { self.release_target_inner(target, false); }

    // Heap-to-heap edges are traced in place. Only external JNI roots enter
    // liveness for objects/arrays; immutable leaf storage remains pin-counted.
    fn retain_edge(&self, target: RefTarget) {
        if matches!(target, RefTarget::Str(_) | RefTarget::Throwable(_)) { self.pin_target(target); return; }
        // Publication callers hold a source/temporary root. Serialize incoming
        // edge accounting with collection before publishing the owner's edge.
        let _live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        match target {
            RefTarget::Object(id) => { let objects = self.objects.lock().unwrap(); let count = &objects[&id].incoming; count.set(count.get().checked_add(1).expect("heap edge overflow")); }
            RefTarget::Array(id) => { let arrays = self.arrays.lock().unwrap(); let count = &arrays[&id].incoming; count.set(count.get().checked_add(1).expect("heap edge overflow")); }
            _ => {}
        }
    }

    fn release_target_inner(&self, target: RefTarget, edge: bool) {
        let (RefTarget::Str(id) | RefTarget::Throwable(id) | RefTarget::Object(id) | RefTarget::Array(id)) = target else { return; };
        let mut live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(target, RefTarget::Object(_) | RefTarget::Array(_)) {
            if !edge {
                let count = live.get_mut(&id).expect("JNI external root released twice");
                *count = count.checked_sub(1).expect("JNI external root underflow");
            }
            if !edge && live.get(&id).copied().unwrap_or(0) != 0 { return; }
            let mut objects = self.objects.lock().unwrap();
            let mut arrays = self.arrays.lock().unwrap();
            if edge { Self::release_graph_edge(target, &objects, &arrays); }
            if live.get(&id).copied().unwrap_or(0) != 0 { return; }
            let graph_edge = |target| matches!(target, RefTarget::Object(_) | RefTarget::Array(_));
            let (incoming, has_children) = match target {
                RefTarget::Object(id) => { let object = &objects[&id]; (object.incoming.get(), object.fields.values().filter_map(|v|v.reference()).any(graph_edge)) }
                RefTarget::Array(id) => { let array = &arrays[&id]; (array.incoming.get(), array.elements.iter().flatten().copied().any(graph_edge)) }
                _ => unreachable!(),
            };
            if incoming == 0 {
                if self.reclaim_unreferenced_graph(target, &mut live, &mut objects, &mut arrays) {
                    self.collect_objects(&mut live, &mut objects, &mut arrays);
                }
                trim_reference_table(&mut objects); trim_reference_table(&mut arrays); trim_reference_table(&mut live);
            } else if !has_children {
                // A node with no outgoing graph edges cannot be the sole path
                // to another owner. Removing its root therefore cannot make
                // any other graph node unreachable. Incoming heap edges keep
                // it alive; otherwise reclaim it immediately without tracing.
                // Its incoming owners remain reachable without this leaf.
            } else {
                self.collect_objects(&mut live, &mut objects, &mut arrays);
            }
            return;
        }
        let count = live.get_mut(&id).expect("JNI leaf released without a pin");
        *count = count.checked_sub(1).expect("JNI leaf pin underflow");
        if *count == 0 { live.remove(&id); self.reclaim_leaf(target); trim_reference_table(&mut live); }
    }

    fn reclaim_leaf(&self, target: RefTarget) {
        match target {
            RefTarget::Str(id) => {
                let mut strings = self.strings.lock().unwrap();
                strings.remove(&id); trim_reference_table(&mut strings);
            }
            RefTarget::Throwable(id) => {
                let mut throwables = self.throwables.lock().unwrap();
                throwables.remove(&id); trim_reference_table(&mut throwables);
            }
            RefTarget::Class(_) | RefTarget::ReflectedMethod(_) | RefTarget::ReflectedField(_) => {}
            _ => unreachable!("object/array requires graph collection"),
        }
    }

    fn release_graph_edge(target: RefTarget, objects: &HashMap<u64, fields::Object>, arrays: &HashMap<u64, objects::ObjectArray>) {
        let count = match target {
            RefTarget::Object(id) => &objects[&id].incoming,
            RefTarget::Array(id) => &arrays[&id].incoming,
            _ => return,
        };
        count.set(count.get().checked_sub(1).expect("heap edge underflow"));
    }

    fn release_heap_leaf(&self, live: &mut HashMap<u64, usize>, target: RefTarget) {
        let (RefTarget::Str(child) | RefTarget::Throwable(child)) = target else { return; };
        let count = live.get_mut(&child).expect("heap leaf edge is pinned");
        *count = count.checked_sub(1).expect("heap leaf edge underflow");
        if *count == 0 { live.remove(&child); self.reclaim_leaf(target); }
    }

    /// Peel only nodes proven unowned by both JNI roots and heap edges. A
    /// residual zero-root child with graph children may be part of a dead
    /// cycle, so request the ordinary full trace after the peel completes.
    fn reclaim_unreferenced_graph(&self, target: RefTarget, live: &mut HashMap<u64, usize>, objects: &mut HashMap<u64, fields::Object>, arrays: &mut HashMap<u64, objects::ObjectArray>) -> bool {
        let mut head = Some(target);
        let mut trace_required = false;
        while let Some(target) = head {
            match target {
                RefTarget::Object(id) => {
                    let object = objects.remove(&id).expect("zero-incoming object exists");
                    head = object.trace_next.take(); live.remove(&id);
                    for child in object.fields.into_values().filter_map(|v|v.reference()) { trace_required |= self.release_unreferenced_child(child, &mut head, live, objects, arrays); }
                }
                RefTarget::Array(id) => {
                    let array = arrays.remove(&id).expect("zero-incoming array exists");
                    head = array.trace_next.take(); live.remove(&id);
                    for child in array.elements.into_iter().flatten() { trace_required |= self.release_unreferenced_child(child, &mut head, live, objects, arrays); }
                }
                _ => unreachable!("only graph nodes enter the reclamation list"),
            }
        }
        trace_required
    }

    fn release_unreferenced_child(&self, child: RefTarget, head: &mut Option<RefTarget>, live: &mut HashMap<u64, usize>, objects: &HashMap<u64, fields::Object>, arrays: &HashMap<u64, objects::ObjectArray>) -> bool {
        Self::release_graph_edge(child, objects, arrays);
        self.release_heap_leaf(live, child);
        let (id, incoming, next) = match child {
            RefTarget::Object(id) => { let object = &objects[&id]; (id, &object.incoming, &object.trace_next) }
            RefTarget::Array(id) => { let array = &arrays[&id]; (id, &array.incoming, &array.trace_next) }
            _ => return false,
        };
        if live.get(&id).copied().unwrap_or(0) != 0 { return false; }
        if incoming.get() == 0 { next.set(*head); *head = Some(child); return false; }
        match child {
            RefTarget::Object(id) => objects[&id].fields.values().filter_map(|v|v.reference()).any(|t|matches!(t,RefTarget::Object(_)|RefTarget::Array(_))),
            RefTarget::Array(id) => arrays[&id].elements.iter().flatten().any(|t|matches!(t,RefTarget::Object(_)|RefTarget::Array(_))),
            _ => unreachable!(),
        }
    }

    fn collect_objects(&self, live: &mut HashMap<u64, usize>, objects: &mut HashMap<u64, fields::Object>, arrays: &mut HashMap<u64, objects::ObjectArray>) -> usize {
        for object in objects.values() { object.marked.set(false); object.trace_next.set(None); }
        for array in arrays.values() { array.marked.set(false); array.trace_next.set(None); }
        // Intrusive work list: each reachable node is expanded once, including
        // shared children and cycles. No allocation or recursive Rust stack;
        // all scratch links stay protected by the same collector locks.
        let mark = |target, head: &mut Option<RefTarget>| {
            let (marked, next) = match target {
                RefTarget::Object(id) => { let object = objects.get(&id).expect("live object edge"); (&object.marked, &object.trace_next) }
                RefTarget::Array(id) => { let array = arrays.get(&id).expect("live array edge"); (&array.marked, &array.trace_next) }
                _ => return,
            };
            if !marked.replace(true) { next.set(*head); *head = Some(target); }
        };
        let mut head = None;
        for id in objects.keys().filter(|id| live.get(id).copied().unwrap_or(0) != 0) { mark(RefTarget::Object(*id), &mut head); }
        for id in arrays.keys().filter(|id| live.get(id).copied().unwrap_or(0) != 0) { mark(RefTarget::Array(*id), &mut head); }
        let mut expanded = 0;
        while let Some(target) = head {
            expanded += 1;
            match target {
                RefTarget::Object(id) => {
                    let object = &objects[&id]; head = object.trace_next.take();
                    for target in object.fields.values().filter_map(|v| v.reference()) { mark(target, &mut head); }
                }
                RefTarget::Array(id) => {
                    let array = &arrays[&id]; head = array.trace_next.take();
                    for target in array.elements.iter().flatten() { mark(*target, &mut head); }
                }
                _ => unreachable!("only graph nodes enter the trace list"),
            }
        }
        // All dead owners are still present while their outgoing leaves are
        // released. No recursive collector or in-flight edge counters exist.
        for object in objects.values().filter(|o| !o.marked.get()) {
            for target in object.fields.values().filter_map(|v| v.reference()) { Self::release_graph_edge(target, objects, arrays); self.release_heap_leaf(live,target); }
        }
        for array in arrays.values().filter(|a| !a.marked.get()) {
            for target in array.elements.iter().flatten() { Self::release_graph_edge(*target, objects, arrays); self.release_heap_leaf(live,*target); }
        }
        objects.retain(|id,object| { if object.marked.get() { true } else { live.remove(id); false } });
        arrays.retain(|id,array| { if array.marked.get() { true } else { live.remove(id); false } });
        trim_reference_table(objects); trim_reference_table(arrays); trim_reference_table(live);
        expanded
    }

    // -- class / method resolution

    fn is_subclass(&self, child: u64, parent: u64) -> bool {
        Self::assignable(&mut self.classes.lock().unwrap(), child, parent)
    }
    fn is_assignable_from(&self, source: *mut c_void, target: *mut c_void) -> u8 {
        if self.blocked_by_pending("IsAssignableFrom") { return abi::JNI_FALSE; }
        let (Some(source), Some(target)) = (
            self.resolve_class(source, "IsAssignableFrom"),
            self.resolve_class(target, "IsAssignableFrom"),
        ) else { return abi::JNI_FALSE; };
        u8::from(self.is_subclass(source, target))
    }
    fn assignable(classes:&mut ClassTable,child:u64,parent:u64)->bool {
        classes.visit_epoch=classes.visit_epoch.wrapping_add(1);
        if classes.visit_epoch==0{for value in classes.visits.values_mut(){*value=0;}classes.visit_epoch=1;}
        fn walk(classes:&mut ClassTable,mut child:u64,parent:u64,depth:usize,epoch:u64)->bool {
            if depth>=64{return false;}if child==parent{return true;}
            let child_name=Arc::clone(&classes.by_id[&child]);let parent_name=Arc::clone(&classes.by_id[&parent]);
            if child_name.starts_with('[') {
                if matches!(parent_name.as_ref(),"java/lang/Object"|"java/lang/Cloneable"|"java/io/Serializable"){return true;}
                if parent_name.starts_with('['){
                    let component=|name:&str|{let c=&name[1..];let c=if c.starts_with('L')&&c.ends_with(';'){&c[1..c.len()-1]}else if c.starts_with('['){c}else{return None;};classes.by_name.get(c).copied()};
                    return match(component(&child_name),component(&parent_name)){(Some(c),Some(p))=>walk(classes,c,p,depth+1,epoch),_=>false};
                }
            }
            for level in depth..64 {
                if child==parent{return true;}
                let Some(visit)=classes.visits.get_mut(&child) else{return false;};if *visit==epoch{return false;}*visit=epoch;
                let count=classes.interfaces.get(&child).map_or(0,Vec::len);
                for i in 0..count {let interface=classes.interfaces[&child][i];if walk(classes,interface,parent,level+1,epoch){return true;}}
                let Some(next)=classes.parents.get(&child) else{return false;};child=*next;
            }false
        }
        walk(classes,child,parent,0,classes.visit_epoch)
    }

    fn set_superclass(&self, child: *mut c_void, parent: *mut c_void) {
        let (Some(child), Some(parent)) = (
            self.resolve_class(child, "setSuperclass"), self.resolve_class(parent, "setSuperclass")
        ) else { return; };
        let mut classes = self.classes.lock().unwrap();
        if Self::assignable(&mut classes,parent,child){drop(classes);self.set_pending("java/lang/ClassCircularityError","cyclic superclass/interface registration");return;}
        let mut ancestor = parent;
        for depth in 0..64 {
            if ancestor == child || depth == 63 {
                drop(classes);
                self.set_pending("java/lang/ClassCircularityError", "cyclic or overly deep superclass registration");
                return;
            }
            let Some(next) = classes.parents.get(&ancestor) else { break; };
            ancestor = *next;
        }
        let object = classes.by_name.get("java/lang/Object").copied();
        if classes.parents.get(&child).is_some_and(|previous| *previous != parent && Some(*previous) != object) {
            drop(classes);
            self.set_pending("java/lang/LinkageError", "class superclass already defined");
            return;
        }
        classes.parents.insert(child, parent);
        drop(classes);
        self.methods.lock().unwrap().aliases.clear();
    }

    fn proxy_class(&self, base: *mut c_void, interfaces: *mut c_void) -> Option<*mut c_void> {
        let base = self.resolve_class(base, "proxyClass")?;
        let Some(RefTarget::Array(array)) = self.resolve(interfaces) else {
            self.set_pending("java/lang/IllegalArgumentException", "proxy interfaces must be Class[]");
            return None;
        };
        let expected=self.find_class("[Ljava/lang/Class;")?;let expected_id=self.resolve_class(expected,"proxyClass")?;self.delete_ref(expected,false,"proxyClass");
        let mut members = {
            let arrays = self.arrays.lock().unwrap();
            let array = arrays.get(&array)?;
            let mut members = Vec::new();
            if array.class!=expected_id || array.primitive.is_some() {
                drop(arrays);
                self.set_pending("java/lang/IllegalArgumentException", "invalid proxy interface array");
                return None;
            }
            if members.try_reserve(array.elements.len()).is_err(){drop(arrays);self.set_pending("java/lang/OutOfMemoryError","proxy interface metadata allocation failed");return None;}
            for element in &array.elements {
                let Some(RefTarget::Class(class)) = element else {
                    drop(arrays);
                    self.set_pending("java/lang/IllegalArgumentException", "proxy interface element must be Class");
                    return None;
                };
                members.push(*class);
            }
            members
        };
        members.sort_unstable(); members.dedup();
        // Class entity IDs are stable and never reused. Equivalent interface
        // sets share one nominal type, independently of local-reference IDs.
        let mut name = format!("org/rudroid/runtime/Proxy${base}");
        for member in &members { use std::fmt::Write; write!(name, "${member}").unwrap(); }
        let class = self.find_class(&name)?;
        let id = self.resolve_class(class, "proxyClass")?;
        let mut classes = self.classes.lock().unwrap();
        classes.parents.insert(id, base);
        classes.interfaces.entry(id).or_insert(members);
        drop(classes);
        Some(class)
    }


    fn alloc_object(&self, class: *mut c_void) -> *mut c_void {
        if self.blocked_by_pending("AllocObject") { return std::ptr::null_mut(); }
        let Some(class) = self.resolve_class(class, "AllocObject") else { return std::ptr::null_mut(); };
        let Some(id) = self.next_serial() else {
            self.serial_exhausted("AllocObject");
            return std::ptr::null_mut();
        };
        let handle = self.new_ref(RefTarget::Object(id), false);
        if !handle.is_null() { self.objects.lock().unwrap().insert(id, fields::Object::new(class)); }
        handle
    }

    fn object_class(&self, object: *mut c_void, entry: &str) -> Option<u64> {
        match self.resolve(object) {
            Some(RefTarget::Object(id)) => self.objects.lock().unwrap().get(&id).map(|o| o.class),
            Some(RefTarget::Array(id)) => self.arrays.lock().unwrap().get(&id).map(|a| a.class),
            Some(RefTarget::Str(_)) => {
                let class = self.find_class("java/lang/String")?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            Some(RefTarget::ReflectedMethod(id)) => {
                let constructor=self.methods.lock().unwrap().by_id.get(&id).is_some_and(|method|method.name.as_ref()=="<init>");
                let class = self.find_class(if constructor{"java/lang/reflect/Constructor"}else{"java/lang/reflect/Method"})?;
                let id = self.resolve_class(class,entry); self.delete_ref(class,false,entry); id
            }
            Some(RefTarget::ReflectedField(_)) => {let class=self.find_class("java/lang/reflect/Field")?;let id=self.resolve_class(class,entry);self.delete_ref(class,false,entry);id}
            Some(RefTarget::Class(_)) => {
                let class = self.find_class("java/lang/Class")?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            Some(RefTarget::Throwable(id)) => {
                let name = self.throwables.lock().unwrap().get(&id).map(|e| e.class.clone())?;
                let class = self.find_class(&name)?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            _ => {
                self.set_pending("java/lang/NullPointerException", "not a live object reference");
                None
            }
        }
    }

    fn find_class(&self, name: &str) -> Option<*mut c_void> {
        if self.blocked_by_pending("FindClass") {
            return None;
        }
        if name.is_empty() {
            self.log("FindClass: empty class name");
            self.set_pending("java/lang/NoClassDefFoundError", "empty class name");
            return None;
        }
        if let Some(component) = name.strip_prefix('[') {
            let component = if component.starts_with('L') && component.ends_with(';') { Some(&component[1..component.len()-1]) } else if component.starts_with('[') { Some(component) } else { None };
            if let Some(component) = component {
                let handle = self.find_class(component)?; self.delete_ref(handle,false,"FindClass component");
            }
        }
        // Auto-register the class (jnivm parity): names are formed like
        // `android/media/AudioManager`; any well-formed name becomes a
        // session class. Values still come only from the catalog.
        let mut classes = self.classes.lock().unwrap();
        let existing = classes.by_name.get(name).copied();
        let id = match existing {
            Some(id) => id,
            None => {
                let Some(id) = self.next_serial() else {
                    drop(classes);
                    self.serial_exhausted("FindClass");
                    return None;
                };
                let name: Arc<str> = Arc::from(name);
                classes.by_name.insert(Arc::clone(&name), id);
                classes.by_id.insert(id, name);
                classes.visits.insert(id,0);
                if let Some(object) = classes.by_name.get("java/lang/Object").copied() {
                    if object != id { classes.parents.insert(id, object); }
                }
                id
            }
        };
        drop(classes);
        let handle = self.new_ref(RefTarget::Class(id), false);
        (!handle.is_null()).then_some(handle)
    }

    /// Shared resolution for GetMethodID/GetStaticMethodID: the catalog
    /// answers `(class, method, sig)`; a miss is null + pending
    /// NoSuchMethodError (spec behavior for both unknown methods and wrong
    /// signatures — the v1 registry API has no enumeration to tell them
    /// apart; broader class-table diagnostics stay ticket 40).
    fn resolve_method(
        &self,
        class: *mut c_void,
        name: &str,
        sig: &str,
        entry: &str,
    ) -> Option<*mut c_void> {
        if self.blocked_by_pending(entry) {
            return None;
        }
        let class_id = self.resolve_class(class, entry)?;
        if name.is_empty() || sig.is_empty() {
            self.log(&format!("{entry}: empty method name or signature"));
            self.set_pending("java/lang/NoSuchMethodError", "empty method name or signature");
            return None;
        }
        self.resolve_method_for_class(class_id, name, sig, entry)
    }

    fn method_declarations(&self, class_id: u64, name: &str) -> Vec<u64> {
        {let classes=self.classes.lock().unwrap();let mut order=vec![class_id];
            if name!="<init>" {
                let mut class=class_id;for _ in 0..64 {let Some(parent)=classes.parents.get(&class).copied() else{break;};if order.contains(&parent){break;}order.push(parent);class=parent;}
                let mut i=0;while i<order.len(){let class=order[i];
                    if let Some(interfaces)=classes.interfaces.get(&class){for interface in interfaces{if !order.contains(interface){order.push(*interface);}}}
                    if let Some(parent)=classes.parents.get(&class){if !order.contains(parent){order.push(*parent);}}i+=1;
                }
            }order}
    }

    fn reflected_method_id(&self, class: *mut c_void, name: &str, signature: &str) -> Option<*mut c_void> {
        if self.blocked_by_pending("reflectMethod") { return None; }
        let class_id = self.resolve_class(class, "reflectMethod")?;
        let normalized;
        let signature=if signature.contains('.') {normalized=signature.replace('.',"/");normalized.as_str()}else{signature};
        let declarations=self.method_declarations(class_id,name);
        // An inherited exact overload outranks a child assignable overload.
        for declaring in &declarations {
            if let Some(id)=self.resolve_declared_method(*declaring,name,signature,"reflectMethod"){return Some(make_handle(TAG_METHOD,id));}
            if self.with_thread(|t|t.pending.is_some()){return None;}
        }
        let mut best=None;let mut ambiguous=false;
        for declaring in declarations {
            let candidates = {
                let methods = self.methods.lock().unwrap();
                if let Some(id)=methods.find(declaring,name,signature){return Some(make_handle(TAG_METHOD,id));}
                methods.by_id.iter().filter(|(_,m)|m.class==declaring&&m.name.as_ref()==name).map(|(id,m)|(*id,m.sig.clone())).collect::<Vec<_>>()
            };
            for (id,declared) in candidates {
                if let Some(score)=self.reflection_score(&declared,signature){
                    match best {
                        None=>{best=Some((id,score,declared));ambiguous=false;},
                        Some((_,current,_)) if score>current=>{best=Some((id,score,declared));ambiguous=false;},
                        Some((_,current,ref previous)) if score==current&&previous!=&declared=>ambiguous=true,
                        _=>{}
                    }
                }
            }
        }
        if let Some((id,_,_))=best {if !ambiguous{return Some(make_handle(TAG_METHOD,id));}
            self.set_pending("java/lang/NoSuchMethodError","ambiguous reflected overload");return None;}
        self.resolve_method(class, name, signature, "reflectMethod")
    }

    // Unity's ReflectionHelper accepts binary class names and argument
    // subclasses. The result is the original declaring MethodID, so cached
    // calls still use its exact descriptor and native ABI shape.
    fn reflection_score(&self,declared:&str,requested:&str)->Option<usize> {
        fn types(signature:&str)->Option<Vec<&str>>{
            let args=signature.strip_prefix('(')?;let (args,result)=args.split_once(')')?;
            let mut values=Vec::new();let mut offset=0;
            while offset<args.len(){let start=offset;while args.as_bytes().get(offset)==Some(&b'['){offset+=1;}
                match *args.as_bytes().get(offset)? {b'L'=>{offset+=args[offset..].find(';')?+1;},b'Z'|b'B'|b'C'|b'S'|b'I'|b'J'|b'F'|b'D'=>offset+=1,_=>return None}
                values.push(&args[start..offset]);
            }if result.is_empty(){return None;}values.push(result);Some(values)
        }
        fn reference(name:&str)->Option<&str>{if name.starts_with('L'){name.strip_prefix('L')?.strip_suffix(';')}else if name.starts_with('['){Some(name)}else{None}}
        let declared=types(declared)?;let requested=types(requested)?;if declared.len()!=requested.len(){return None;}
        let mut score=0;
        for (i,(expected,actual)) in declared.iter().zip(&requested).enumerate(){
            if expected==actual{score+=4;continue;}
            let (expected,actual)=(reference(expected)?,reference(actual)?);
            // Unity uses Object as the signature hint for a null argument and
            // for a generic object return. It is a lowest-score wildcard.
            if actual=="java/lang/Object" {continue;}
            let ids={let classes=self.classes.lock().unwrap();(classes.by_name.get(expected).copied(),classes.by_name.get(actual).copied())};
            let (Some(expected),Some(actual))=ids else{return None;};
            let compatible=if i==declared.len()-1{self.is_subclass(expected,actual)}else{self.is_subclass(actual,expected)};
            if !compatible{return None;}score+=1;
        }Some(score)
    }

    fn resolve_declared_method(&self, declaring:u64, name:&str, sig:&str, entry:&str)->Option<u64>{
        if let Some(id)=self.methods.lock().unwrap().find(declaring,name,sig){return Some(id);}
        let value=self.catalog_query(&self.class_name(declaring),name,sig)?;
        let mut methods=self.methods.lock().unwrap();
        if let Some(id)=methods.find(declaring,name,sig){return Some(id);}
        let Some(id)=self.next_serial() else{drop(methods);self.serial_exhausted(entry);return None;};
        methods.insert(id,Method{class:declaring,name:name.into(),sig:sig.into(),value,next_collision:0});Some(id)
    }

    fn resolve_method_for_class(&self, class_id: u64, name: &str, sig: &str, entry: &str) -> Option<*mut c_void> {
        if let Some(id) = self.methods.lock().unwrap().find_cached(class_id, name, sig) {
            return Some(make_handle(TAG_METHOD, id));
        }
        // Cold resolution walks class implementations before interface defaults.
        // Warm inherited calls cache only the declaring method ID.
        let declarations = self.method_declarations(class_id, name);
        for declaring in declarations {
            let id=self.resolve_declared_method(declaring,name,sig,entry);
            if let Some(id) = id {
                self.methods.lock().unwrap().cache_alias(class_id, id);
                return Some(make_handle(TAG_METHOD, id));
            }
        }
        self.log(&format!("{entry}: no catalog entry for {}/{name}{sig} (unknown method or wrong signature) — pending NoSuchMethodError", self.class_name(class_id)));
        self.set_pending("java/lang/NoSuchMethodError", "no catalog entry");
        None
    }

    fn class_name(&self, id: u64) -> Arc<str> {
        self.classes
            .lock()
            .unwrap()
            .by_id.get(&id)
            .cloned()
            .unwrap_or_else(|| Arc::from("<unregistered>"))
    }

    /// Call dispatch: blocked-while-pending, shared validity checks, then the
    /// KIND-CHECKED answer — calling through the wrong accessor fails
    /// explicitly (zero + pending), never a silent default. `wanted` names
    /// the calling variant for logs.
    #[cfg(test)]
    fn call_checked(&self, receiver: *mut c_void, method: *mut c_void,
        wanted: abi::MethodKind, entry: &str, instance: bool) -> Option<MethodValue> {
        self.call_checked_dispatch(receiver, method, wanted, entry, Dispatch::from(instance))
    }

    fn call_checked_dispatch(&self, receiver: *mut c_void, method: *mut c_void,
        wanted: abi::MethodKind, entry: &str, dispatch: Dispatch) -> Option<MethodValue> {
        if self.blocked_by_pending(entry) {
            return None;
        }
        let (tag, serial) = handle_parts(method);
        if tag != TAG_METHOD {
            self.log(&format!("{entry}: not a method ID"));
            self.set_pending("java/lang/NoSuchMethodError", "not a method ID");
            return None;
        }
        let method = {
            let methods = self.methods.lock().unwrap();
            methods.by_id.get(&serial).map(|method| (method.class, method.value))
        };
        let Some((method_class, mut value)) = method else {
            self.log(&format!("{entry}: unknown method ID"));
            self.set_pending("java/lang/NoSuchMethodError", "unknown method ID");
            return None;
        };
        if self.trace_calls.load(Ordering::Relaxed){
            let (name,sig)={let methods=self.methods.lock().unwrap();let method=&methods.by_id[&serial];(method.name.clone(),method.sig.clone())};
            self.log(&format!("JNI_CALL {entry} {}/{name}{sig} id={serial} value={value:?}",self.class_name(method_class)));
        }
        // The class argument must reference the method's class.
        let class_id = if dispatch != Dispatch::Static { self.object_class(receiver, entry)? }
            else { self.resolve_class(receiver, entry)? };
        if class_id != method_class && !self.is_subclass(class_id, method_class) {
            let message = {
                let methods = self.methods.lock().unwrap();
                let method = &methods.by_id[&serial];
                format!(
                    "{entry}: method {}/{} belongs to {} but the caller passed {}",
                    method.name, method.sig, self.class_name(method_class), self.class_name(class_id)
                )
            };
            self.log(&message);
            self.set_pending("java/lang/NoSuchMethodError", "method belongs to another class");
            return None;
        }
        if let Dispatch::Nonvirtual(class) = dispatch {
            let supplied = self.resolve_class(class, entry)?;
            if !self.is_subclass(class_id, supplied) {
                self.set_pending("java/lang/IllegalArgumentException", "nonvirtual class is not a receiver supertype");
                return None;
            }
            let (cached, cold) = {
                let methods = self.methods.lock().unwrap();
                let original = &methods.by_id[&serial];
                match methods.find_cached(supplied, &original.name, &original.sig) {
                    Some(id) => (Some(id), None),
                    None => (None, Some((original.name.clone(), original.sig.clone()))),
                }
            };
            // GetMethodID on the supplied class must yield this exact method
            // identity. A child's override cannot be passed with its parent.
            // Resolution/providers run outside locks; never dispatch through
            // the actual receiver's override in this mode.
            let selected = match (cached, cold) {
                (Some(id), _) => id,
                (_, Some((name, signature))) => handle_parts(self.resolve_method_for_class(supplied, &name, &signature, entry)?).1,
                _ => unreachable!("cached identity or cold descriptor"),
            };
            if selected != serial {
                self.set_pending("java/lang/NoSuchMethodError", "nonvirtual method was not obtained from the supplied class");
                return None;
            }
            value = self.methods.lock().unwrap().by_id[&serial].value;
        }
        if dispatch == Dispatch::Virtual && method_class != class_id {
            let (cached, cold) = {
                let methods = self.methods.lock().unwrap();
                let original = &methods.by_id[&serial];
                match methods.find_cached(class_id, &original.name, &original.sig) {
                    Some(id) => (Some(methods.by_id[&id].value), None),
                    None => (None, Some((original.name.clone(), original.sig.clone()))),
                }
            };
            if let Some(resolved) = cached { value = resolved; }
            else if let Some((name, sig)) = cold {
                let resolved = self.resolve_method_for_class(class_id, &name, &sig, entry)?;
                value = self.methods.lock().unwrap().by_id[&handle_parts(resolved).1].value;
            }
        }
        if matches!(value,MethodValue::NativeOther(_) | MethodValue::NativeUnbound) {
            self.set_pending("java/lang/UnsatisfiedLinkError", "native binding absent or call signature unsupported");
            return None;
        }
        // Type mismatch: calling through the wrong accessor kind fails
        // explicitly (zero + pending), never a silent default.
        let matches_kind = match (wanted, value) {
            (kind, MethodValue::NativeScalar(_, returned, _)) => kind == returned.kind(),
            (abi::MethodKind::Int, MethodValue::Int(_))
            | (abi::MethodKind::Float,MethodValue::NativeFloatNoArgs(_) | MethodValue::NativeFloatInt(_) | MethodValue::NativeFloatTwoInts(_) | MethodValue::NativeFloatObjectFloat(_))
            | (abi::MethodKind::Long,MethodValue::NativeLongNoArgs(_) | MethodValue::NativeLongObjectLong(_))
            | (abi::MethodKind::Int,MethodValue::NativeInt(_,_))
            | (abi::MethodKind::Bool,MethodValue::NativeBoolObjectLong(_) | MethodValue::NativeBoolObjectBoolean(_))
            | (abi::MethodKind::Bool, MethodValue::Bool(_))
            | (abi::MethodKind::Bool, MethodValue::NativeBoolString(_))
            | (abi::MethodKind::Bool, MethodValue::NativeBoolNoArgs(_) | MethodValue::NativeBoolInt(_)) => true,
            (abi::MethodKind::Void, MethodValue::NativeVoid(_, _)) => true,
            (abi::MethodKind::Object, MethodValue::NativeObject(_, _)) => true,
            _ => false,
        };
        if !matches_kind {
            let message = {
                let methods = self.methods.lock().unwrap();
                let method = &methods.by_id[&serial];
                format!(
                    "{entry}: TYPE MISMATCH — method {}/{} resolves to {value:?} but the \
                     caller used the {wanted:?} accessor; returning the zero default \
                     with a pending exception", method.name, method.sig
                )
            };
            self.log(&message);
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "wrong call accessor for the method's value kind",
            );
            return None;
        }
        Some(value)
    }

    // -- strings

    fn new_string(&self, bytes: &[u8]) -> *mut c_void {
        if self.blocked_by_pending("NewStringUTF") { return std::ptr::null_mut(); }
        let mut terminated = Vec::new();
        if terminated.try_reserve_exact(bytes.len().saturating_add(1)).is_err() {
            self.set_pending("java/lang/OutOfMemoryError", "JNI string allocation failed");
            return std::ptr::null_mut();
        }
        terminated.extend_from_slice(bytes);
        terminated.push(0);
        self.new_string_owned(terminated.into_boxed_slice())
    }

    fn new_string_owned(&self, terminated: Box<[u8]>) -> *mut c_void {
        if self.blocked_by_pending("NewStringUTF") { return std::ptr::null_mut(); }
        let Some(id) = self.next_serial() else {
            self.serial_exhausted("NewStringUTF");
            return std::ptr::null_mut();
        };
        let handle = self.new_ref(RefTarget::Str(id), false);
        if handle.is_null() { return handle; }
        self.sweep_probe();
        self.strings.lock().unwrap().insert(id, strings::StringData::new(terminated));
        handle
    }

    #[cfg(test)]
    fn string_bytes(&self, handle: *mut c_void, entry: &str) -> Option<Box<[u8]>> {
        match self.resolve(handle) {
            Some(RefTarget::Str(id)) => self.strings.lock().unwrap().get(&id).map(|bytes| bytes[..bytes.len() - 1].into()),
            other => {
                self.log(&format!("{entry}: not a live string reference (got {other:?})"));
                self.set_pending("java/lang/StringIndexOutOfBoundsException", "not a live string");
                None
            }
        }
    }

    fn string_length(&self, handle: *mut c_void) -> Option<usize> {
        match self.resolve(handle) {
            Some(RefTarget::Str(id)) => self.strings.lock().unwrap().get(&id).map(|bytes| bytes.len() - 1),
            other => {
                self.log(&format!("GetStringUTFLength: not a live string reference (got {other:?})"));
                self.set_pending("java/lang/StringIndexOutOfBoundsException", "not a live string");
                None
            }
        }
    }
}

// ------------------------------------------------------- global + lifecycle

static SESSION: OnceLock<Session> = OnceLock::new();

/// Install the process session (first write wins; a later bring-up in the
/// same process reuses it — the caller reconciles with the registry's
/// shared-VM slot to stay idempotent). Attach the initializing thread so
/// guest JNI_OnLoad can query its env before creating any JNI references.
pub fn init_with_host(host: HostApiV0) {
    let session = SESSION.get_or_init(Session::new);
    session.install_host(host);
    session.attach_current_thread();
    session.builtins.get_or_init(|| install_builtin_types(session));
}

fn install_builtin_types(session: &Session) {
    // Establish Object before any other type receives its default parent.
    let object = session.find_class("java/lang/Object").unwrap();
    session.delete_ref(object, false, "bootstrap root type");
    // Plugin metadata enters the same class authority through a normal JNI
    // native method. This internal bootstrap class owns no parallel state.
    let class = session.find_class("org/rudroid/runtime/TypeRegistry").unwrap();
    let methods = [JniNativeMethod {
        name: c"setSuperclass".as_ptr(),
        signature: c"(Ljava/lang/Class;Ljava/lang/Class;)V".as_ptr(),
        fn_ptr: shims::set_superclass as *mut c_void,
    }, JniNativeMethod { name:c"defineField".as_ptr(), signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)V".as_ptr(), fn_ptr:shims::define_field as *mut c_void },
    JniNativeMethod { name:c"proxyClass".as_ptr(), signature:c"(Ljava/lang/Class;[Ljava/lang/Class;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::proxy_class as *mut c_void },JniNativeMethod{name:c"fieldSignature".as_ptr(),signature:c"(Ljava/lang/reflect/Field;)Ljava/lang/String;".as_ptr(),fn_ptr:shims::field_signature as *mut c_void},
    JniNativeMethod{name:c"reflectField".as_ptr(),signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;".as_ptr(),fn_ptr:shims::lookup_reflected_field as *mut c_void},
    JniNativeMethod{name:c"reflectMethod".as_ptr(),signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;".as_ptr(),fn_ptr:shims::lookup_reflected_method as *mut c_void}];
    // SAFETY: static names/descriptors and shims matching each descriptor.
    unsafe { session.register_natives(class, methods.as_ptr(), methods.len() as i32); }
    session.delete_ref(class, false, "bootstrap types");
    let boolean=session.find_class("java/lang/Boolean").unwrap();
    session.define_field(session.resolve_class(boolean,"bootstrap Boolean").unwrap(),"value","Z",false);
    session.delete_ref(boolean,false,"bootstrap Boolean");
    let long=session.find_class("java/lang/Long").unwrap();session.define_field(session.resolve_class(long,"bootstrap Long").unwrap(),"value","J",false);session.delete_ref(long,false,"bootstrap Long");
    let integer=session.find_class("java/lang/Integer").unwrap();session.define_field(session.resolve_class(integer,"bootstrap Integer").unwrap(),"value","I",false);session.delete_ref(integer,false,"bootstrap Integer");
    for (name, bindings) in [
        ("java/nio/ByteBuffer",vec![JniNativeMethod{name:c"allocateDirect".as_ptr(),signature:c"(I)Ljava/nio/ByteBuffer;".as_ptr(),fn_ptr:shims::allocate_direct as *mut c_void},JniNativeMethod{name:c"_rudroidZeroOwned".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::zero_owned as *mut c_void}]),
        ("java/lang/System",vec![JniNativeMethod{name:c"identityHashCode".as_ptr(),signature:c"(Ljava/lang/Object;)I".as_ptr(),fn_ptr:shims::identity_hash as *mut c_void}]),
        ("java/lang/Long",vec![JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(J)V".as_ptr(),fn_ptr:shims::long_init as *mut c_void},JniNativeMethod{name:c"longValue".as_ptr(),signature:c"()J".as_ptr(),fn_ptr:shims::long_value as *mut c_void}]),
        ("java/lang/Integer",vec![JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(I)V".as_ptr(),fn_ptr:shims::integer_init as *mut c_void},JniNativeMethod{name:c"intValue".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::integer_value as *mut c_void},JniNativeMethod{name:c"parseInt".as_ptr(),signature:c"(Ljava/lang/String;)I".as_ptr(),fn_ptr:shims::integer_parse as *mut c_void}]),
        ("java/lang/Boolean",vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Z)V".as_ptr(),fn_ptr:shims::boolean_init as *mut c_void},
            JniNativeMethod{name:c"booleanValue".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:shims::boolean_value as *mut c_void},
        ]),
        ("java/lang/Class", vec![
            JniNativeMethod { name:c"isArray".as_ptr(), signature:c"()Z".as_ptr(), fn_ptr:shims::class_is_array as *mut c_void },
            JniNativeMethod { name:c"getClassLoader".as_ptr(), signature:c"()Ljava/lang/ClassLoader;".as_ptr(), fn_ptr:shims::class_loader as *mut c_void },
            JniNativeMethod { name:c"getName".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::class_name_string as *mut c_void },
            JniNativeMethod { name:c"forName".as_ptr(), signature:c"(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::class_for_name as *mut c_void },
        ]),
        ("java/lang/reflect/Method", vec![
            JniNativeMethod { name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::reflected_method_name as *mut c_void },
            JniNativeMethod { name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::reflected_declaring_class as *mut c_void },
        ]),
        ("java/lang/reflect/Constructor",vec![
            JniNativeMethod{name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::reflected_declaring_name as *mut c_void},
            JniNativeMethod{name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::reflected_declaring_class as *mut c_void},
        ]),
        ("java/lang/reflect/Field",vec![JniNativeMethod{name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::field_name as *mut c_void},JniNativeMethod{name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::field_declaring_class as *mut c_void}]),
        ("java/util/Scanner", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Ljava/io/InputStream;Ljava/lang/String;)V".as_ptr(),fn_ptr:streams::scanner::init as *mut c_void},
            JniNativeMethod{name:c"useDelimiter".as_ptr(),signature:c"(Ljava/lang/String;)Ljava/util/Scanner;".as_ptr(),fn_ptr:streams::scanner::delimiter as *mut c_void},
            JniNativeMethod{name:c"next".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:streams::scanner::next as *mut c_void},
            JniNativeMethod{name:c"hasNext".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:streams::scanner::has_next as *mut c_void},
            JniNativeMethod{name:c"close".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:streams::scanner::close as *mut c_void},
        ]),
        ("java/io/FileInputStream", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Ljava/lang/String;)V".as_ptr(),fn_ptr:shims::stream_init as *mut c_void},
            JniNativeMethod{name:c"read".as_ptr(),signature:c"([B)I".as_ptr(),fn_ptr:shims::stream_read as *mut c_void},
            JniNativeMethod{name:c"read".as_ptr(),signature:c"([BII)I".as_ptr(),fn_ptr:shims::stream_read_range as *mut c_void},
            JniNativeMethod{name:c"available".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::stream_available as *mut c_void},
            JniNativeMethod{name:c"close".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::stream_close as *mut c_void},
        ]),
        ("java/lang/ClassLoader", vec![
            JniNativeMethod { name:c"loadClass".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::load_class as *mut c_void },
            JniNativeMethod { name:c"findClass".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::load_class as *mut c_void },
        ]),
        ("java/lang/String", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::immutable_string_constructor as *mut c_void},
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"([BLjava/lang/String;)V".as_ptr(),fn_ptr:shims::immutable_string_constructor as *mut c_void},
            JniNativeMethod{name:c"equals".as_ptr(),signature:c"(Ljava/lang/Object;)Z".as_ptr(),fn_ptr:shims::string_equals as *mut c_void},
            JniNativeMethod{name:c"isEmpty".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:shims::string_empty as *mut c_void},
            JniNativeMethod{name:c"length".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::get_string_length as *mut c_void},
            JniNativeMethod{name:c"getBytes".as_ptr(),signature:c"(Ljava/lang/String;)[B".as_ptr(),fn_ptr:shims::string_bytes_utf8 as *mut c_void},
        ]),
        ("java/lang/Object", vec![
            JniNativeMethod{name:c"equals".as_ptr(),signature:c"(Ljava/lang/Object;)Z".as_ptr(),fn_ptr:shims::object_equals as *mut c_void},
            JniNativeMethod { name:c"toString".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::object_to_string as *mut c_void },
            JniNativeMethod { name:c"getClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::get_object_class as *mut c_void },
        ]),
        ("java/lang/StringBuilder", vec![
            JniNativeMethod { name:c"<init>".as_ptr(), signature:c"()V".as_ptr(), fn_ptr:shims::builder_init as *mut c_void },
            JniNativeMethod { name:c"append".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/StringBuilder;".as_ptr(), fn_ptr:shims::builder_append as *mut c_void },
            JniNativeMethod { name:c"toString".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::builder_string as *mut c_void },
        ]),
    ] {
        let class = session.find_class(name).unwrap();
        // SAFETY: as the TypeRegistry bindings above.
        unsafe { session.register_natives(class, bindings.as_ptr(), bindings.len() as c_int); }
        session.delete_ref(class, false, "bootstrap types");
    }
    let stream=session.find_class("java/io/InputStream").unwrap();let file=session.find_class("java/io/FileInputStream").unwrap();session.set_superclass(file,stream);session.delete_ref(file,false,"bootstrap stream");session.delete_ref(stream,false,"bootstrap stream");
    let mut classes = session.classes.lock().unwrap();
    let object = classes.by_name["java/lang/Object"];
    for name in ["java/lang/Class", "java/lang/ClassLoader", "java/lang/StringBuilder", "org/rudroid/runtime/TypeRegistry"] {
        let child = classes.by_name[name];
        classes.parents.insert(child, object);
    }
}

/// The process session, if the `android_jni_vm` phase created it.
pub fn session() -> Option<&'static Session> {
    SESSION.get()
}

/// Log + pending + zero default for every unimplemented table entry.
pub fn report_unimplemented_slot(name: &str, env: *const c_void) -> usize {
    let message = format!(
        "JNI table entry `{name}` is not implemented by the minimal session \
         (ticket 52): returning the zero default with a pending exception"
    );
    match session() {
        Some(session) => {
            session.log(&message);
            if !env.is_null() {
                session.set_pending("java/lang/InternalError", &message);
            }
        }
        None => eprintln!("[android_jni_vm] {message}"),
    }
    0
}

// ------------------------------------------------------------------ shims

/// The `extern "C"` table entries. They resolve the global session and
/// delegate; without a session they fail explicitly (JNI_ERR on the VM
/// table, zero results with a stderr diagnostic on the env table) — the
/// table exists from first use, the session semantics only after
/// `android_jni_vm` early-init ran.
pub mod shims {
    pub(crate) use super::scalar_calls::*;
    pub(crate) use super::nonvirtual_calls::*;
    pub(crate) use super::fields::shims::*;
    pub(crate) use super::strings::shims::*;
    pub(crate) use super::primitive_arrays::{new_boolean_array,get_boolean_array_region,set_boolean_array_region,get_boolean_array_elements,release_boolean_array_elements,new_byte_array,get_byte_array_region,set_byte_array_region,get_byte_array_elements,release_byte_array_elements,new_char_array,get_char_array_region,set_char_array_region,get_char_array_elements,release_char_array_elements,new_short_array,get_short_array_region,set_short_array_region,get_short_array_elements,release_short_array_elements,new_int_array,get_int_array_region,set_int_array_region,get_int_array_elements,release_int_array_elements,new_long_array,get_long_array_region,set_long_array_region,get_long_array_elements,release_long_array_elements,new_float_array,get_float_array_region,set_float_array_region,get_float_array_elements,release_float_array_elements,new_double_array,get_double_array_region,set_double_array_region,get_double_array_elements,release_double_array_elements,get_primitive_array_critical,release_primitive_array_critical};
    
    pub use super::direct_buffers::shims::*;
    pub(crate) use super::streams::shims::*;
    use super::*;

    pub extern "C" fn new_object_array(_env: *mut c_void, size: i32, class: *mut c_void, initial: *mut c_void) -> *mut c_void {
        session().map(|s| s.new_object_array(size, class, initial)).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn get_array_length(_env: *mut c_void, array: *mut c_void) -> i32 {
        session().map(|s| s.array_length(array)).unwrap_or(0)
    }

    pub extern "C" fn get_object_array_element(_env: *mut c_void, array: *mut c_void, index: i32) -> *mut c_void {
        session().map(|s| s.object_array_element(array, index)).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn set_object_array_element(_env: *mut c_void, array: *mut c_void, index: i32, value: *mut c_void) {
        if let Some(s) = session() { s.set_object_array_element(array, index, value); }
    }

    pub unsafe extern "C" fn new_object_a(env: *mut c_void, class: *mut c_void, method: *mut c_void, args: *const JValue) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("NewObjectA") { return std::ptr::null_mut(); }
        let Some(class_id) = s.resolve_class(class, "NewObjectA") else { return std::ptr::null_mut(); };
        let (tag, id) = handle_parts(method);
        let valid = tag == TAG_METHOD && s.methods.lock().unwrap().by_id.get(&id)
            .is_some_and(|m| m.name.as_ref() == "<init>" && m.class == class_id && m.sig.ends_with(")V"));
        if !valid {
            s.set_pending("java/lang/NoSuchMethodError", "not this class's constructor");
            return std::ptr::null_mut();
        }
        if s.class_name(class_id).as_ref()=="java/lang/String" && s.methods.lock().unwrap().by_id[&id].sig.as_ref()=="()V" {
            return s.new_string_units(&[]);
        }
        if s.class_name(class_id).as_ref()=="java/lang/String" && s.methods.lock().unwrap().by_id[&id].sig.as_ref()=="([BLjava/lang/String;)V" {
            if args.is_null(){s.set_pending("java/lang/IllegalArgumentException","missing String constructor arguments");return std::ptr::null_mut();}
            let Some(utf8)=s.with_string_bytes((*args.add(1)).l,|bytes|matches!(bytes,b"UTF-8"|b"UTF8")) else{return std::ptr::null_mut();};
            if !utf8{s.set_pending("java/io/UnsupportedEncodingException","String byte constructor supports UTF-8");return std::ptr::null_mut();}
            let Some(RefTarget::Array(array))=s.resolve((*args).l) else{s.set_pending("java/lang/NullPointerException","missing String byte array");return std::ptr::null_mut();};
            let units={let arrays=s.arrays.lock().unwrap();let Some(bytes)=arrays.get(&array).and_then(|a|a.primitive.as_ref().and_then(primitive_arrays::Values::bytes)) else{drop(arrays);s.set_pending("java/lang/IllegalArgumentException","String requires byte[]");return std::ptr::null_mut();};
                String::from_utf8_lossy(bytes::unsigned(bytes)).encode_utf16().collect::<Vec<_>>()};
            return s.new_string_units(&units);
        }
        let object = s.alloc_object(class);
        if object.is_null() { return object; }
        call_void_method_a(env, object, method, args);
        if s.with_thread(|t| t.pending.is_some()) {
            s.delete_ref(object, false, "NewObjectA failed constructor");
            std::ptr::null_mut()
        } else { object }
    }

    pub extern "C" fn immutable_string_constructor(_env:*mut c_void,_object:*mut c_void,_bytes:*mut c_void,_encoding:*mut c_void){if let Some(s)=session(){s.set_pending("java/lang/IllegalArgumentException","immutable String constructor requires NewObject");}}
    fn with_boxed_field<R>(env: *mut c_void, object: *mut c_void, signature: &str, access: impl FnOnce(*mut c_void) -> R) -> R {
        let class = get_object_class(env, object);
        let field = session().map(|s| s.resolve_field(class, "value", signature, false)).unwrap_or(std::ptr::null_mut());
        let result = access(field);
        delete_local_ref(env, class);
        result
    }

    pub extern "C" fn long_init(env: *mut c_void, object: *mut c_void, value: i64) {
        with_boxed_field(env, object, "J", |field| set_long_field(env, object, field, value));
    }
    pub extern "C" fn integer_init(env: *mut c_void, object: *mut c_void, value: i32) {
        with_boxed_field(env, object, "I", |field| set_int_field(env, object, field, value));
    }
    pub extern "C" fn integer_parse(_env: *mut c_void, _class: *mut c_void, text: *mut c_void) -> i32 {
        let Some(s)=session() else { return 0; };
        if !text.is_null() {
            if let Some(result)=s.with_string_units(text,crate::integers::parse) {
                if let Some(value)=result { return value; }
            } else { return 0; }
        }
        s.set_pending("java/lang/NumberFormatException","invalid or out-of-range decimal integer");
        0
    }
    pub extern "C" fn integer_value(env: *mut c_void, object: *mut c_void) -> i32 {
        with_boxed_field(env, object, "I", |field| get_int_field(env, object, field))
    }
    pub extern "C" fn long_value(env: *mut c_void, object: *mut c_void) -> i64 {
        with_boxed_field(env, object, "J", |field| get_long_field(env, object, field))
    }
    pub extern "C" fn boolean_init(env: *mut c_void, object: *mut c_void, value: u8) {
        with_boxed_field(env, object, "Z", |field| set_boolean_field(env, object, field, value));
    }
    pub extern "C" fn boolean_value(env: *mut c_void, object: *mut c_void) -> u8 {
        with_boxed_field(env, object, "Z", |field| get_boolean_field(env, object, field))
    }
    pub extern "C" fn reflected_declaring_name(env:*mut c_void,object:*mut c_void)->*mut c_void{let class=reflected_declaring_class(env,object);let name=class_name_string(env,class);delete_local_ref(env,class);name}

    pub extern "C" fn builder_init(_env: *mut c_void, object: *mut c_void) {
        let Some(s) = session() else { return; };
        if let Some(RefTarget::Object(id)) = s.resolve(object) { s.objects.lock().unwrap().get_mut(&id).unwrap().builder = Some(Vec::new()); }
    }

    pub unsafe extern "C" fn builder_append(env: *mut c_void, object: *mut c_void, string: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let Some(RefTarget::Object(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
        let chars = if string.is_null() { std::ptr::null() } else {
            let chars = get_string_utf_chars(env, string, std::ptr::null_mut());
            if chars.is_null() { return std::ptr::null_mut(); }
            chars
        };
        let bytes = if chars.is_null() { b"null".as_slice() } else { CStr::from_ptr(chars).to_bytes() };
        let result = {
            let mut objects = s.objects.lock().unwrap();
            match objects.get_mut(&id).and_then(|o| o.builder.as_mut()) {
                None => Err(("java/lang/IllegalStateException", "uninitialized StringBuilder")),
                Some(buffer) => {
                    if buffer.try_reserve(bytes.len()).is_err() { Err(("java/lang/OutOfMemoryError", "StringBuilder growth failed")) }
                    else { buffer.extend_from_slice(bytes); Ok(()) }
                }
            }
        };
        if !chars.is_null() { release_string_utf_chars(env, string, chars); }
        if let Err((class, message)) = result {
            s.set_pending(class, message);
            return std::ptr::null_mut();
        }
        s.new_ref(RefTarget::Object(id), false)
    }

    pub extern "C" fn builder_string(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let Some(RefTarget::Object(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
        let result = {
            let objects = s.objects.lock().unwrap();
            match objects.get(&id).and_then(|o| o.builder.as_ref()) {
                None => Err(("java/lang/IllegalStateException", "uninitialized StringBuilder")),
                Some(text) => {
                    let mut bytes = Vec::new();
                    if bytes.try_reserve_exact(text.len().saturating_add(1)).is_err() {
                        Err(("java/lang/OutOfMemoryError", "StringBuilder snapshot failed"))
                    } else {
                        bytes.extend_from_slice(text); bytes.push(0); Ok(bytes.into_boxed_slice())
                    }
                }
            }
        };
        match result {
            Ok(bytes) => s.new_string_owned(bytes),
            Err((class, message)) => { s.set_pending(class, message); std::ptr::null_mut() }
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn rudroid_jni_argument_kinds(method: *mut c_void, out: *mut u8, capacity: usize) -> c_int {
        let Some(s) = session() else { return -1; };
        if s.blocked_by_pending("JNI variadic arguments") { return -1; }
        let (tag, id) = handle_parts(method);
        if tag != TAG_METHOD || out.is_null() {
            s.set_pending("java/lang/NoSuchMethodError", "invalid JNI variadic method ID");
            return -1;
        }
        let methods = s.methods.lock().unwrap();
        let Some(method) = methods.by_id.get(&id) else {
            drop(methods);
            s.set_pending("java/lang/NoSuchMethodError", "unknown JNI variadic method ID");
            return -1;
        };
        let bytes = method.sig.as_bytes();
        let mut cursor = 1;
        let mut count = 0;
        let result = (|| {
            if bytes.first() != Some(&b'(') { return None; }
            while *bytes.get(cursor)? != b')' {
                if count >= capacity { return None; }
                let mut kind = *bytes.get(cursor)?;
                while *bytes.get(cursor)? == b'[' { kind = b'L'; cursor += 1; }
                match *bytes.get(cursor)? {
                    b'L' => {
                        kind = b'L';
                        cursor += 1;
                        let begin = cursor;
                        while *bytes.get(cursor)? != b';' { cursor += 1; }
                        if cursor == begin { return None; }
                    }
                    b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D' => {}
                    _ => return None,
                }
                cursor += 1;
                // Caller provides capacity writable bytes; the method descriptor
                // is borrowed under the method lock, never copied or reallocated.
                *out.add(count) = kind;
                count += 1;
            }
            Some(count as c_int)
        })();
        drop(methods);
        result.unwrap_or_else(|| {
            s.set_pending("java/lang/IllegalArgumentException", "invalid or oversized JNI argument descriptor");
            -1
        })
    }

    pub extern "C" fn class_loader(_env: *mut c_void, _class: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let handle = *s.class_loader.get_or_init(|| {
            let class = s.find_class("java/lang/ClassLoader").unwrap();
            let local = s.alloc_object(class);
            let global = s.new_ref(s.resolve(local).unwrap(), true);
            s.delete_ref(local, false, "ClassLoader");
            s.delete_ref(class, false, "ClassLoader");
            global as usize
        }) as *mut c_void;
        s.new_ref(s.resolve(handle).unwrap(), false)
    }

    pub extern "C" fn class_name_string(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let Some(id) = s.resolve_class(class, "Class.getName") else { return std::ptr::null_mut(); };
        s.new_string(s.class_name(id).replace('/', ".").as_bytes())
    }
    pub extern "C" fn class_is_array(_env:*mut c_void,class:*mut c_void)->u8{
        let Some(s)=session() else{return 0;};let Some(id)=s.resolve_class(class,"Class.isArray") else{return 0;};u8::from(s.class_name(id).starts_with('['))
    }

    pub extern "C" fn load_class(env: *mut c_void, _loader: *mut c_void, name: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let _ = env;
        let Some(class_name) = s.with_string_bytes(name, |bytes| String::from_utf8_lossy(bytes).replace('.', "/")) else {
            return std::ptr::null_mut();
        };
        s.find_class(&class_name).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn class_for_name(env: *mut c_void, class: *mut c_void, name: *mut c_void,
        _initialize: u8, _loader: *mut c_void) -> *mut c_void {
        load_class(env, class, name)
    }

    pub extern "C" fn string_equals(_env:*mut c_void,string:*mut c_void,other:*mut c_void)->u8 {
        let Some(s)=session() else{return 0;};let(Some(RefTarget::Str(a)),Some(RefTarget::Str(b)))=(s.resolve(string),s.resolve(other))else{return 0;};
        let strings=s.strings.lock().unwrap();u8::from(strings.get(&a).zip(strings.get(&b)).is_some_and(|(a,b)|**a==**b))
    }
    pub extern "C" fn string_empty(_env:*mut c_void,string:*mut c_void)->u8 {session().and_then(|s|s.string_length(string)).map(|length|u8::from(length==0)).unwrap_or(0)}
    pub extern "C" fn object_equals(env:*mut c_void,object:*mut c_void,other:*mut c_void)->u8 {is_same_object(env,object,other)}

    pub extern "C" fn object_to_string(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        match s.resolve(object) {
            Some(RefTarget::Str(id)) => s.new_ref(RefTarget::Str(id), false),
            Some(RefTarget::Throwable(id)) => {
                let text = s.throwables.lock().unwrap().get(&id).map(|e| format!("{}: {}", e.class, e.message));
                text.map(|t| s.new_string(t.as_bytes())).unwrap_or(std::ptr::null_mut())
            }
            Some(RefTarget::Class(id)) => s.new_string(s.class_name(id).as_bytes()),
            Some(RefTarget::ReflectedMethod(id)) => { let name=s.methods.lock().unwrap().by_id[&id].name.clone(); s.new_string(name.as_bytes()) },
            Some(RefTarget::ReflectedField(_))=>field_name(_env,object),
            Some(RefTarget::Object(id)) => {
                let class = s.objects.lock().unwrap().get(&id).unwrap().class;
                s.new_string(format!("{}@{id:x}", s.class_name(class)).as_bytes())
            }
            Some(RefTarget::Array(id)) => s.new_string(format!("array@{id:x}").as_bytes()),
            None => std::ptr::null_mut(),
        }
    }

    pub unsafe extern "C" fn call_object_method_a(env: *mut c_void, object: *mut c_void,
        method: *mut c_void, args: *const JValue) -> *mut c_void {
        call_object_a(env, object, method, args, Dispatch::Virtual)
    }

    pub unsafe extern "C" fn call_static_object_method_a(env: *mut c_void, class: *mut c_void,
        method: *mut c_void, args: *const JValue) -> *mut c_void {
        call_object_a(env, class, method, args, Dispatch::Static)
    }

    pub(super) unsafe fn call_object_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void,
        args: *const JValue, dispatch: Dispatch) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        match s.call_checked_dispatch(receiver, method, abi::MethodKind::Object, "CallObjectMethodA", dispatch) {
            Some(MethodValue::NativeObject(address, ObjectShape::NoArgs)) => {
                native_call!(address, fn(*mut c_void, *mut c_void) -> *mut c_void; env, receiver)
            }
            Some(MethodValue::NativeObject(address, ObjectShape::Int)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, i32) -> *mut c_void; env, receiver, (*args).i)
            }
            Some(MethodValue::NativeObject(address,ObjectShape::ThreeInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32,i32) ->*mut c_void; env,receiver,(*args).i,(*args.add(1)).i,(*args.add(2)).i)}
            Some(MethodValue::NativeObject(address,ObjectShape::TwoInts)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,i32,i32)->*mut c_void;env,receiver,(*args).i,(*args.add(1)).i)}
            Some(MethodValue::NativeObject(address,ObjectShape::ClassStringsBoolean)) if !args.is_null()=>{
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void,u8) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).z)
            }
            Some(MethodValue::NativeObject(address,ObjectShape::ObjectLongObject)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i64,*mut c_void) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).j,(*args.add(2)).l)}
            Some(MethodValue::NativeObject(address,ObjectShape::LongBoolean)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i64,u8) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).z)}
            Some(MethodValue::NativeObject(address,ObjectShape::LongTwoObjects)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).l,(*args.add(2)).l)}
            Some(MethodValue::NativeObject(address, ObjectShape::String)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void; env, receiver, (*args).l)
            }
            Some(MethodValue::NativeObject(address, ObjectShape::StringBoolObject)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, u8, *mut c_void) -> *mut c_void; env, receiver, (*args).l, (*args.add(1)).z, (*args.add(2)).l)
            }
            Some(MethodValue::NativeObject(address, ObjectShape::TwoObjects)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> *mut c_void; env,receiver,(*args).l,(*args.add(1)).l)
            }
            Some(MethodValue::NativeObject(address,ObjectShape::ThreeObjects)) if !args.is_null()=>{
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l)
            }
            Some(MethodValue::NativeObject(address,ObjectShape::StringInt)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).i)
            }
            Some(MethodValue::NativeObject(address,ObjectShape::StringLong)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,i64)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).j)}
            Some(MethodValue::NativeObject(address,ObjectShape::StringFloat)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,f32)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).f)}
            Some(MethodValue::NativeObject(address,ObjectShape::StringBoolean)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,u8)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).z)}
            Some(MethodValue::NativeObject(address, ObjectShape::LongThreeObjects)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).l)
            }
            Some(MethodValue::NativeObject(address, ObjectShape::LongObject)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, i64, *mut c_void) -> *mut c_void; env,receiver,(*args).j,(*args.add(1)).l)
            }
            Some(_) => {
                s.set_pending("java/lang/IllegalArgumentException", "missing native object argument array");
                std::ptr::null_mut()
            }
            None => std::ptr::null_mut(),
        }
    }

    pub unsafe extern "C" fn call_long_method_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue)->i64{call_long(env,receiver,method,args,Dispatch::Virtual)}
    pub unsafe extern "C" fn call_static_long_method_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue)->i64{call_long(env,receiver,method,args,Dispatch::Static)}
    pub(super) unsafe fn call_float(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue, dispatch: Dispatch) -> f32 {
        let Some(s) = session() else { return 0.0; };
        match s.call_checked_dispatch(receiver, method, abi::MethodKind::Float, "CallFloatMethodA", dispatch) {
            Some(MethodValue::NativeFloatNoArgs(address)) => {
                native_call!(address, fn(*mut c_void, *mut c_void) -> f32; env, receiver)
            },
            Some(MethodValue::NativeFloatInt(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void,*mut c_void,i32) ->f32; env,receiver,(*args).i)
            },
            Some(MethodValue::NativeFloatTwoInts(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void,*mut c_void,i32,i32) ->f32; env,receiver,(*args).i,(*args.add(1)).i)
            },
            Some(MethodValue::NativeFloatObjectFloat(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,f32)->f32; env,receiver,(*args).l,(*args.add(1)).f)
            },
            Some(MethodValue::NativeFloatInt(_) | MethodValue::NativeFloatTwoInts(_) | MethodValue::NativeFloatObjectFloat(_)) => {s.set_pending("java/lang/IllegalArgumentException","missing native float argument array");0.0},
            _ => 0.0
        }
    }
    pub unsafe extern "C" fn call_float_method_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue) -> f32 { call_float(env, receiver, method, args, Dispatch::Virtual) }
    pub unsafe extern "C" fn call_static_float_method_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue) -> f32 { call_float(env, receiver, method, args, Dispatch::Static) }
    pub(super) unsafe fn call_long(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue,dispatch:Dispatch)->i64{
        let Some(s)=session()else{return 0;};
        match s.call_checked_dispatch(receiver,method,abi::MethodKind::Long,"CallLongMethodA",dispatch){
            Some(MethodValue::NativeLongNoArgs(address))=>{native_call!(address,fn(*mut c_void,*mut c_void)->i64;env,receiver)},
            Some(MethodValue::NativeLongObjectLong(address)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,i64)->i64;env,receiver,(*args).l,(*args.add(1)).j)},
            Some(_)=>{s.set_pending("java/lang/IllegalArgumentException","missing native long argument array");0},None=>0
        }
    }
    pub unsafe extern "C" fn call_int_method_a(env:*mut c_void,object:*mut c_void,method:*mut c_void,args:*const JValue)->i32 {call_int_a(env,object,method,args,Dispatch::Virtual)}
    pub(super) unsafe fn call_int_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue,dispatch:Dispatch)->i32 {
        let Some(s)=session() else{return 0;};
        match s.call_checked_dispatch(receiver,method,abi::MethodKind::Int,"CallIntMethodA",dispatch) {
            Some(MethodValue::Int(value))=>value as i32,
            Some(MethodValue::NativeInt(address,IntShape::ThreeReferences)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void) ->i32; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l)}
            Some(MethodValue::NativeInt(address,IntShape::ThreeInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32,i32) ->i32; env,receiver,(*args).i,(*args.add(1)).i,(*args.add(2)).i)}
            Some(MethodValue::NativeInt(address,IntShape::TwoInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32) ->i32; env,receiver,(*args).i,(*args.add(1)).i)}
            Some(MethodValue::NativeInt(address,IntShape::Int)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32) ->i32; env,receiver,(*args).i)}
            Some(MethodValue::NativeInt(address,IntShape::NoArgs))=>{native_call!(address, fn(*mut c_void,*mut c_void) ->i32; env,receiver)}
            Some(MethodValue::NativeInt(address,IntShape::Reference)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void) ->i32; env,receiver,(*args).l)}
            Some(MethodValue::NativeInt(address,IntShape::ReferenceInt)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32) ->i32; env,receiver,(*args).l,(*args.add(1)).i)}
            Some(MethodValue::NativeInt(address,IntShape::ReferenceIntInt)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,i32) ->i32; env,receiver,(*args).l,(*args.add(1)).i,(*args.add(2)).i)}
            Some(_)=>{s.set_pending("java/lang/IllegalArgumentException","missing int method arguments");0},None=>0,
        }
    }

    pub(super) fn reflection_strings(s: &Session, name: *mut c_void, signature: *mut c_void) -> Option<(String, String)> {
        let name = s.with_string_bytes(name, |bytes| std::str::from_utf8(bytes).map(str::to_owned))?;
        let signature = s.with_string_bytes(signature, |bytes| std::str::from_utf8(bytes).map(str::to_owned))?;
        match (name, signature) {
            (Ok(name), Ok(signature)) => Some((name, signature)),
            _ => { s.set_pending("java/lang/IllegalArgumentException", "invalid reflection descriptor"); None }
        }
    }
    pub extern "C" fn identity_hash(_env: *mut c_void, _class: *mut c_void, object: *mut c_void) -> i32 {
        let Some(s) = session() else { return 0; };
        if object.is_null() { return 0; }
        let Some(target) = s.resolve(object) else { s.set_pending("java/lang/IllegalArgumentException", "invalid identityHashCode reference"); return 0; };
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        target.hash(&mut hash); let hash = hash.finish(); (hash ^ (hash >> 32)) as i32
    }
    pub extern "C" fn lookup_reflected_method(env: *mut c_void, _registry: *mut c_void, class: *mut c_void, name: *mut c_void, signature: *mut c_void, is_static: u8) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("reflectMethod") { return std::ptr::null_mut(); }
        let Some((name, signature)) = reflection_strings(s, name, signature) else { return std::ptr::null_mut(); };
        let Some(id) = s.reflected_method_id(class, &name, &signature) else { return std::ptr::null_mut(); };
        to_reflected_method(env, class, id, is_static)
    }
    pub extern "C" fn to_reflected_method(_env:*mut c_void,class:*mut c_void,method:*mut c_void,_static:u8)->*mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("ToReflectedMethod") { return std::ptr::null_mut(); }
        let Some(class) = s.resolve_class(class,"ToReflectedMethod") else { return std::ptr::null_mut(); };
        let (tag,id) = handle_parts(method);
        let declaring = s.methods.lock().unwrap().by_id.get(&id).filter(|_|tag==TAG_METHOD).map(|m|m.class);
        if !declaring.is_some_and(|declaring|s.is_subclass(class,declaring)) { s.set_pending("java/lang/IllegalArgumentException","invalid reflected method"); return std::ptr::null_mut(); }
        s.new_ref(RefTarget::ReflectedMethod(id),false)
    }
    pub extern "C" fn from_reflected_method(_env:*mut c_void,object:*mut c_void)->*mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("FromReflectedMethod") { return std::ptr::null_mut(); }
        match s.resolve(object) { Some(RefTarget::ReflectedMethod(id)) => make_handle(TAG_METHOD,id), _=>{ s.set_pending("java/lang/IllegalArgumentException","not a reflected method"); std::ptr::null_mut() } }
    }
    pub extern "C" fn reflected_method_name(_env:*mut c_void,object:*mut c_void)->*mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let Some(RefTarget::ReflectedMethod(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
        let name = s.methods.lock().unwrap().by_id[&id].name.clone(); s.new_string(name.as_bytes())
    }
    pub extern "C" fn reflected_declaring_class(_env:*mut c_void,object:*mut c_void)->*mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        let Some(RefTarget::ReflectedMethod(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
        let class = s.methods.lock().unwrap().by_id[&id].class; s.new_ref(RefTarget::Class(class),false)
    }

    // ---- JNIInvokeInterface (JavaVM) ---------------------------------

    pub extern "C" fn destroy_java_vm(_vm: *mut c_void) -> c_int {
        // Minimal session: nothing to tear down while guests hold handles;
        // the reference returns JNI_OK from its DestroyJavaVM too.
        abi::JNI_OK
    }

    pub unsafe extern "C" fn attach_current_thread(
        _vm: *mut c_void,
        penv: *mut *mut c_void,
        _args: *mut c_void,
    ) -> c_int {
        attach_shared(penv)
    }

    pub unsafe extern "C" fn attach_current_thread_as_daemon(
        _vm: *mut c_void,
        penv: *mut *mut c_void,
        _args: *mut c_void,
    ) -> c_int {
        attach_shared(penv)
    }

    pub extern "C" fn detach_current_thread(_vm: *mut c_void) -> c_int {
        // Ticket 88: drop the calling thread's locals and pending exception.
        let Some(session) = session() else {
            return abi::JNI_OK;
        };
        session.detach_current_thread()
    }

    pub unsafe extern "C" fn get_env(
        _vm: *mut c_void,
        penv: *mut *mut c_void,
        version: c_int,
    ) -> c_int {
        if penv.is_null() {
            return abi::JNI_ERR;
        }
        // SAFETY: the C ABI requires a writable output pointer. Clear it
        // before any failure, including a missing session.
        unsafe { *penv = std::ptr::null_mut() };
        let Some(session) = session() else {
            eprintln!(
                "[android_jni_vm] GetEnv: no session registered — guest \
                 JNI_OnLoad cannot obtain a JNIEnv (ticket 52 minimum missing)"
            );
            return abi::JNI_ERR;
        };
        if !matches!(
            version,
            abi::JNI_VERSION_1_1 | abi::JNI_VERSION_1_2 | abi::JNI_VERSION_1_4 | abi::JNI_VERSION_1_6
        ) {
            session.log(&format!(
                "GetEnv: unsupported JNI version {version:#x} — JNI_EVERSION"
            ));
            return abi::JNI_EVERSION;
        }
        if !session.current_thread_is_attached() {
            return abi::JNI_EDETACHED;
        }
        // SAFETY: non-null caller pointer, documented C ABI contract.
        unsafe { *penv = abi::env_ptr() };
        abi::JNI_OK
    }

    unsafe fn attach_shared(penv: *mut *mut c_void) -> c_int {
        if penv.is_null() {
            return abi::JNI_ERR;
        }
        // SAFETY: non-null caller output pointer per the C ABI.
        unsafe { *penv = std::ptr::null_mut() };
        let Some(session) = session() else {
            eprintln!("[android_jni_vm] AttachCurrentThread: no session registered");
            return abi::JNI_ERR;
        };
        if !session.attach_current_thread() {
            return abi::JNI_ERR;
        }
        // Oracle parity (non-GC jnivm): every thread shares the one env.
        // SAFETY: non-null caller pointer.
        unsafe { *penv = abi::env_ptr() };
        abi::JNI_OK
    }

    // ---- JNINativeInterface (JNIEnv) ---------------------------------

    pub extern "C" fn get_version(_env: *mut c_void) -> c_int {
        abi::JNI_VERSION_1_6
    }

    pub unsafe extern "C" fn find_class(env: *mut c_void, name: *const c_char) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        let name = if name.is_null() { "" } else {
            match CStr::from_ptr(name).to_str() { Ok(name)=>name,Err(_)=>{session.set_pending("java/lang/NoClassDefFoundError","invalid class name encoding");return std::ptr::null_mut();} }
        };
        session.find_class(name).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn throw(env: *mut c_void, obj: *mut c_void) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        let _ = env;
        if obj.is_null() {
            session.log("Throw: null throwable");
            return abi::JNI_ERR;
        }
        session.throw(obj)
    }

    pub unsafe extern "C" fn throw_new(
        env: *mut c_void,
        class: *mut c_void,
        message: *const c_char,
    ) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        let _ = env;
        let Some(_class_id) = session.resolve_class(class, "ThrowNew") else {
            return abi::JNI_ERR;
        };
        let message = if message.is_null() {
            std::borrow::Cow::Borrowed("")
        } else {
            // SAFETY: caller passes a NUL-terminated C string per the ABI.
            unsafe { CStr::from_ptr(message) }.to_string_lossy()
        };
        session.set_pending("java/lang/Throwable", &message);
        abi::JNI_OK
    }

    pub extern "C" fn exception_occurred(env: *mut c_void) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        // Ticket 88: the pending exception is per-thread; the throwable
        // handle is created from the calling thread's own pending state.
        session.exception_occurred()
    }

    pub extern "C" fn exception_describe(env: *mut c_void) {
        let Some(session) = session() else { return };
        let _ = env;
        if let Some(pending) = session.take_pending() {
            session.log(&format!(
                "ExceptionDescribe: {} ({}) — describing and clearing",
                pending.class, pending.message
            ));
        }
    }

    pub extern "C" fn exception_clear(env: *mut c_void) {
        let Some(session) = session() else { return };
        let _ = env;
        session.take_pending();
    }

    pub unsafe extern "C" fn fatal_error(_env: *mut c_void, message: *const c_char) {
        let message = if message.is_null() {
            String::new()
        } else {
            // SAFETY: caller passes a NUL-terminated C string per the ABI.
            unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
        };
        eprintln!("[android_jni_vm] FatalError: {message} — aborting (JNI spec)");
        std::process::abort();
    }

    pub extern "C" fn new_global_ref(env: *mut c_void, obj: *mut c_void) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        session.promote_reference(obj,true)
    }

    pub extern "C" fn delete_global_ref(env: *mut c_void, obj: *mut c_void) {
        if let Some(session) = session() {
            let _ = env;
            session.delete_ref(obj, true, "DeleteGlobalRef");
        }
    }

    pub extern "C" fn delete_local_ref(env: *mut c_void, obj: *mut c_void) {
        if let Some(session) = session() {
            let _ = env;
            session.delete_ref(obj, false, "DeleteLocalRef");
        }
    }

    pub extern "C" fn is_same_object(
        env: *mut c_void,
        a: *mut c_void,
        b: *mut c_void,
    ) -> u8 {
        let Some(session) = session() else {
            return abi::JNI_FALSE;
        };
        let _ = env;
        if a.is_null() && b.is_null() {
            return abi::JNI_TRUE;
        }
        match (session.resolve(a), session.resolve(b)) {
            (Some(target_a), Some(target_b)) => {
                if target_a == target_b {
                    abi::JNI_TRUE
                } else {
                    abi::JNI_FALSE
                }
            }
            (None, None) => abi::JNI_TRUE, // two dead handles compare "same dead"
            _ => abi::JNI_FALSE,
        }
    }

    pub extern "C" fn new_local_ref(env: *mut c_void, obj: *mut c_void) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        session.promote_reference(obj,false)
    }

    pub extern "C" fn new_weak_global_ref(_env:*mut c_void,object:*mut c_void)->*mut c_void {
        let Some(s)=session() else{return std::ptr::null_mut();};if object.is_null(){return std::ptr::null_mut();}
        let Some(target)=s.resolve(object) else {s.set_pending("java/lang/IllegalArgumentException","invalid weak reference source");return std::ptr::null_mut();};
        let Some(id)=s.next_serial() else{s.serial_exhausted("NewWeakGlobalRef");return std::ptr::null_mut();};
        s.weaks.lock().unwrap().insert(id,target);make_handle(TAG_WEAK,id)
    }
    pub extern "C" fn delete_weak_global_ref(_env:*mut c_void,object:*mut c_void) {
        let Some(s)=session() else{return;};if object.is_null(){return;}
        let (tag,id)=handle_parts(object);if tag!=TAG_WEAK {s.log("DeleteWeakGlobalRef: invalid weak handle");return;}
        let mut weaks=s.weaks.lock().unwrap();weaks.remove(&id);trim_reference_table(&mut weaks);
    }
    pub extern "C" fn get_object_ref_type(_env:*mut c_void,object:*mut c_void)->i32 {
        let Some(s)=session() else{return 0;};let(tag,id)=handle_parts(object);
        match tag {TAG_WEAK if s.weaks.lock().unwrap().contains_key(&id)=>3,TAG_GLOBAL if s.resolve(object).is_some()=>2,TAG_LOCAL if s.resolve(object).is_some()=>1,_=>0}
    }

    pub extern "C" fn push_local_frame(_env: *mut c_void, capacity: c_int) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        session.push_local_frame(capacity)
    }

    pub extern "C" fn pop_local_frame(_env: *mut c_void, result: *mut c_void) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        session.pop_local_frame(result)
    }

    pub extern "C" fn ensure_local_capacity(_env: *mut c_void, capacity: c_int) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        session.ensure_local_capacity(capacity)
    }

    pub unsafe extern "C" fn register_natives(
        _env: *mut c_void,
        class: *mut c_void,
        methods: *const JniNativeMethod,
        count: c_int,
    ) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        // SAFETY: the guest passes `count` JNINativeMethod entries per the ABI.
        unsafe { session.register_natives(class, methods, count) }
    }

    pub extern "C" fn unregister_natives(_env: *mut c_void, class: *mut c_void) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        session.unregister_natives(class)
    }

    pub extern "C" fn monitor_enter(_env: *mut c_void, object: *mut c_void) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        session.monitor_enter(object)
    }

    pub extern "C" fn monitor_exit(_env: *mut c_void, object: *mut c_void) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        session.monitor_exit(object)
    }

    pub extern "C" fn alloc_object(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
        session().map(|s| s.alloc_object(class)).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn set_superclass(_env: *mut c_void, _class: *mut c_void,
        child: *mut c_void, parent: *mut c_void) {
        if let Some(s) = session() { s.set_superclass(child, parent); }
    }

    pub extern "C" fn proxy_class(_env: *mut c_void, _registry: *mut c_void, base: *mut c_void, interfaces: *mut c_void) -> *mut c_void {
        session().and_then(|s| s.proxy_class(base, interfaces)).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn get_object_class(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("GetObjectClass") { return std::ptr::null_mut(); }
        s.object_class(object, "GetObjectClass")
            .map(|id| s.new_ref(super::RefTarget::Class(id), false))
            .unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn get_superclass(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
        let Some(s) = session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("GetSuperclass") { return std::ptr::null_mut(); }
        let Some(class) = s.resolve_class(class, "GetSuperclass") else { return std::ptr::null_mut(); };
        let parent = s.classes.lock().unwrap().parents.get(&class).copied();
        parent.map(|id| s.new_ref(RefTarget::Class(id), false)).unwrap_or(std::ptr::null_mut())
    }

    pub extern "C" fn is_instance_of(_env: *mut c_void, object: *mut c_void, class: *mut c_void) -> u8 {
        let Some(s) = session() else { return abi::JNI_FALSE; };
        if s.blocked_by_pending("IsInstanceOf") { return abi::JNI_FALSE; }
        let Some(class) = s.resolve_class(class, "IsInstanceOf") else { return abi::JNI_FALSE; };
        // JNI defines null as assignable to any reference type.
        if object.is_null() { return abi::JNI_TRUE; }
        s.object_class(object, "IsInstanceOf").map(|id| u8::from(s.is_subclass(id, class))).unwrap_or(abi::JNI_FALSE)
    }

    pub extern "C" fn is_assignable_from(_env: *mut c_void, source: *mut c_void, target: *mut c_void) -> u8 {
        session().map_or(abi::JNI_FALSE, |s| s.is_assignable_from(source, target))
    }

    pub unsafe extern "C" fn call_void_method_a(env: *mut c_void, object: *mut c_void,
        method: *mut c_void, args: *const super::JValue) {
        call_void_a(env, object, method, args, Dispatch::Virtual);
    }

    pub unsafe extern "C" fn call_static_void_method_a(env: *mut c_void, class: *mut c_void,
        method: *mut c_void, args: *const super::JValue) {
        call_void_a(env, class, method, args, Dispatch::Static);
    }

    pub(super) unsafe fn call_void_a(env: *mut c_void, object: *mut c_void,
        method: *mut c_void, args: *const super::JValue, dispatch: Dispatch) {
        let Some(s) = session() else { return; };
        let Some(super::MethodValue::NativeVoid(address, shape)) = s.call_checked_dispatch(
            object, method, abi::MethodKind::Void, if dispatch != Dispatch::Static { "CallVoidMethodA" } else { "CallStaticVoidMethodA" }, dispatch) else { return; };
        if shape != super::VoidShape::NoArgs && args.is_null() {
            s.set_pending("java/lang/IllegalArgumentException", "missing native jvalue argument array");
            return;
        }
        // The descriptor is classified once during RegisterNatives. Each branch
        // uses its exact C prototype; the JNI caller supplies readable jvalues.
        match shape {
            super::VoidShape::NoArgs => {
                native_call!(address, fn(*mut c_void, *mut c_void); env, object);
            }
            super::VoidShape::Boolean => {
                native_call!(address, fn(*mut c_void, *mut c_void, u8); env, object, (*args).z);
            }
            super::VoidShape::Long => {native_call!(address, fn(*mut c_void,*mut c_void,i64); env,object,(*args).j)}
            super::VoidShape::Int => {
                native_call!(address, fn(*mut c_void, *mut c_void, i32); env, object, (*args).i);
            }
            super::VoidShape::Float => {
                native_call!(address, fn(*mut c_void, *mut c_void, f32); env, object, (*args).f);
            }
            super::VoidShape::IntInt => {
                native_call!(address, fn(*mut c_void, *mut c_void, i32, i32); env, object, (*args).i, (*args.add(1)).i);
            }
            super::VoidShape::Context => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void); env, object, (*args).l);
            }
            super::VoidShape::ContextInt => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, i32); env, object, (*args).l, (*args.add(1)).i);
            }
            super::VoidShape::IntSurface => {
                native_call!(address, fn(*mut c_void, *mut c_void, i32, *mut c_void); env, object, (*args).i, (*args.add(1)).l);
            }
            super::VoidShape::FieldDeclaration => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, *mut c_void, u8); env, object, (*args).l, (*args.add(1)).l, (*args.add(2)).l, (*args.add(3)).z);
            }
            super::VoidShape::TwoReferences => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void); env, object, (*args).l, (*args.add(1)).l);
            }
            super::VoidShape::ThreeReferences => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void); env,object,(*args).l,(*args.add(1)).l,(*args.add(2)).l);
            }
            super::VoidShape::IntReference => {
                native_call!(address, fn(*mut c_void,*mut c_void,i32,*mut c_void); env,object,(*args).i,(*args.add(1)).l);
            }
            super::VoidShape::ReferenceTwoInts => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,i32); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).i);
            }
            super::VoidShape::LongThreeReferences => {
                native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void,*mut c_void); env,object,(*args).j,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).l);
            }
            super::VoidShape::TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,u8,u8,*mut c_void,*mut c_void,u8,*mut c_void,u8); env,object,(*args).l,(*args.add(1)).l,(*args.add(2)).z,(*args.add(3)).z,(*args.add(4)).l,(*args.add(5)).l,(*args.add(6)).z,(*args.add(7)).l,(*args.add(8)).z);
            }
            super::VoidShape::ReferenceIntFourBooleansReferenceIntTwoBooleans => {
                native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,u8,u8,u8,u8,*mut c_void,i32,u8,u8); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).z,(*args.add(3)).z,(*args.add(4)).z,(*args.add(5)).z,(*args.add(6)).l,(*args.add(7)).i,(*args.add(8)).z,(*args.add(9)).z);
            }
        }
    }

    pub unsafe extern "C" fn call_boolean_method_a(env: *mut c_void, object: *mut c_void,
        method: *mut c_void, args: *const super::JValue) -> u8 {
        call_boolean_a(env, object, method, args, Dispatch::Virtual)
    }

    pub(super) unsafe fn call_boolean_a(env: *mut c_void, receiver: *mut c_void,
        method: *mut c_void, args: *const super::JValue, dispatch: Dispatch) -> u8 {
        let Some(s) = session() else { return abi::JNI_FALSE; };
        let entry = if dispatch != Dispatch::Static { "CallBooleanMethodA" } else { "CallStaticBooleanMethodA" };
        match s.call_checked_dispatch(receiver, method, abi::MethodKind::Bool, entry, dispatch) {
            Some(super::MethodValue::Bool(value)) => u8::from(value),
            Some(super::MethodValue::NativeBoolNoArgs(address)) => {
                native_call!(address, fn(*mut c_void, *mut c_void) -> u8; env, receiver)
            }
            Some(super::MethodValue::NativeBoolInt(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, i32) -> u8; env, receiver, (*args).i)
            }
            Some(super::MethodValue::NativeBoolString(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void) -> u8; env, receiver, (*args).l)
            }
            Some(super::MethodValue::NativeBoolObjectLong(address)) if !args.is_null() => {
                native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, i64) -> u8; env, receiver, (*args).l, (*args.add(1)).j)
            }
            Some(super::MethodValue::NativeBoolObjectBoolean(address)) if !args.is_null()=>{
                native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,u8)->u8;env,receiver,(*args).l,(*args.add(1)).z)
            }
            Some(_) => {
                s.set_pending("java/lang/IllegalArgumentException", "missing native jvalue arguments or unsupported boolean signature");
                abi::JNI_FALSE
            }
            None => abi::JNI_FALSE,
        }
    }

    pub unsafe extern "C" fn get_method_id(
        env: *mut c_void,
        class: *mut c_void,
        name: *const c_char,
        sig: *const c_char,
    ) -> *mut c_void {
        resolve_method_shim(env, class, name, sig, "GetMethodID")
    }

    pub unsafe extern "C" fn get_static_method_id(
        env: *mut c_void,
        class: *mut c_void,
        name: *const c_char,
        sig: *const c_char,
    ) -> *mut c_void {
        resolve_method_shim(env, class, name, sig, "GetStaticMethodID")
    }

    unsafe fn resolve_method_shim(
        env: *mut c_void,
        class: *mut c_void,
        name: *const c_char,
        sig: *const c_char,
        entry: &'static str,
    ) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        let read = |p: *const c_char| {
            if p.is_null() {
                return None;
            }
            // SAFETY: caller passes a NUL-terminated C string per the ABI.
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy())
        };
        let (Some(name), Some(sig)) = (read(name), read(sig)) else {
            session.log(&format!("{entry}: method name or signature is null"));
            session.set_pending("java/lang/NoSuchMethodError", "null name or signature");
            return std::ptr::null_mut();
        };
        session
            .resolve_method(class, &name, &sig, entry)
            .unwrap_or(std::ptr::null_mut())
    }

    pub unsafe extern "C" fn call_static_int_method_a(env:*mut c_void,class:*mut c_void,method:*mut c_void,args:*const JValue)->i32 {call_int_a(env,class,method,args,Dispatch::Static)}

    pub unsafe extern "C" fn call_static_boolean_method_a(
        env: *mut c_void, class: *mut c_void, method: *mut c_void, args: *mut c_void,
    ) -> u8 {
        call_boolean_a(env, class, method, args.cast(), Dispatch::Static)
    }

    pub unsafe extern "C" fn new_string_utf(
        env: *mut c_void,
        bytes: *const c_char,
    ) -> *mut c_void {
        let Some(session) = session() else {
            return std::ptr::null_mut();
        };
        let _ = env;
        if bytes.is_null() {
            session.log("NewStringUTF: null input");
            session.set_pending("java/lang/NullPointerException", "null UTF-8 input");
            return std::ptr::null_mut();
        }
        // SAFETY: caller passes a NUL-terminated C string per the ABI.
        let bytes = unsafe { CStr::from_ptr(bytes) }.to_bytes();
        session.new_string(bytes)
    }

    pub extern "C" fn get_string_utf_length(env: *mut c_void, string: *mut c_void) -> c_int {
        let Some(session) = session() else {
            return 0;
        };
        let _ = env;
        session
            .string_length(string)
            .map(|len| len as c_int)
            .unwrap_or(0)
    }

    pub unsafe extern "C" fn get_string_utf_chars(
        env: *mut c_void,
        string: *mut c_void,
        is_copy: *mut u8,
    ) -> *const c_char {
        let Some(session) = session() else {
            return std::ptr::null();
        };
        let _ = env;
        let chars = session.borrow_string_chars(string);
        if !chars.is_null() && !is_copy.is_null() {
            // SAFETY: caller-provided out-param, non-null checked above.
            unsafe { *is_copy = abi::JNI_FALSE };
        }
        chars
    }

    /// # Safety
    /// Release only an owned, outstanding JNI string borrow, with no users
    /// of that borrow remaining. A matching pointer alone does not own a lease.
    pub unsafe extern "C" fn release_string_utf_chars(
        env: *mut c_void,
        string: *mut c_void,
        chars: *const c_char,
    ) {
        let Some(session) = session() else { return };
        let _ = env;
        // A null string/chars pair is logged as a misuse.
        if string.is_null() || chars.is_null() {
            session.log("ReleaseStringUTFChars: null string or chars pointer");
            return;
        }
        session.release_string_chars(chars);
    }

    pub unsafe extern "C" fn get_java_vm(env: *mut c_void, vm: *mut *mut c_void) -> c_int {
        let Some(session) = session() else {
            return abi::JNI_ERR;
        };
        let _ = env;
        if vm.is_null() {
            session.log("GetJavaVM: vm out-param is null");
            return abi::JNI_ERR;
        }
        // SAFETY: non-null caller pointer.
        unsafe { *vm = abi::java_vm_ptr() };
        abi::JNI_OK
    }

    pub extern "C" fn exception_check(env: *mut c_void) -> u8 {
        let Some(session) = session() else {
            return abi::JNI_FALSE;
        };
        let _ = env;
        if session.with_thread(|s| s.pending.is_some()) {
            abi::JNI_TRUE
        } else {
            abi::JNI_FALSE
        }
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
#[path = "../tests/unit/session_test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "../tests/unit/session.rs"]
mod tests;


// Ticket 88: two-thread deterministic fixture — locals and pending
// exceptions are per-thread; globals are shared; Detach clears the
// thread's state; repeated method resolution does not grow the table.
#[cfg(test)]
#[path = "../tests/unit/session_thread_isolation_tests.rs"]
mod thread_isolation_tests;

// Review regressions: local frames, Throw identity, monitors,
// RegisterNatives and thread-exit cleanup, all driven through the raw env
// table slots of the global session like a guest would.
#[cfg(test)]
#[path = "../tests/unit/session_frame_and_lifetime_tests.rs"]
mod frame_and_lifetime_tests;
