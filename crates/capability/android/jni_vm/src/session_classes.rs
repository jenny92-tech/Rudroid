//! INPUT: shared session state and JNI handles
//! OUTPUT: validated operations on the shared VM
//! POS: class hierarchy and declared method resolution
#![forbid(unsafe_code)]

use super::*;

impl Session {
    pub(super) fn is_subclass(&self, child: u64, parent: u64) -> bool {
        Self::assignable(&mut self.classes.write().unwrap(), child, parent)
    }
    pub(super) fn is_assignable_from(&self, source: *mut c_void, target: *mut c_void) -> u8 {
        if self.blocked_by_pending("IsAssignableFrom") {
            return abi::JNI_FALSE;
        }
        let (Some(source), Some(target)) = (
            self.resolve_class(source, "IsAssignableFrom"),
            self.resolve_class(target, "IsAssignableFrom"),
        ) else {
            return abi::JNI_FALSE;
        };
        u8::from(self.is_subclass(source, target))
    }
    pub(super) fn assignable(classes: &mut ClassTable, child: u64, parent: u64) -> bool {
        classes.visit_epoch = classes.visit_epoch.wrapping_add(1);
        if classes.visit_epoch == 0 {
            for value in classes.visits.values_mut() {
                *value = 0;
            }
            classes.visit_epoch = 1;
        }
        fn walk(
            classes: &mut ClassTable,
            mut child: u64,
            parent: u64,
            depth: usize,
            epoch: u64,
        ) -> bool {
            if depth >= 64 {
                return false;
            }
            if child == parent {
                return true;
            }
            let child_name = Arc::clone(&classes.by_id[&child]);
            let parent_name = Arc::clone(&classes.by_id[&parent]);
            if child_name.starts_with('[') {
                if matches!(
                    parent_name.as_ref(),
                    "java/lang/Object" | "java/lang/Cloneable" | "java/io/Serializable"
                ) {
                    return true;
                }
                if parent_name.starts_with('[') {
                    let component = |name: &str| {
                        let c = &name[1..];
                        let c = if c.starts_with('L') && c.ends_with(';') {
                            &c[1..c.len() - 1]
                        } else if c.starts_with('[') {
                            c
                        } else {
                            return None;
                        };
                        classes.by_name.get(c).copied()
                    };
                    return match (component(&child_name), component(&parent_name)) {
                        (Some(c), Some(p)) => walk(classes, c, p, depth + 1, epoch),
                        _ => false,
                    };
                }
            }
            for level in depth..64 {
                if child == parent {
                    return true;
                }
                let Some(visit) = classes.visits.get_mut(&child) else {
                    return false;
                };
                if *visit == epoch {
                    return false;
                }
                *visit = epoch;
                let count = classes.interfaces.get(&child).map_or(0, Vec::len);
                for i in 0..count {
                    let interface = classes.interfaces[&child][i];
                    if walk(classes, interface, parent, level + 1, epoch) {
                        return true;
                    }
                }
                let Some(next) = classes.parents.get(&child) else {
                    return false;
                };
                child = *next;
            }
            false
        }
        walk(classes, child, parent, 0, classes.visit_epoch)
    }

    pub(super) fn set_superclass(&self, child: *mut c_void, parent: *mut c_void) {
        let (Some(child), Some(parent)) = (
            self.resolve_class(child, "setSuperclass"),
            self.resolve_class(parent, "setSuperclass"),
        ) else {
            return;
        };
        let mut classes = self.classes.write().unwrap();
        if Self::assignable(&mut classes, parent, child) {
            drop(classes);
            self.set_pending(
                "java/lang/ClassCircularityError",
                "cyclic superclass/interface registration",
            );
            return;
        }
        let mut ancestor = parent;
        for depth in 0..64 {
            if ancestor == child || depth == 63 {
                drop(classes);
                self.set_pending(
                    "java/lang/ClassCircularityError",
                    "cyclic or overly deep superclass registration",
                );
                return;
            }
            let Some(next) = classes.parents.get(&ancestor) else {
                break;
            };
            ancestor = *next;
        }
        let object = classes.by_name.get("java/lang/Object").copied();
        if classes
            .parents
            .get(&child)
            .is_some_and(|previous| *previous != parent && Some(*previous) != object)
        {
            drop(classes);
            self.set_pending("java/lang/LinkageError", "class superclass already defined");
            return;
        }
        classes.parents.insert(child, parent);
        drop(classes);
        self.methods.lock().unwrap().aliases.clear();
    }

