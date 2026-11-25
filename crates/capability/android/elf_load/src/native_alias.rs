//! INPUT: exact aliases and a scoped symbol resolver from a pinned capability
//! OUTPUT: independently revocable native handles in the guest dynamic linker
//! POS: no file discovery, guest ELF mapping, or runtime-specific policy
#![forbid(unsafe_code)]

use rudroid_common::{handles::HandleTable, native_alias::NativeAlias, registry::HostStr};
use std::{
    ffi::{c_int, c_void, CStr, CString},
    sync::{Arc, OnceLock},
};

struct Alias {
    names: [CString; 4],
    symbol: extern "C" fn(HostStr<'_>) -> usize,
}

static ALIAS: OnceLock<Arc<Alias>> = OnceLock::new();
static HANDLES: HandleTable<Alias> = HandleTable::new();

pub(super) extern "C" fn register(alias: &NativeAlias<'_>) -> c_int {
    let names = alias.names.map(|name| {
        name.to_cstr()
            .filter(|name| !name.is_empty())
            .map(CStr::to_owned)
    });
    let [Some(basename), Some(relative), Some(guest), Some(native)] = names else {
        return -1;
    };
    let value = Alias {
        names: [basename, relative, guest, native],
        symbol: alias.symbol,
    };
    if ALIAS.set(Arc::new(value)).is_ok() {
        0
    } else {
        -1
    }
}

fn find(name: &CStr) -> Option<&'static Arc<Alias>> {
    ALIAS.get().filter(|alias| {
        alias
            .names
            .iter()
            .any(|candidate| candidate.as_c_str() == name)
    })
}

pub(super) fn available(name: &CStr) -> bool {
    find(name).is_some()
}

pub(super) fn open(name: &CStr) -> Option<*mut c_void> {
    find(name).map(|alias| HANDLES.insert_arc(Arc::clone(alias)))
}

pub(super) fn close(handle: *mut c_void) -> bool {
    ALIAS.get().is_some() && HANDLES.remove(handle).is_some()
}

pub(super) fn is_handle(handle: *mut c_void) -> bool {
    ALIAS.get().is_some() && HANDLES.get(handle).is_some()
}

pub(super) fn symbol(handle: *mut c_void, name: &CStr) -> Option<usize> {
    ALIAS.get()?;
    let alias = HANDLES.get(handle)?;
    Some((alias.symbol)(HostStr::new(name)))
}

pub(super) fn import(name: &str) -> Option<usize> {
    let alias = ALIAS.get()?;
    let name = CString::new(name).ok()?;
    let result = (alias.symbol)(HostStr::new(&name));
    (result != 0).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_paths_and_independent_handles() {
        extern "C" fn resolve(name: HostStr<'_>) -> usize {
            usize::from(name.to_cstr() == Some(c"defined")) * 142
        }
        let config = NativeAlias {
            names: [
                c"libexample.so",
                c"guest/libexample.so",
                c"/port/guest/libexample.so",
                c"/runtime/libexample.so",
            ]
            .map(HostStr::new),
            symbol: resolve,
        };
        assert_eq!(register(&config), 0);
        assert_ne!(register(&config), 0);
        let first = open(c"libexample.so").unwrap();
        let second = open(c"/runtime/libexample.so").unwrap();
        assert_ne!(first, second);
        assert!(available(c"/port/guest/libexample.so"));
        assert!(open(c"/foreign/libexample.so").is_none());
        assert_eq!(symbol(first, c"defined"), Some(142));
        assert_eq!(symbol(second, c"missing"), Some(0));
        assert!(close(first));
        assert_eq!(symbol(first, c"defined"), None);
        assert_eq!(symbol(second, c"defined"), Some(142));
        assert!(close(second));
        assert_eq!(import("defined"), Some(142));
    }
}
