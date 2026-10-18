//! INPUT: declared class fields and shared JNI object/reference identities
//! OUTPUT: stable field IDs, checked typed values and owned reference edges
//! POS: the JNI field authority indexes exact declarations without copying names or retaining handles
use super::{ handle_parts, make_handle, RefTarget, Session, TAG_FIELD};
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::hash::BuildHasher;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FieldValue {
    Object(Option<RefTarget>), Boolean(u8), Byte(i8), Char(u16), Short(i16),
    Int(i32), Long(i64), Float(f32), Double(f64),
}
impl FieldValue {
    pub(super) fn reference(self) -> Option<RefTarget> {
        match self { Self::Object(target) => target, _ => None }
    }
    fn zero(signature: &str) -> Option<Self> {
        Some(match signature {
            "Z" => Self::Boolean(0), "B" => Self::Byte(0), "C" => Self::Char(0),
            "S" => Self::Short(0), "I" => Self::Int(0), "J" => Self::Long(0),
            "F" => Self::Float(0.0), "D" => Self::Double(0.0),
            s if reference_name(s).is_some() => Self::Object(None), _ => return None,
        })
    }
    fn same_kind(self, other: Self) -> bool { std::mem::discriminant(&self) == std::mem::discriminant(&other) }
}
fn reference_name(signature: &str) -> Option<&str> {
    if let Some(name) = signature.strip_prefix('L').and_then(|s| s.strip_suffix(';')) {
        (!name.is_empty() && !name.contains([';', '[', '.'])).then_some(name)
    } else if signature.starts_with('[') {
        let component = signature.trim_start_matches('[');
        (signature.len() - component.len() <= 255 &&
            (matches!(component, "Z"|"B"|"C"|"S"|"I"|"J"|"F"|"D") ||
             component.starts_with('L') && reference_name(component).is_some())).then_some(signature)
    } else { None }
}

pub(super) struct Object {
    pub class: u64,
    pub fields: HashMap<u64, FieldValue>,
    pub builder: Option<Vec<u8>>,
    // Keep the slot pointer-sized for every non-stream Java object.
    pub stream: Option<Box<super::stream_buffer::FileStream>>,
    pub direct: Option<super::direct_buffers::Buffer>,
    pub marked: Cell<bool>,
    // Collector scratch, accessed only under the heap/liveness locks.
    pub trace_next: Cell<Option<RefTarget>>,
    pub incoming: Cell<usize>,
}
impl Object {
    pub(super) fn new(class: u64) -> Self {
        Self { class, fields:HashMap::new(), builder:None, stream:None, direct:None, marked:Cell::new(false), trace_next:Cell::new(None), incoming:Cell::new(0) }
    }
}
pub(super) struct Field {
    class: u64, name: Box<str>, signature: Box<str>, is_static: bool,
    value: FieldValue, reference_class: Option<u64>,
    next_collision: u64,
}
#[derive(Default)]
pub(super) struct Fields {
    records: HashMap<u64, Field>,
    // Only fingerprints and IDs: metadata remains owned once by records.
    by_key: HashMap<u64, u64>,
}
impl Fields {
    fn find(&self, class: u64, name: &str, signature: &str, is_static: bool) -> Option<u64> {
        let key = self.by_key.hasher().hash_one((class, name, signature, is_static));
        self.find_from(*self.by_key.get(&key)?, class, name, signature, is_static)
    }
    fn find_from(&self, mut id: u64, class: u64, name: &str, signature: &str, is_static: bool) -> Option<u64> {
        while id != 0 {
            let field = self.records.get(&id)?;
            if field.class == class && field.name.as_ref() == name
                && field.signature.as_ref() == signature && field.is_static == is_static {
                return Some(id);
            }
            id = field.next_collision;
        }
        None
    }
    fn insert(&mut self, id: u64, mut field: Field) {
        let key = self.by_key.hasher().hash_one((field.class, field.name.as_ref(), field.signature.as_ref(), field.is_static));
        field.next_collision = self.by_key.insert(key, id).unwrap_or(0);
        self.records.insert(id, field);
    }
}

