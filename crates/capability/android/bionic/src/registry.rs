//! INPUT:  every android_bionic Rust/C symbol source and the Android libc.map version nodes
//! OUTPUT:  liblog/libm/libc provider lists, version requirements and enumerable names
//! POS:    the single place deciding which provider answers an Android name; duplicates are test failures
//!
//! Every provider is a table whose lookup and name list come from the same
//! source (`symbol_provider!` match arms in Rust, `RUDROID_SYMBOL_PROVIDER` in
//! `symbol_table.h` for C). Lookups stay allocation-free and linear in the
//! same order as before; the order is no longer a silent tie-breaker because
//! the tests below reject any name two libc providers both answer.

use std::ffi::{c_char, CStr};

/// Define `symbol(name) -> usize` and `NAMES` for one provider module from a
/// single list of match arms, so the enumerable names cannot drift from the
/// lookup. `unsafe;` makes the lookup an `unsafe fn`.
macro_rules! symbol_provider {
    (@emit [$($unsafety:tt)*] $($($name:literal)|+ => $address:expr),+ $(,)?) => {
        pub(crate) $($unsafety)* fn symbol(name: &std::ffi::CStr) -> usize {
            match name.to_bytes() {
                $($($name)|+ => $address,)+
                _ => 0,
            }
        }
        /// Every name `symbol` answers, in table order.
        pub(crate) const NAMES: &[&[u8]] = &[$($($name),+),+];
    };
    (unsafe; $($rest:tt)*) => { symbol_provider!(@emit [unsafe] $($rest)*); };
    ($($rest:tt)*) => { symbol_provider!(@emit [] $($rest)*); };
}

/// One symbol source in the resolution order.
pub(crate) struct Provider {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) label: &'static str,
    pub(crate) lookup: unsafe fn(&CStr) -> usize,
    /// Enumerates exactly the names `lookup` answers (tests and audits only).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) names: fn() -> Vec<&'static [u8]>,
}

fn c_names(name_at: unsafe extern "C" fn(usize) -> *const c_char) -> Vec<&'static [u8]> {
    let mut names = Vec::new();
    for index in 0.. {
        // SAFETY: the C enumerator returns NULL past the end and otherwise a
        // string literal from its static table.
        let name = unsafe { name_at(index) };
        if name.is_null() {
            break;
        }
        // SAFETY: as above; table strings live for the process.
        names.push(unsafe { CStr::from_ptr(name) }.to_bytes());
    }
    names
}

macro_rules! c_provider {
    ($provider:ident, $label:literal, $lookup:ident, $name_at:ident) => {
        unsafe extern "C" {
            fn $lookup(name: *const c_char) -> usize;
            fn $name_at(index: usize) -> *const c_char;
        }
        const $provider: Provider = Provider {
            label: $label,
            // SAFETY: NUL-terminated name for the C table walk.
            lookup: |name| unsafe { $lookup(name.as_ptr()) },
            names: || c_names($name_at),
        };
    };
}

macro_rules! rust_provider {
    ($provider:ident, $label:literal, $lookup:path, $names:path) => {
        const $provider: Provider = Provider {
            label: $label,
            lookup: $lookup,
            names: || $names.to_vec(),
        };
    };
}

c_provider!(LOGGING_C, "logging.c", rudroid_bionic_logging_symbol, rudroid_bionic_logging_symbol_name);
c_provider!(MATH_C, "math.c", rudroid_bionic_math_symbol, rudroid_bionic_math_symbol_name);
c_provider!(LOCALE_C, "locale.c", rudroid_bionic_locale_symbol, rudroid_bionic_locale_symbol_name);
c_provider!(JUMP_C, "jump.c", rudroid_bionic_jump_symbol, rudroid_bionic_jump_symbol_name);
c_provider!(SIGNALS_C, "signals.c", rudroid_bionic_signal_symbol, rudroid_bionic_signal_symbol_name);
c_provider!(NETWORK_C, "network.c", rudroid_bionic_network_symbol, rudroid_bionic_network_symbol_name);
c_provider!(ATFORK_C, "atfork.c", rudroid_bionic_atfork_symbol, rudroid_bionic_atfork_symbol_name);
c_provider!(POSIX_C, "posix.c", rudroid_bionic_posix_symbol, rudroid_bionic_posix_symbol_name);
c_provider!(STDIO_FORMAT_C, "stdio_variadic.c", rudroid_bionic_stdio_format_symbol, rudroid_bionic_stdio_format_symbol_name);

