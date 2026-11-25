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
//! - Unconfigured class lookup retains legacy jnivm parity. A verified catalog
//!   enables lookup from application names and explicitly declared host types;
//!   absent classes never become empty classes as a side effect of a query.
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
//!   static calls preserve exact boolean native prototypes, including (I)Z
//!   and Unity's `(Landroid/view/InputEvent;I)Z` input callback.
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
#[path = "builtins.rs"]
mod builtins;
use builtins::install_builtin_types;
#[path = "session_threads.rs"]
mod threads;
#[path = "heap.rs"]
mod heap;
#[path = "fields.rs"]
mod fields;
#[path = "class_lookup.rs"]
mod class_lookup;
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
    NativeBoolObjectInt(usize),
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
pub enum VoidShape { NoArgs, Boolean, Int, Float, Long, IntInt, Context, ContextInt, IntSurface, IntReference, ReferenceTwoInts, ReferenceIntReference, ReferenceEightInts, LongThreeReferences, TwoReferences, ThreeReferences, FieldDeclaration, TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean, ReferenceIntFourBooleansReferenceIntTwoBooleans }

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
        b"LIL"=>VoidShape::ReferenceIntReference,
        b"LIIIIIIII"=>VoidShape::ReferenceEightInts,
        b"JLLL"=>VoidShape::LongThreeReferences,
        b"LLZZLLZLZ"=>VoidShape::TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean,
        b"LIZZZZLIZZ"=>VoidShape::ReferenceIntFourBooleansReferenceIntTwoBooleans,
        _=>return None,
    };Some(MethodValue::NativeVoid(address,shape))
}
#[derive(Debug,Clone,Copy,PartialEq)]
pub enum IntShape {NoArgs,Int,TwoInts,ThreeInts,Reference,ReferenceInt,ReferenceIntInt,ThreeReferences,FourStringsStringArray}

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
    lookup_catalog: Option<std::collections::HashSet<Arc<str>>>,
    lookup_miss_records: u8,
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
    classes: std::sync::RwLock<ClassTable>,
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
    // The base frame needs no index: popping it clears all locals.
    pushed_locals: std::collections::BTreeMap<usize, Vec<u64>>,

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
    position: usize,
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
            classes: std::sync::RwLock::new(ClassTable::default()),
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
        self.error(&format!("{entry}: JNI handle serials exhausted — refusing to reuse a live id"));
        self.set_pending("java/lang/OutOfMemoryError", "JNI handle serials exhausted");
    }

    /// Structured log through the host table when installed, stderr
    /// otherwise (unit tests drive sessions without a host).
    fn log(&self, message: &str) {
        if let Some(host) = self.host.get() {
            host.log_line(c"android_jni_vm", message);
            return;
        }
        rudroid_common::diagnostic!("[android_jni_vm] {message}");
    }

    fn error(&self, message: &str) {
        if let Some(host) = self.host.get() {
            host.log_error(c"android_jni_vm", message);
        } else {
            rudroid_common::error!("[android_jni_vm] {message}");
        }
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
                self.log(&format!("native binding {}/{}{} address={address:#x}", self.class_name(class_id), name, sig));
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
                "(Landroid/view/InputEvent;I)Z" => MethodValue::NativeBoolObjectInt(address),
                "(Ljava/lang/Runnable;J)Z" | "(Landroid/os/Message;J)Z" => MethodValue::NativeBoolObjectLong(address),
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I" => MethodValue::NativeInt(address,IntShape::ThreeReferences),
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;)I" => MethodValue::NativeInt(address,IntShape::FourStringsStringArray),
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
                MethodValue::NativeBoolString(_) | MethodValue::NativeBoolNoArgs(_) | MethodValue::NativeBoolInt(_) | MethodValue::NativeBoolObjectInt(_) | MethodValue::NativeVoid(_, _) | MethodValue::NativeObject(_, _) | MethodValue::NativeInt(_,_) | MethodValue::NativeScalar(_,_,_) | MethodValue::NativeLongNoArgs(_) | MethodValue::NativeLongObjectLong(_) | MethodValue::NativeFloatNoArgs(_) | MethodValue::NativeFloatInt(_) | MethodValue::NativeFloatTwoInts(_) | MethodValue::NativeFloatObjectFloat(_) | MethodValue::NativeBoolObjectLong(_) | MethodValue::NativeBoolObjectBoolean(_) | MethodValue::NativeOther(_)) {
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
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .by_name
            .get(class)
            .copied()?;
        let methods=self.methods.lock().unwrap_or_else(|p| p.into_inner());
        let serial=methods.find(class_id,name,sig)?;
        match methods.by_id.get(&serial)?.value {
            MethodValue::NativeBoolString(address) | MethodValue::NativeBoolNoArgs(address) | MethodValue::NativeBoolInt(address) | MethodValue::NativeBoolObjectInt(address)
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
            session.error(&message);
            if !env.is_null() {
                session.set_pending("java/lang/InternalError", &message);
            }
        }
        None => rudroid_common::error!("[android_jni_vm] {message}"),
    }
    0
}

// ------------------------------------------------------------------ shims

/// The `extern "C"` table entries. They resolve the global session and
/// delegate; without a session they fail explicitly (JNI_ERR on the VM
/// table, zero results with a stderr diagnostic on the env table) — the
/// table exists from first use, the session semantics only after
/// `android_jni_vm` early-init ran.
#[path = "shims.rs"]
pub mod shims;

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

#[path = "session_handles.rs"]
mod handles;

#[path = "session_classes.rs"]
mod classes;

#[path = "session_dispatch.rs"]
mod dispatch;