#[cfg(test)]
#[path = "../tests/unit/field_index.rs"]
mod index_tests;

impl Session {
    pub(super) fn define_field(&self, class: u64, name: &str, signature: &str, is_static: bool) -> Option<u64> {
        let Some(value) = FieldValue::zero(signature).filter(|_| !name.is_empty()) else {
            self.set_pending("java/lang/ClassFormatError", "invalid field declaration"); return None;
        };
        let reference_class = if let Some(name) = reference_name(signature) {
            let handle = self.find_class(name)?;
            let id = self.resolve_class(handle, "defineField")?;
            self.delete_ref(handle, false, "defineField"); Some(id)
        } else { None };
        let mut fields = self.fields.lock().unwrap();
        if let Some(id) = fields.find(class, name, signature, is_static) { return Some(id); }
        let Some(id) = self.next_serial() else {
            drop(fields); self.serial_exhausted("defineField"); return None;
        };
        fields.insert(id, Field { class, name:name.into(), signature:signature.into(), is_static, value, reference_class, next_collision:0 });
        Some(id)
    }
    pub(super) fn resolve_field(&self, class: *mut c_void, name: &str, signature: &str, is_static: bool) -> *mut c_void {
        if self.blocked_by_pending("GetFieldID") { return std::ptr::null_mut(); }
        let Some(class_id) = self.resolve_class(class, "GetFieldID") else { return std::ptr::null_mut(); };
        if let Some(id) = self.fields.lock().unwrap().find(class_id, name, signature, is_static) {
            return make_handle(TAG_FIELD, id);
        }
        // Preserve the existing inheritance/interface order on a miss. Do not
        // cache inherited aliases: later declarations may shadow a parent.
        for class in self.method_declarations(class_id, "field") {
            if let Some(id) = self.fields.lock().unwrap().find(class, name, signature, is_static) {
                return make_handle(TAG_FIELD, id);
            }
        }
        self.log(&format!("missing field {name}:{signature} on {}", self.class_name(class_id)));
        self.set_pending("java/lang/NoSuchFieldError", name); std::ptr::null_mut()
    }
    // Unity's managed reflection supplies Object as a reference type hint.
    // Select the declared field identity; do not add a second field/value.
    pub(super) fn reflected_field_id(&self, class: *mut c_void, name: &str, signature: &str, is_static: bool) -> Option<u64> {
        if self.blocked_by_pending("reflectField") { return None; }
        let class_id = self.resolve_class(class, "reflectField")?;
        if let Some(id) = self.fields.lock().unwrap().find(class_id, name, signature, is_static) {
            return Some(id);
        }
        for class in self.method_declarations(class_id, "field") {
            let found = {
                let fields = self.fields.lock().unwrap();
                let matching = |f: &&Field| f.class == class && f.name.as_ref() == name && f.is_static == is_static;
                fields.find(class, name, signature, is_static)
                    .or_else(|| (signature == "Ljava/lang/Object;").then(|| fields.records.iter()
                        .find(|(_,f)| matching(f) && reference_name(&f.signature).is_some())
                        .map(|(id,_)| *id)).flatten())
            };
            if found.is_some() { return found; }
        }
        self.set_pending("java/lang/NoSuchFieldError", name); None
    }
    fn field_info(&self, receiver: *mut c_void, field: *mut c_void, kind: FieldValue, is_static: bool) -> Option<(u64, FieldValue, Option<u64>)> {
        if self.blocked_by_pending("JNI field access") { return None; }
        let (tag, id) = handle_parts(field);
        let info = self.fields.lock().unwrap().records.get(&id)
            .filter(|f| tag == TAG_FIELD && f.is_static == is_static && f.value.same_kind(kind))
            .map(|f| (f.class, f.value, f.reference_class));
        let Some((declaring, zero, reference)) = info else {
            self.set_pending("java/lang/IllegalArgumentException", "invalid field ID or accessor kind"); return None;
        };
        let class = if is_static { self.resolve_class(receiver, "static field")? } else { self.object_class(receiver, "instance field")? };
        if !self.is_subclass(class, declaring) || !is_static && !matches!(self.resolve(receiver), Some(RefTarget::Object(_))) {
            self.set_pending("java/lang/IllegalArgumentException", "field receiver has incompatible class"); return None;
        }
        Some((id, zero, reference))
    }
    // Object snapshots acquire a temporary root while the owner is locked.
    // A concurrent replacement cannot reclaim the payload before NewLocalRef.
    pub(super) fn get_field(&self, receiver: *mut c_void, field: *mut c_void, kind: FieldValue, is_static: bool) -> Option<FieldValue> {
        let _receiver_root = if is_static { None } else { self.root_reference(receiver) };
        let (id, zero, _) = self.field_info(receiver, field, kind, is_static)?;
        let object = if is_static { None } else { match _receiver_root.as_ref().map(|root| root.target) { Some(RefTarget::Object(id)) => Some(id), _ => return None } };
        let mut live = self.liveness.lock().unwrap();
        let value = if is_static { self.fields.lock().unwrap().records[&id].value } else {
            self.objects.lock().unwrap().get(&object.unwrap())?.fields.get(&id).copied().unwrap_or(zero)
        };
        if let Some(RefTarget::Str(entity)|RefTarget::Throwable(entity)|RefTarget::Object(entity)|RefTarget::Array(entity)) = value.reference() {
            let count = live.entry(entity).or_default();
            *count = count.checked_add(1).expect("JNI pin overflow");
        }
        Some(value)
    }
    pub(super) fn set_field(&self, receiver: *mut c_void, field: *mut c_void, value: FieldValue, is_static: bool) {
        let _receiver_root = if is_static { None } else { self.root_reference(receiver) };
        let Some((id, _, reference_class)) = self.field_info(receiver, field, value, is_static) else { return; };
        if let Some(target) = value.reference() {
            let handle = self.new_ref(target, false);
            if handle.is_null() { return; }
            let actual = self.object_class(handle, "SetObjectField");
            self.delete_ref(handle, false, "SetObjectField");
            if !actual.is_some_and(|actual| reference_class.is_some_and(|wanted| self.is_subclass(actual, wanted))) {
                self.set_pending("java/lang/IllegalArgumentException", "incompatible object field value"); return;
            }
            if is_static { self.pin_target(target); } else { self.retain_edge(target); }
        }
        let previous = if is_static {
            std::mem::replace(&mut self.fields.lock().unwrap().records.get_mut(&id).unwrap().value, value)
        } else {
            let Some(RefTarget::Object(object)) = _receiver_root.as_ref().map(|root| root.target) else {
                if let Some(target) = value.reference() { self.release_target_inner(target, true); }
                return;
            };
            let mut objects = self.objects.lock().unwrap();
            let Some(object) = objects.get_mut(&object) else {
                drop(objects);
                if let Some(target) = value.reference() { self.release_target_inner(target, true); }
                return;
            };
            object.fields.insert(id, value).unwrap_or(FieldValue::Object(None))
        };
        if let Some(target) = previous.reference() { self.release_target_inner(target, !is_static); }
    }
}