/// Allocation family and C++ destructor registration.
mod alloc {
    symbol_provider! {
        b"malloc" => libc::malloc as *const () as usize,
        b"calloc" => libc::calloc as *const () as usize,
        b"realloc" => libc::realloc as *const () as usize,
        b"free" => libc::free as *const () as usize,
        b"posix_memalign" => libc::posix_memalign as *const () as usize,
        b"memalign" => libc::memalign as *const () as usize,
        b"aligned_alloc" => libc::aligned_alloc as *const () as usize,
        b"malloc_usable_size" => libc::malloc_usable_size as *const () as usize,
        b"__cxa_atexit" => crate::destructors::register as *const () as usize,
        b"__cxa_finalize" => crate::destructors::finalize as *const () as usize,
        b"__cxa_thread_atexit_impl" => crate::destructors::register_thread as *const () as usize,
    }
}

rust_provider!(ALLOC, "registry.rs alloc", alloc::symbol, alloc::NAMES);
rust_provider!(BASIC, "basic.rs", crate::basic::symbol, crate::basic::NAMES);
rust_provider!(SYNC, "sync.rs", crate::sync::symbol, crate::sync::NAMES);
rust_provider!(THREADS, "threads.rs", crate::threads::symbol, crate::threads::NAMES);
rust_provider!(DIAGNOSTICS, "diagnostics.rs", crate::diagnostics::symbol, crate::diagnostics::NAMES);
rust_provider!(MULTIBYTE, "multibyte.rs", crate::multibyte::symbol, crate::multibyte::NAMES);
rust_provider!(IDENTITY, "posix.rs", crate::posix::symbol, crate::posix::NAMES);
rust_provider!(STDIO, "stdio.rs", crate::stdio::symbol, crate::stdio::NAMES);

/// liblog: answered only for bare (unversioned) queries, before anything else.
pub(crate) static LOGGING: Provider = LOGGING_C;
/// libm.so/LIBC, bare queries, and libc.so's legacy `ldexp`.
pub(crate) static MATH: Provider = MATH_C;
/// libc.so providers in lookup order. Their names are disjoint (tested), so
/// the order only affects lookup cost, never which address answers.
pub(crate) static LIBC: [Provider; 15] = [
    ALLOC, BASIC, SYNC, THREADS, DIAGNOSTICS, MULTIBYTE, IDENTITY, LOCALE_C, JUMP_C, SIGNALS_C,
    NETWORK_C, ATFORK_C, POSIX_C, STDIO, STDIO_FORMAT_C,
];

/// Android `libc.map.txt` nodes for the supported names outside base `LIBC`.
pub(crate) const VERSIONED: &[(&[u8], &CStr)] = &[
    (b"pthread_barrier_init", c"LIBC_N"),
    (b"pthread_barrier_destroy", c"LIBC_N"),
    (b"pthread_barrier_wait", c"LIBC_N"),
    (b"pthread_barrierattr_init", c"LIBC_N"),
    (b"pthread_barrierattr_destroy", c"LIBC_N"),
    (b"pthread_barrierattr_getpshared", c"LIBC_N"),
    (b"pthread_barrierattr_setpshared", c"LIBC_N"),
    (b"pthread_spin_init", c"LIBC_N"),
    (b"pthread_spin_destroy", c"LIBC_N"),
    (b"pthread_spin_lock", c"LIBC_N"),
    (b"pthread_spin_trylock", c"LIBC_N"),
    (b"pthread_spin_unlock", c"LIBC_N"),
    (b"aligned_alloc", c"LIBC_P"),
    (b"fopen64", c"LIBC_N"),
    (b"fseeko64", c"LIBC_N"),
    (b"ftello64", c"LIBC_N"),
    (b"tmpfile64", c"LIBC_N"),
    (b"pthread_getname_np", c"LIBC_O"),
    (b"pthread_attr_getinheritsched", c"LIBC_P"),
    (b"pthread_attr_setinheritsched", c"LIBC_P"),
    (b"pthread_mutexattr_getprotocol", c"LIBC_P"),
    (b"pthread_mutexattr_setprotocol", c"LIBC_P"),
];

/// The libc.so version node a name must be requested with.
pub(crate) fn required_version(name: &[u8]) -> &'static CStr {
    VERSIONED
        .iter()
        .find(|(versioned, _)| *versioned == name)
        .map_or(c"LIBC", |(_, version)| version)
}

/// First non-zero libc.so provider answer (at most one can answer).
///
/// # Safety
/// `name` must stay valid for the call; see `rudroid_bionic_symbol`.
pub(crate) unsafe fn libc_symbol(name: &CStr) -> usize {
    for provider in &LIBC {
        let address = (provider.lookup)(name);
        if address != 0 {
            return address;
        }
    }
    0
}

#[cfg(test)]
#[path = "../tests/unit/registry.rs"]
mod tests;
