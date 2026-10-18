//! In-repo Reply Catalog source table.
//!
//! Entries are declared here as Rust `const` data and compiled into the same
//! binary blob that ships with the kit (same-build distribution). The table
//! lives in the format crate so the catalog build script can encode it
//! (`[build-dependencies]` cannot link the crate being built). The C++
//! sources stay a read-only oracle: literal values copied from them (e.g. the
//! android_base setDefault entries, ticket 15) must be byte-identical.
#![forbid(unsafe_code)]

use crate::ReplyValue;

/// One fixed reply: class/method/signature → value.
pub struct SourceEntry {
    /// JVM internal name, e.g. `android/media/AudioManager`.
    pub class: &'static str,
    pub method: &'static str,
    /// JNI descriptor, e.g. `()Z`.
    pub sig: &'static str,
    pub value: ReplyValue,
}

/// The full source table. Values must be byte-identical to the C++ oracle:
/// the `android_base.cpp` setDefault entries (ticket 15), where
/// `TYPE_WIRED_HEADPHONES = 4` comes from `javastubs/android.h`, and the
/// register-phase addon stubs the kit resolves through the catalog instead
/// of native descriptors — `org/fmod/FMOD.checkInit` answers the oracle's
/// `bool FMOD::checkInit() { return true; }` (`sdk_fmod/fakefmod.cpp`,
/// registered `STATIC`, ticket 76). Never contains placeholder junk.
pub static SOURCE_TABLE: &[SourceEntry] = &[
    SourceEntry {
        class: "android/media/AudioManager",
        method: "isBluetoothA2dpOn",
        sig: "()Z",
        value: ReplyValue::Bool(false),
    },
    SourceEntry {
        class: "android/media/AudioManager",
        method: "getStreamVolume",
        sig: "(I)I",
        value: ReplyValue::Int(100),
    },
    SourceEntry {
        class: "android/media/AudioDeviceInfo",
        method: "getType",
        sig: "()I",
        value: ReplyValue::Int(4),
    },
    // Register-phase addon entries: the shared JNI session's
    // GetStaticMethodID resolves these for addon register_jni phases
    // (android_jni_vm consults the catalog; a miss is a structured -3).
    // FMOD.checkInit: oracle fakefmod.cpp `bool FMOD::checkInit() { return
    // true; }` (STATIC descriptor, `()Z`) — the sdk_fmod register join
    // answers this before any guest library loads.
    SourceEntry {
        class: "org/fmod/FMOD",
        method: "checkInit",
        sig: "()Z",
        value: ReplyValue::Bool(true),
    },
];
