//! INPUT:  private items of `src/registry.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/registry.rs`; declared there with `#[path]`
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn query(name: &[u8], library: Option<&CStr>, version: Option<&CStr>) -> usize {
    let name = std::ffi::CString::new(name).unwrap();
    // SAFETY: valid strings for the duration of the call.
    unsafe {
        crate::rudroid_bionic_symbol(
            name.as_ptr(),
            library.map_or(std::ptr::null(), CStr::as_ptr),
            version.map_or(std::ptr::null(), CStr::as_ptr),
            std::ptr::null_mut(),
        )
    }
}

fn names_of(providers: &[&Provider]) -> BTreeMap<&'static [u8], Vec<&'static str>> {
    let mut owners: BTreeMap<&'static [u8], Vec<&'static str>> = BTreeMap::new();
    for provider in providers {
        for name in (provider.names)() {
            owners.entry(name).or_default().push(provider.label);
        }
    }
    owners
}

fn libc_names() -> BTreeMap<&'static [u8], Vec<&'static str>> {
    names_of(&LIBC.iter().collect::<Vec<_>>())
}

fn lossy(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

#[test]
fn libc_providers_answer_disjoint_names() {
    let shadowed: Vec<String> = libc_names()
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(name, owners)| format!("{} in {owners:?}", lossy(name)))
        .collect();
    assert!(
        shadowed.is_empty(),
        "every later entry is unreachable; keep one owner: {shadowed:?}"
    );
}

#[test]
fn provider_tables_have_no_internal_duplicates() {
    for provider in LIBC.iter().chain([&LOGGING, &MATH]) {
        let names = (provider.names)();
        let unique: BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{} lists a name twice", provider.label);
    }
}

/// Bare queries try liblog, then libm, then libc: a shared name would
/// make a bare query and a versioned one answer different code.
#[test]
fn bare_query_sources_do_not_shadow_each_other() {
    let libc = libc_names();
    let math: BTreeSet<_> = (MATH.names)().into_iter().collect();
    for name in (LOGGING.names)() {
        assert!(!math.contains(name) && !libc.contains_key(name), "liblog shadows {}", lossy(name));
    }
    for name in &math {
        assert!(!libc.contains_key(name), "libm shadows libc {}", lossy(name));
    }
}

#[test]
fn versioned_names_are_served_and_reject_the_base_node() {
    let libc = libc_names();
    let unique: BTreeSet<_> = VERSIONED.iter().map(|(name, _)| name).collect();
    assert_eq!(unique.len(), VERSIONED.len(), "a name has two version nodes");
    for (name, version) in VERSIONED {
        assert!(libc.contains_key(name), "{} is versioned but no provider answers it", lossy(name));
        assert_ne!(query(name, Some(c"libc.so"), Some(*version)), 0, "{}", lossy(name));
        assert_eq!(query(name, Some(c"libc.so"), Some(c"LIBC")), 0, "{}", lossy(name));
    }
}

/// Every enumerated name answers through the public query with its own
/// library/version contract, and the bare query returns the same code.
#[test]
fn every_enumerated_name_resolves_through_the_public_query() {
    let libc = libc_names();
    for name in libc.keys() {
        let versioned = query(name, Some(c"libc.so"), Some(required_version(name)));
        assert_ne!(versioned, 0, "libc {} does not resolve", lossy(name));
        assert_eq!(query(name, None, None), versioned, "bare libc {}", lossy(name));
    }
    let math = (MATH.names)();
    for name in &math {
        let versioned = query(name, Some(c"libm.so"), Some(c"LIBC"));
        assert_ne!(versioned, 0, "libm {} does not resolve", lossy(name));
        assert_eq!(query(name, None, None), versioned, "bare libm {}", lossy(name));
    }
    assert_eq!(
        query(b"ldexp", Some(c"libc.so"), Some(c"LIBC")),
        query(b"ldexp", Some(c"libm.so"), Some(c"LIBC")),
        "libc.so keeps the legacy ldexp alias to libm"
    );
    let logging = (LOGGING.names)();
    for name in &logging {
        assert_ne!(query(name, None, None), 0, "liblog {}", lossy(name));
        assert_eq!(query(name, Some(c"libc.so"), Some(c"LIBC")), 0, "liblog {} via libc", lossy(name));
    }
    let joined = |names: Vec<&[u8]>| names.into_iter().map(lossy).collect::<Vec<_>>().join(" ");
    println!(
        "BIONIC_SYMBOLS libc={} libm={} liblog={}",
        libc.len(),
        math.len(),
        logging.len()
    );
    println!("BIONIC_SYMBOLS_LIBC {}", joined(libc.keys().copied().collect()));
    println!("BIONIC_SYMBOLS_LIBM {}", joined(math));
    println!("BIONIC_SYMBOLS_LIBLOG {}", joined(logging));
}
