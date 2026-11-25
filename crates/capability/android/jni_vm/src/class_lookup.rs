//! INPUT: trusted class declarations, an optional verified class-name catalog
//! OUTPUT: class lookup without manufacturing absent classes, typed failures
//! POS: shared VM metadata only; no DEX execution or per-title class blacklist
#![forbid(unsafe_code)]

use super::{session, RefTarget, Session};
use std::{collections::HashSet, ffi::c_void, sync::Arc};
use rudroid_common::dex_classes::{valid_class_name, MAX_CLASSES};

// These are standard bootstrap types emitted by the VM/framework's existing
// error paths, including PlayerInputDispatch's cold NoSuchMethodError query.
// Their parent metadata is real; constructors/other methods remain explicit
// missing operations. Never replace this list with a java/ prefix allowance.
pub(super) fn bootstrap_exceptions(s: &Session) {
    for (name, parent) in [
        ("java/lang/Throwable","java/lang/Object"),
        ("java/lang/Error","java/lang/Throwable"),
        ("java/lang/Exception","java/lang/Throwable"),
        ("java/lang/VirtualMachineError","java/lang/Error"),
        ("java/lang/InternalError","java/lang/VirtualMachineError"),
        ("java/lang/OutOfMemoryError","java/lang/VirtualMachineError"),
        ("java/lang/LinkageError","java/lang/Error"),
        ("java/lang/IncompatibleClassChangeError","java/lang/LinkageError"),
        ("java/lang/NoSuchMethodError","java/lang/IncompatibleClassChangeError"),
        ("java/lang/NoSuchFieldError","java/lang/IncompatibleClassChangeError"),
        ("java/lang/NoClassDefFoundError","java/lang/LinkageError"),
        ("java/lang/ClassFormatError","java/lang/LinkageError"),
        ("java/lang/ClassCircularityError","java/lang/LinkageError"),
        ("java/lang/UnsatisfiedLinkError","java/lang/LinkageError"),
        ("java/lang/RuntimeException","java/lang/Exception"),
        ("java/lang/ReflectiveOperationException","java/lang/Exception"),
        ("java/lang/ClassNotFoundException","java/lang/ReflectiveOperationException"),
        ("java/lang/IllegalArgumentException","java/lang/RuntimeException"),
        ("java/lang/NumberFormatException","java/lang/IllegalArgumentException"),
        ("java/lang/IllegalStateException","java/lang/RuntimeException"),
        ("java/lang/IllegalMonitorStateException","java/lang/RuntimeException"),
        ("java/lang/IndexOutOfBoundsException","java/lang/RuntimeException"),
        ("java/lang/ArrayIndexOutOfBoundsException","java/lang/IndexOutOfBoundsException"),
        ("java/lang/StringIndexOutOfBoundsException","java/lang/IndexOutOfBoundsException"),
        ("java/lang/ArrayStoreException","java/lang/RuntimeException"),
        ("java/lang/ClassCastException","java/lang/RuntimeException"),
        ("java/lang/NegativeArraySizeException","java/lang/RuntimeException"),
        ("java/lang/NullPointerException","java/lang/RuntimeException"),
        ("java/lang/UnsupportedOperationException","java/lang/RuntimeException"),
        ("java/util/NoSuchElementException","java/lang/RuntimeException"),
        ("java/io/IOException","java/lang/Exception"),
        ("java/io/FileNotFoundException","java/io/IOException"),
        ("java/io/UnsupportedEncodingException","java/io/IOException"),
        ("java/security/GeneralSecurityException","java/lang/Exception"),
        ("java/security/KeyStoreException","java/security/GeneralSecurityException"),
        ("java/security/NoSuchAlgorithmException","java/security/GeneralSecurityException"),
        ("java/security/cert/CertificateException","java/security/GeneralSecurityException"),
        ("java/security/cert/CertificateEncodingException","java/security/cert/CertificateException"),
    ] {
        let class = s.find_class(name).expect("bootstrap exception type");
        let parent = s.find_class(parent).expect("bootstrap exception parent");
        s.set_superclass(class, parent);
        s.delete_ref(class, false, "bootstrap exception");
        s.delete_ref(parent, false, "bootstrap exception");
    }
}

impl Session {
    pub(super) fn install_class_catalog(&self, names: Vec<String>) -> bool {
        if self.blocked_by_pending("installClassCatalog") { return false; }
        if names.is_empty() || names.len() > MAX_CLASSES
            || names.iter().any(|name| !valid_class_name(name))
            || names.iter().map(String::len).sum::<usize>() > 16 * 1024 * 1024 {
            self.set_pending("java/lang/IllegalArgumentException", "invalid class catalog");
            return false;
        }
        let catalog: HashSet<Arc<str>> = names.into_iter().map(Arc::from).collect();
        let mut classes = self.classes.write().unwrap();
        if let Some(installed) = &classes.lookup_catalog {
            if installed == &catalog { return true; }
            drop(classes);
            self.set_pending("java/lang/IllegalStateException", "a different class catalog is already installed");
            return false;
        }
        // Validation finishes before one atomic publication; a rejected catalog
        // never partially restricts queries. Existing provider declarations stay.
        classes.lookup_catalog = Some(catalog);
        true
    }