pub(super) mod shims {
    use super::*;
    pub extern "C" fn to_reflected_field(_env:*mut c_void,class:*mut c_void,field:*mut c_void,is_static:u8)->*mut c_void{
        let Some(s)=super::super::session()else{return std::ptr::null_mut();};if s.blocked_by_pending("ToReflectedField"){return std::ptr::null_mut();}
        let Some(class)=s.resolve_class(class,"ToReflectedField")else{return std::ptr::null_mut();};let (tag,id)=handle_parts(field);
        let declaring=s.fields.lock().unwrap().records.get(&id).filter(|field|tag==TAG_FIELD&&field.is_static==(is_static!=0)).map(|field|field.class);
        if !declaring.is_some_and(|parent|s.is_subclass(class,parent)){s.set_pending("java/lang/IllegalArgumentException","field does not belong to class or static kind");return std::ptr::null_mut();}s.new_ref(RefTarget::ReflectedField(id),false)
    }
    pub extern "C" fn from_reflected_field(_env:*mut c_void,object:*mut c_void)->*mut c_void{
        let Some(s)=super::super::session()else{return std::ptr::null_mut();};if s.blocked_by_pending("FromReflectedField"){return std::ptr::null_mut();}match s.resolve(object){Some(RefTarget::ReflectedField(id))=>make_handle(TAG_FIELD,id),_=>{s.set_pending("java/lang/IllegalArgumentException","not a reflected field");std::ptr::null_mut()}}
    }
    pub extern "C" fn field_name(_env:*mut c_void,object:*mut c_void)->*mut c_void{let Some(s)=super::super::session()else{return std::ptr::null_mut();};let Some(RefTarget::ReflectedField(id))=s.resolve(object)else{return std::ptr::null_mut();};let name=s.fields.lock().unwrap().records[&id].name.clone();s.new_string(name.as_bytes())}
    pub extern "C" fn field_declaring_class(_env:*mut c_void,object:*mut c_void)->*mut c_void{let Some(s)=super::super::session()else{return std::ptr::null_mut();};let Some(RefTarget::ReflectedField(id))=s.resolve(object)else{return std::ptr::null_mut();};let class=s.fields.lock().unwrap().records[&id].class;s.new_ref(RefTarget::Class(class),false)}
    pub extern "C" fn field_signature(_env:*mut c_void,_registry:*mut c_void,object:*mut c_void)->*mut c_void{let Some(s)=super::super::session()else{return std::ptr::null_mut();};let Some(RefTarget::ReflectedField(id))=s.resolve(object)else{return std::ptr::null_mut();};let signature=s.fields.lock().unwrap().records[&id].signature.clone();s.new_string(signature.as_bytes())}
    pub extern "C" fn lookup_reflected_field(_env: *mut c_void, _registry: *mut c_void, class: *mut c_void, name: *mut c_void, signature: *mut c_void, is_static: u8) -> *mut c_void {
        let Some(s) = super::super::session() else { return std::ptr::null_mut(); };
        if s.blocked_by_pending("reflectField") { return std::ptr::null_mut(); }
        let Some((name, signature)) = super::super::shims::reflection_strings(s, name, signature) else { return std::ptr::null_mut(); };
        s.reflected_field_id(class, &name, &signature, is_static != 0)
            .map(|id| s.new_ref(RefTarget::ReflectedField(id), false)).unwrap_or(std::ptr::null_mut())
    }
    pub extern "C" fn define_field(_env: *mut c_void, _types: *mut c_void, class: *mut c_void,
        name: *mut c_void, signature: *mut c_void, is_static: u8) {
        let Some(s) = super::super::session() else { return; };
        let Some(class) = s.resolve_class(class, "defineField") else { return; };
        let Some(name) = s.with_string_bytes(name, |bytes| std::str::from_utf8(bytes).map(str::to_owned)) else { return; };
        let Some(signature) = s.with_string_bytes(signature, |bytes| std::str::from_utf8(bytes).map(str::to_owned)) else { return; };
        if let (Ok(name), Ok(signature)) = (name, signature) { s.define_field(class, &name, &signature, is_static != 0); }
        else { s.set_pending("java/lang/ClassFormatError", "field metadata is not UTF-8"); }
    }
    macro_rules! lookup {
        ($name:ident,$static:expr) => {
            pub unsafe extern "C" fn $name(_env: *mut c_void, class: *mut c_void, name: *const c_char, signature: *const c_char) -> *mut c_void {
                let Some(s) = super::super::session() else { return std::ptr::null_mut(); };
                if name.is_null() || signature.is_null() { s.set_pending("java/lang/NoSuchFieldError", "null field metadata"); return std::ptr::null_mut(); }
                s.resolve_field(class, &CStr::from_ptr(name).to_string_lossy(), &CStr::from_ptr(signature).to_string_lossy(), $static)
            }
        }
    }
    lookup!(get_field_id,false); lookup!(get_static_field_id,true);
    macro_rules! scalar {
        ($get:ident,$set:ident,$static:expr,$variant:ident,$ty:ty,$zero:expr) => {
            pub extern "C" fn $get(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void) -> $ty {
                super::super::session().and_then(|s| s.get_field(receiver, field, FieldValue::$variant($zero), $static))
                    .and_then(|v| if let FieldValue::$variant(value) = v { Some(value) } else { None }).unwrap_or($zero)
            }
            pub extern "C" fn $set(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void, value: $ty) {
                if let Some(s) = super::super::session() { s.set_field(receiver, field, FieldValue::$variant(value), $static); }
            }
        }
    }
    scalar!(get_boolean_field,set_boolean_field,false,Boolean,u8,0);
    scalar!(get_byte_field,set_byte_field,false,Byte,i8,0);
    scalar!(get_char_field,set_char_field,false,Char,u16,0);
    scalar!(get_short_field,set_short_field,false,Short,i16,0);
    scalar!(get_int_field,set_int_field,false,Int,i32,0);
    scalar!(get_long_field,set_long_field,false,Long,i64,0);
    scalar!(get_float_field,set_float_field,false,Float,f32,0.0);
    scalar!(get_double_field,set_double_field,false,Double,f64,0.0);
    pub extern "C" fn get_object_field(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void) -> *mut c_void {
        let Some(s) = super::super::session() else { return std::ptr::null_mut(); };
        let Some(FieldValue::Object(Some(target))) = s.get_field(receiver,field,FieldValue::Object(None),false) else { return std::ptr::null_mut(); };
        let result = s.new_ref(target,false); s.release_target(target); result
    }
    pub extern "C" fn set_object_field(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void, value: *mut c_void) {
        let Some(s) = super::super::session() else { return; };
        let root = if value.is_null() { None } else { s.root_reference(value) };
        let target = if value.is_null() { None } else {
            let Some(root) = &root else { s.set_pending("java/lang/IllegalArgumentException","invalid object field value"); return; }; Some(root.target)
        };
        s.set_field(receiver,field,FieldValue::Object(target),false);
    }
    scalar!(get_static_boolean_field,set_static_boolean_field,true,Boolean,u8,0);
    scalar!(get_static_byte_field,set_static_byte_field,true,Byte,i8,0);
    scalar!(get_static_char_field,set_static_char_field,true,Char,u16,0);
    scalar!(get_static_short_field,set_static_short_field,true,Short,i16,0);
    scalar!(get_static_int_field,set_static_int_field,true,Int,i32,0);
    scalar!(get_static_long_field,set_static_long_field,true,Long,i64,0);
    scalar!(get_static_float_field,set_static_float_field,true,Float,f32,0.0);
    scalar!(get_static_double_field,set_static_double_field,true,Double,f64,0.0);
    pub extern "C" fn get_static_object_field(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void) -> *mut c_void {
        let Some(s) = super::super::session() else { return std::ptr::null_mut(); };
        let Some(FieldValue::Object(Some(target))) = s.get_field(receiver,field,FieldValue::Object(None),true) else { return std::ptr::null_mut(); };
        let result = s.new_ref(target,false); s.release_target(target); result
    }
    pub extern "C" fn set_static_object_field(_env: *mut c_void, receiver: *mut c_void, field: *mut c_void, value: *mut c_void) {
        let Some(s) = super::super::session() else { return; };
        let root = if value.is_null() { None } else { s.root_reference(value) };
        let target = if value.is_null() { None } else {
            let Some(root) = &root else { s.set_pending("java/lang/IllegalArgumentException","invalid object field value"); return; }; Some(root.target)
        };
        s.set_field(receiver,field,FieldValue::Object(target),true);
    }
}