    pub(super) fn proxy_class(
        &self,
        base: *mut c_void,
        interfaces: *mut c_void,
    ) -> Option<*mut c_void> {
        let base = self.resolve_class(base, "proxyClass")?;
        let Some(RefTarget::Array(array)) = self.resolve(interfaces) else {
            self.set_pending(
                "java/lang/IllegalArgumentException",
                "proxy interfaces must be Class[]",
            );
            return None;
        };
        let expected = self.find_class("[Ljava/lang/Class;")?;
        let expected_id = self.resolve_class(expected, "proxyClass")?;
        self.delete_ref(expected, false, "proxyClass");
        let mut members = {
            let arrays = self.arrays.lock().unwrap();
            let array = arrays.get(&array)?;
            let mut members = Vec::new();
            if array.class != expected_id || array.primitive.is_some() {
                drop(arrays);
                self.set_pending(
                    "java/lang/IllegalArgumentException",
                    "invalid proxy interface array",
                );
                return None;
            }
            if members.try_reserve(array.elements.len()).is_err() {
                drop(arrays);
                self.set_pending(
                    "java/lang/OutOfMemoryError",
                    "proxy interface metadata allocation failed",
                );
                return None;
            }
            for element in &array.elements {
                let Some(RefTarget::Class(class)) = element else {
                    drop(arrays);
                    self.set_pending(
                        "java/lang/IllegalArgumentException",
                        "proxy interface element must be Class",
                    );
                    return None;
                };
                members.push(*class);
            }
            members
        };
        members.sort_unstable();
        members.dedup();
        // Class entity IDs are stable and never reused. Equivalent interface
        // sets share one nominal type, independently of local-reference IDs.
        let mut name = format!("org/rudroid/runtime/Proxy${base}");
        for member in &members {
            use std::fmt::Write;
            write!(name, "${member}").unwrap();
        }
        let class = self.find_class(&name)?;
        let id = self.resolve_class(class, "proxyClass")?;
        let mut classes = self.classes.write().unwrap();
        classes.parents.insert(id, base);
        classes.interfaces.entry(id).or_insert(members);
        drop(classes);
        Some(class)
    }

    pub(super) fn alloc_object(&self, class: *mut c_void) -> *mut c_void {
        if self.blocked_by_pending("AllocObject") {
            return std::ptr::null_mut();
        }
        let Some(class) = self.resolve_class(class, "AllocObject") else {
            return std::ptr::null_mut();
        };
        let Some(id) = self.next_serial() else {
            self.serial_exhausted("AllocObject");
            return std::ptr::null_mut();
        };
        let handle = self.new_ref(RefTarget::Object(id), false);
        if !handle.is_null() {
            self.objects
                .lock()
                .unwrap()
                .insert(id, fields::Object::new(class));
        }
        handle
    }

    pub(super) fn object_class(&self, object: *mut c_void, entry: &str) -> Option<u64> {
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
                let constructor = self
                    .methods
                    .lock()
                    .unwrap()
                    .by_id
                    .get(&id)
                    .is_some_and(|method| method.name.as_ref() == "<init>");
                let class = self.find_class(if constructor {
                    "java/lang/reflect/Constructor"
                } else {
                    "java/lang/reflect/Method"
                })?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            Some(RefTarget::ReflectedField(_)) => {
                let class = self.find_class("java/lang/reflect/Field")?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            Some(RefTarget::Class(_)) => {
                let class = self.find_class("java/lang/Class")?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            Some(RefTarget::Throwable(id)) => {
                let name = self
                    .throwables
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|e| e.class.clone())?;
                let class = self.find_class(&name)?;
                let id = self.resolve_class(class, entry);
                self.delete_ref(class, false, entry);
                id
            }
            _ => {
                self.set_pending(
                    "java/lang/NullPointerException",
                    "not a live object reference",
                );
                None
            }
        }
    }

