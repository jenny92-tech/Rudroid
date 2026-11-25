//! INPUT: shared session state and JNI handles
//! OUTPUT: validated operations on the shared VM
//! POS: checked dispatch for registered Java methods
#![forbid(unsafe_code)]

use super::*;

impl Session {
    /// Call dispatch: blocked-while-pending, shared validity checks, then the
    /// KIND-CHECKED answer — calling through the wrong accessor fails
    /// explicitly (zero + pending), never a silent default. `wanted` names
    /// the calling variant for logs.
    #[cfg(test)]
    pub(super) fn call_checked(
        &self,
        receiver: *mut c_void,
        method: *mut c_void,
        wanted: abi::MethodKind,
        entry: &str,
        instance: bool,
    ) -> Option<MethodValue> {
        self.call_checked_dispatch(receiver, method, wanted, entry, Dispatch::from(instance))
    }

    pub(super) fn call_checked_dispatch(
        &self,
        receiver: *mut c_void,
        method: *mut c_void,
        wanted: abi::MethodKind,
        entry: &str,
        dispatch: Dispatch,
    ) -> Option<MethodValue> {
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
            methods
                .by_id
                .get(&serial)
                .map(|method| (method.class, method.value))
        };
        let Some((method_class, mut value)) = method else {
            self.log(&format!("{entry}: unknown method ID"));
            self.set_pending("java/lang/NoSuchMethodError", "unknown method ID");
            return None;
        };
        // The class argument must reference the method's class.
        let class_id = if dispatch != Dispatch::Static {
            self.object_class(receiver, entry)?
        } else {
            self.resolve_class(receiver, entry)?
        };
        if self.trace_calls.load(Ordering::Relaxed) {
            let (name, sig) = {
                let methods = self.methods.lock().unwrap();
                let method = &methods.by_id[&serial];
                (method.name.clone(), method.sig.clone())
            };
            self.log(&format!("JNI_CALL {entry} {}/{name}{sig} receiver={} thread={:?} id={serial} value={value:?}",self.class_name(method_class),self.class_name(class_id),std::thread::current().id()));
        }
        if class_id != method_class && !self.is_subclass(class_id, method_class) {
            let message = {
                let methods = self.methods.lock().unwrap();
                let method = &methods.by_id[&serial];
                format!(
                    "{entry}: method {}/{} belongs to {} but the caller passed {}",
                    method.name,
                    method.sig,
                    self.class_name(method_class),
                    self.class_name(class_id)
                )
            };
            self.log(&message);
            self.set_pending(
                "java/lang/NoSuchMethodError",
                "method belongs to another class",
            );
            return None;
        }
        if let Dispatch::Nonvirtual(class) = dispatch {
            let supplied = self.resolve_class(class, entry)?;
            if !self.is_subclass(class_id, supplied) {
                self.set_pending(
                    "java/lang/IllegalArgumentException",
                    "nonvirtual class is not a receiver supertype",
                );
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
                (_, Some((name, signature))) => {
                    handle_parts(self.resolve_method_for_class(supplied, &name, &signature, entry)?)
                        .1
                }
                _ => unreachable!("cached identity or cold descriptor"),
            };
            if selected != serial {
                self.set_pending(
                    "java/lang/NoSuchMethodError",
                    "nonvirtual method was not obtained from the supplied class",
                );
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
            if let Some(resolved) = cached {
                value = resolved;
            } else if let Some((name, sig)) = cold {
                let resolved = self.resolve_method_for_class(class_id, &name, &sig, entry)?;
                value = self.methods.lock().unwrap().by_id[&handle_parts(resolved).1].value;
            }
        }
        if matches!(
            value,
            MethodValue::NativeOther(_) | MethodValue::NativeUnbound
        ) {
            self.set_pending(
                "java/lang/UnsatisfiedLinkError",
                "native binding absent or call signature unsupported",
            );
            return None;
        }
        // Type mismatch: calling through the wrong accessor kind fails
        // explicitly (zero + pending), never a silent default.
        let matches_kind = match (wanted, value) {
            (kind, MethodValue::NativeScalar(_, returned, _)) => kind == returned.kind(),
            (abi::MethodKind::Int, MethodValue::Int(_))
            | (
                abi::MethodKind::Float,
                MethodValue::NativeFloatNoArgs(_)
                | MethodValue::NativeFloatInt(_)
                | MethodValue::NativeFloatTwoInts(_)
                | MethodValue::NativeFloatObjectFloat(_),
            )
            | (
                abi::MethodKind::Long,
                MethodValue::NativeLongNoArgs(_) | MethodValue::NativeLongObjectLong(_),
            )
            | (abi::MethodKind::Int, MethodValue::NativeInt(_, _))
            | (
                abi::MethodKind::Bool,
                MethodValue::NativeBoolObjectLong(_) | MethodValue::NativeBoolObjectBoolean(_),
            )
            | (abi::MethodKind::Bool, MethodValue::Bool(_))
            | (abi::MethodKind::Bool, MethodValue::NativeBoolString(_))
            | (
                abi::MethodKind::Bool,
                MethodValue::NativeBoolNoArgs(_)
                | MethodValue::NativeBoolInt(_)
                | MethodValue::NativeBoolObjectInt(_),
            ) => true,
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
                     with a pending exception",
                    method.name, method.sig
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
}