    pub(super) fn lookup_class(&self, name: &str, java_lookup: bool) -> Option<*mut c_void> {
        if self.blocked_by_pending("class lookup") { return None; }
        let configured = self.classes.read().unwrap().lookup_catalog.is_some();
        if !configured { return self.find_class(name); }
        let failure = if java_lookup { "java/lang/ClassNotFoundException" }
            else { "java/lang/NoClassDefFoundError" };
        let missing = || {
            let record = {
                let mut classes = self.classes.write().unwrap();
                if classes.lookup_miss_records < 64 { classes.lookup_miss_records += 1;true } else { false }
            };
            if record {
                // Existing diagnostic logging policy; class metadata only, no
                // SDK call arguments, exception messages, memory or registers.
                let name = if valid_class_name(name) { name } else { "<invalid descriptor>" };
                self.log(&format!("class lookup miss: entry={} name={name}",
                    if java_lookup { "Java" } else { "JNI" }));
            }
            self.set_pending(failure, "class absent from declared runtime and application catalog");
            None
        };
        if name.len() > 4096 { return missing(); }
        if let Some(component) = name.strip_prefix('[') {
            if name.bytes().take_while(|b| *b == b'[').count() > 255 { return missing(); }
            if matches!(component, "Z"|"B"|"C"|"S"|"I"|"J"|"F"|"D") {
                return self.find_class(name);
            }
            let component = if component.starts_with('[') { component }
                else if let Some(value) = component.strip_prefix('L').and_then(|s| s.strip_suffix(';')).filter(|value| valid_class_name(value)) { value }
                else { return missing(); };
            let local = self.lookup_class(component, java_lookup)?;
            self.delete_ref(local, false, "array component lookup");
            return self.find_class(name);
        }
        if !valid_class_name(name) { return missing(); }
        let known = {
            let classes = self.classes.read().unwrap();
            classes.by_name.contains_key(name) || classes.lookup_catalog.as_ref().unwrap().contains(name)
        };
        if !known { return missing(); }
        self.find_class(name)
    }
}

pub(super) extern "C" fn declare_class(_env: *mut c_void, _class: *mut c_void, name: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let name = match s.with_string_bytes(name, |bytes| std::str::from_utf8(bytes).map(str::to_owned)) {
        Some(Ok(name)) => name,
        Some(Err(_)) => {
            s.set_pending("java/lang/IllegalArgumentException", "invalid class name encoding");
            return std::ptr::null_mut();
        }
        None => return std::ptr::null_mut(),
    };
    if !valid_class_name(&name) {
        s.set_pending("java/lang/IllegalArgumentException", "invalid class declaration");
        return std::ptr::null_mut();
    }
    s.find_class(&name).unwrap_or(std::ptr::null_mut())
}

pub(super) extern "C" fn install_catalog(_env: *mut c_void, _class: *mut c_void, array: *mut c_void) -> u8 {
    let Some(s) = session() else { return 0; };
    if s.blocked_by_pending("installClassCatalog") { return 0; }
    let Some(RefTarget::Array(id)) = s.resolve(array) else {
        s.set_pending("java/lang/IllegalArgumentException", "class catalog must be a String array");return 0;
    };
    let element = s.arrays.lock().unwrap().get(&id)
        .filter(|a| a.primitive.is_none()).map(|a| a.element_class);
    if !element.is_some_and(|id| s.class_name(id).as_ref() == "java/lang/String") {
        s.set_pending("java/lang/IllegalArgumentException", "class catalog must be a String array");return 0;
    }
    let count = s.array_length(array);
    if count <= 0 || count as usize > MAX_CLASSES {
        s.set_pending("java/lang/IllegalArgumentException", "class catalog size outside bound");return 0;
    }
    let mut names = Vec::with_capacity(count as usize);
    let mut bytes = 0usize;
    for index in 0..count {
        let local = s.object_array_element(array, index);
        let name = s.with_string_bytes(local, |data| {
            if data.len() > 4096 { return None; }
            let name = std::str::from_utf8(data).ok()?;
            valid_class_name(name).then(|| name.to_owned())
        });
        s.delete_ref(local, false, "class catalog element");
        match name {
            Some(Some(name)) if bytes + name.len() <= 16 * 1024 * 1024 => {
                bytes += name.len();names.push(name);
            }
            _ => {
                if !s.with_thread(|t| t.pending.is_some()) {
                    s.set_pending("java/lang/IllegalArgumentException", "invalid class catalog name encoding");
                }
                return 0;
            }
        }
    }
    u8::from(s.install_class_catalog(names))
}

#[cfg(test)]
#[path = "../tests/unit/class_lookup.rs"]
mod tests;