    pub(super) fn find_class(&self, name: &str) -> Option<*mut c_void> {
        if self.blocked_by_pending("FindClass") {
            return None;
        }
        if name.is_empty() {
            self.log("FindClass: empty class name");
            self.set_pending("java/lang/NoClassDefFoundError", "empty class name");
            return None;
        }
        if name.bytes().take_while(|b| *b == b'[').take(256).count() == 256 {
            self.set_pending("java/lang/NoClassDefFoundError", "array rank exceeds 255");
            return None;
        }
        if let Some(component) = name.strip_prefix('[') {
            let component = if component.starts_with('L') && component.ends_with(';') {
                Some(&component[1..component.len() - 1])
            } else if component.starts_with('[') {
                Some(component)
            } else {
                None
            };
            if let Some(component) = component {
                let handle = self.find_class(component)?;
                self.delete_ref(handle, false, "FindClass component");
            }
        }
        // Auto-register the class (jnivm parity): names are formed like
        // `android/media/AudioManager`; any well-formed name becomes a
        // session class. Values still come only from the catalog.
        let existing = self.classes.read().unwrap().by_name.get(name).copied();
        if let Some(id) = existing {
            let handle = self.new_ref(RefTarget::Class(id), false);
            return (!handle.is_null()).then_some(handle);
        }
        // Another declaring thread may have inserted the class after the read.
        let mut classes = self.classes.write().unwrap();
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
                classes.visits.insert(id, 0);
                if let Some(object) = classes.by_name.get("java/lang/Object").copied() {
                    if object != id {
                        classes.parents.insert(id, object);
                    }
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
    pub(super) fn resolve_method(
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
            self.set_pending(
                "java/lang/NoSuchMethodError",
                "empty method name or signature",
            );
            return None;
        }
        self.resolve_method_for_class(class_id, name, sig, entry)
    }

    pub(super) fn method_declarations(&self, class_id: u64, name: &str) -> Vec<u64> {
        {
            let classes = self.classes.read().unwrap();
            let mut order = vec![class_id];
            if name != "<init>" {
                let mut class = class_id;
                for _ in 0..64 {
                    let Some(parent) = classes.parents.get(&class).copied() else {
                        break;
                    };
                    if order.contains(&parent) {
                        break;
                    }
                    order.push(parent);
                    class = parent;
                }
                let mut i = 0;
                while i < order.len() {
                    let class = order[i];
                    if let Some(interfaces) = classes.interfaces.get(&class) {
                        for interface in interfaces {
                            if !order.contains(interface) {
                                order.push(*interface);
                            }
                        }
                    }
                    if let Some(parent) = classes.parents.get(&class) {
                        if !order.contains(parent) {
                            order.push(*parent);
                        }
                    }
                    i += 1;
                }
            }
            order
        }
    }

    pub(super) fn reflected_method_id(
        &self,
        class: *mut c_void,
        name: &str,
        signature: &str,
    ) -> Option<*mut c_void> {
        if self.blocked_by_pending("reflectMethod") {
            return None;
        }
        let class_id = self.resolve_class(class, "reflectMethod")?;
        let normalized;
        let signature = if signature.contains('.') {
            normalized = signature.replace('.', "/");
            normalized.as_str()
        } else {
            signature
        };
        let declarations = self.method_declarations(class_id, name);
        // An inherited exact overload outranks a child assignable overload.
        for declaring in &declarations {
            if let Some(id) =
                self.resolve_declared_method(*declaring, name, signature, "reflectMethod")
            {
                return Some(make_handle(TAG_METHOD, id));
            }
            if self.with_thread(|t| t.pending.is_some()) {
                return None;
            }
        }
        let mut best = None;
        let mut ambiguous = false;
        for declaring in declarations {
            let candidates = {
                let methods = self.methods.lock().unwrap();
                if let Some(id) = methods.find(declaring, name, signature) {
                    return Some(make_handle(TAG_METHOD, id));
                }
                methods
                    .by_id
                    .iter()
                    .filter(|(_, m)| m.class == declaring && m.name.as_ref() == name)
                    .map(|(id, m)| (*id, m.sig.clone()))
                    .collect::<Vec<_>>()
            };
            for (id, declared) in candidates {
                if let Some(score) = self.reflection_score(&declared, signature) {
                    match best {
                        None => {
                            best = Some((id, score, declared));
                            ambiguous = false;
                        }
                        Some((_, current, _)) if score > current => {
                            best = Some((id, score, declared));
                            ambiguous = false;
                        }
                        Some((_, current, ref previous))
                            if score == current && previous != &declared =>
                        {
                            ambiguous = true
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some((id, _, _)) = best {
            if !ambiguous {
                return Some(make_handle(TAG_METHOD, id));
            }
            self.set_pending(
                "java/lang/NoSuchMethodError",
                "ambiguous reflected overload",
            );
            return None;
        }
        self.resolve_method(class, name, signature, "reflectMethod")
    }

    // Unity's ReflectionHelper accepts binary class names and argument
    // subclasses. The result is the original declaring MethodID, so cached
    // calls still use its exact descriptor and native ABI shape.
    pub(super) fn reflection_score(&self, declared: &str, requested: &str) -> Option<usize> {
        fn types(signature: &str) -> Option<Vec<&str>> {
            let args = signature.strip_prefix('(')?;
            let (args, result) = args.split_once(')')?;
            let mut values = Vec::new();
            let mut offset = 0;
            while offset < args.len() {
                let start = offset;
                while args.as_bytes().get(offset) == Some(&b'[') {
                    offset += 1;
                }
                match *args.as_bytes().get(offset)? {
                    b'L' => {
                        offset += args[offset..].find(';')? + 1;
                    }
                    b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D' => offset += 1,
                    _ => return None,
                }
                values.push(&args[start..offset]);
            }
            if result.is_empty() {
                return None;
            }
            values.push(result);
            Some(values)
        }
        fn reference(name: &str) -> Option<&str> {
            if name.starts_with('L') {
                name.strip_prefix('L')?.strip_suffix(';')
            } else if name.starts_with('[') {
                Some(name)
            } else {
                None
            }
        }
        let declared = types(declared)?;
        let requested = types(requested)?;
        if declared.len() != requested.len() {
            return None;
        }
        let mut score = 0;
        for (i, (expected, actual)) in declared.iter().zip(&requested).enumerate() {
            if expected == actual {
                score += 4;
                continue;
            }
            let (expected, actual) = (reference(expected)?, reference(actual)?);
            // Unity uses Object as the signature hint for a null argument and
            // for a generic object return. It is a lowest-score wildcard.
            if actual == "java/lang/Object" {
                continue;
            }
            let ids = {
                let classes = self.classes.read().unwrap();
                (
                    classes.by_name.get(expected).copied(),
                    classes.by_name.get(actual).copied(),
                )
            };
            let (Some(expected), Some(actual)) = ids else {
                return None;
            };
            let compatible = if i == declared.len() - 1 {
                self.is_subclass(expected, actual)
            } else {
                self.is_subclass(actual, expected)
            };
            if !compatible {
                return None;
            }
            score += 1;
        }
        Some(score)
    }

    pub(super) fn resolve_declared_method(
        &self,
        declaring: u64,
        name: &str,
        sig: &str,
        entry: &str,
    ) -> Option<u64> {
        if let Some(id) = self.methods.lock().unwrap().find(declaring, name, sig) {
            return Some(id);
        }
        let value = self.catalog_query(&self.class_name(declaring), name, sig)?;
        let mut methods = self.methods.lock().unwrap();
        if let Some(id) = methods.find(declaring, name, sig) {
            return Some(id);
        }
        let Some(id) = self.next_serial() else {
            drop(methods);
            self.serial_exhausted(entry);
            return None;
        };
        methods.insert(
            id,
            Method {
                class: declaring,
                name: name.into(),
                sig: sig.into(),
                value,
                next_collision: 0,
            },
        );
        Some(id)
    }

    pub(super) fn resolve_method_for_class(
        &self,
        class_id: u64,
        name: &str,
        sig: &str,
        entry: &str,
    ) -> Option<*mut c_void> {
        if let Some(id) = self
            .methods
            .lock()
            .unwrap()
            .find_cached(class_id, name, sig)
        {
            return Some(make_handle(TAG_METHOD, id));
        }
        // Cold resolution walks class implementations before interface defaults.
        // Warm inherited calls cache only the declaring method ID.
        let declarations = self.method_declarations(class_id, name);
        for declaring in declarations {
            let id = self.resolve_declared_method(declaring, name, sig, entry);
            if let Some(id) = id {
                self.methods.lock().unwrap().cache_alias(class_id, id);
                return Some(make_handle(TAG_METHOD, id));
            }
        }
        self.log(&format!("{entry}: no catalog entry for {}/{name}{sig} (unknown method or wrong signature) — pending NoSuchMethodError", self.class_name(class_id)));
        self.set_pending("java/lang/NoSuchMethodError", "no catalog entry");
        None
    }

    pub(super) fn class_name(&self, id: u64) -> Arc<str> {
        self.classes
            .read()
            .unwrap()
            .by_id
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Arc::from("<unregistered>"))
    }
}
