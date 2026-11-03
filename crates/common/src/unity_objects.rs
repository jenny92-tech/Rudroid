//! INPUT:  typed IL2CPP metadata, exact GC-handle ABI evidence and an attached player thread
//! OUTPUT:  checked Behaviour/static Sprite pruning and named GameObject deactivation with local GC roots
//! POS:    shared native managed-object boundary; no array offsets or cross-frame object pointers
#![deny(unsafe_op_in_unsafe_fn)]

use crate::il2cpp::{Class, GcHandles, GcRoot, Il2cpp, Method, TYPE_BOOLEAN, TYPE_VOID};
use std::ffi::{c_void, CStr};
use std::marker::PhantomData;
use std::thread::ThreadId;

/// Only immutable metadata survives between calls. All roots are local to `disable`.
#[derive(Clone)]
pub struct BehaviourPruner {
    api: Il2cpp,
    roots: GcHandles,
    find_all: Method,
    length: Method,
    get_value: Method,
    set_enabled: Method,
    behaviour: Class,
    array: Class,
    int32: Class,
    owner: ThreadId,
    _thread: PhantomData<*const ()>,
}

impl BehaviourPruner {
    pub fn discover(api: Il2cpp) -> Result<Self, &'static str> {
        if !api.thread_attached() { return Err("player thread not attached"); }
        let application = api.find_class(c"UnityEngine", c"Application").ok_or("Application unavailable")?;
        let roots = api.verified_gc_handles(application)?;
        let resources = api.find_class(c"UnityEngine", c"Resources").ok_or("Resources unavailable")?;
        let behaviour = api.find_class(c"UnityEngine", c"Behaviour").ok_or("Behaviour unavailable")?;
        let array = api.find_class(c"System", c"Array").ok_or("Array unavailable")?;
        let int32 = api.find_class(c"System", c"Int32").ok_or("Int32 unavailable")?;
        let type_ = api.find_class(c"System", c"Type").ok_or("Type unavailable")?;
        let find_all = api.matching_method(resources, c"FindObjectsOfTypeAll", true, &[(0x12,false)], 0x1d)
            .filter(|m| api.method_param(*m,0).and_then(|ty| api.class_from_type(ty)) == Some(type_))
            .ok_or("FindObjectsOfTypeAll(Type) signature unavailable")?;
        let length = api.matching_method(array, c"get_Length", false, &[], 8).ok_or("Array.Length signature unavailable")?;
        let get_value = api.matching_method(array, c"GetValue", false, &[(8,false)], 0x1c).ok_or("Array.GetValue(Int32) signature unavailable")?;
        let set_enabled = api.matching_method(behaviour, c"set_enabled", false, &[(TYPE_BOOLEAN,false)], TYPE_VOID)
            .ok_or("Behaviour.enabled signature unavailable")?;
        Ok(Self { api, roots, find_all, length, get_value, set_enabled, behaviour, array, int32,
            owner: std::thread::current().id(), _thread: PhantomData })
    }

    fn running(&self, cancelled: &impl Fn() -> bool) -> Result<(), &'static str> {
        if cancelled() { return Err("cancelled"); }
        if self.owner != std::thread::current().id() || !self.api.thread_attached() {
            return Err("player thread changed or detached");
        }
        Ok(())
    }

    // Private: only live returns from checked managed/runtime calls reach this helper.
    fn root(&self, object: *mut c_void) -> Result<GcRoot, &'static str> {
        if object.is_null() { return Err("managed call returned null"); }
        // SAFETY: caller inside this module supplies the live return before another managed call.
        unsafe { self.roots.root(object) }.ok_or("GC root creation failed")
    }

    fn target(&self, root: &GcRoot) -> Result<*mut c_void, &'static str> {
        let value = root.target();
        if value.is_null() { Err("GC root target lost") } else { Ok(value) }
    }

    /// Enumerate through System.Array methods and disable the chosen Behaviour subclass.
    /// No reference is retained after returning, including cancellation or managed failure.
    pub fn disable(&self, class: Class, cancelled: impl Fn() -> bool) -> Result<usize, &'static str> {
        self.running(&cancelled)?;
        if !self.api.derives_from(class, self.behaviour) { return Err("target is not a Behaviour subclass"); }
        let type_root = self.reflection_type(class, &cancelled)?;
        let mut find_args = [self.target(&type_root)?];
        // SAFETY: validated static Object[](System.Type), strong root retained through invocation.
        let objects = unsafe { self.api.invoke(self.find_all, std::ptr::null_mut(), find_args.as_mut_ptr()) }
            .map_err(|()| "object enumeration threw")?;
        let objects = self.array_root(objects, &cancelled)?;
        let count = self.array_length(&objects, &cancelled)?;
        let mut disabled = 0;
        for index in 0..count {
            let Some(object) = self.array_element(&objects, index, &cancelled)? else { continue; };
            self.verify_class(&object, class)?;
            self.disable_object(&object, self.set_enabled, &cancelled)?;
            disabled += 1;
        }
        Ok(disabled)
    }

    fn reflection_type(&self, class: Class, cancelled: &impl Fn()->bool) -> Result<GcRoot, &'static str> {
        self.running(cancelled)?;
        let ty = self.api.class_type(class).ok_or("target type unavailable")?;
        // SAFETY: checked same-module metadata on the owning attached thread.
        let type_object = unsafe { self.api.type_object(ty) }.ok_or("reflection type unavailable")?;
        self.running(cancelled)?;
        self.root(type_object)
    }

    fn class_of(&self, object: &GcRoot) -> Result<Class, &'static str> {
        // SAFETY: private roots are live objects from this module; caller keeps the root through this call.
        unsafe { self.api.object_class(self.target(object)?) }.ok_or("rooted object class unavailable")
    }
    fn verify_class(&self, object: &GcRoot, class: Class) -> Result<(), &'static str> {
        if self.api.derives_from(self.class_of(object)?, class) { Ok(()) }
        else { Err("enumeration element has the wrong class") }
    }

    fn array_root(&self, objects: *mut c_void, cancelled: &impl Fn()->bool) -> Result<GcRoot, &'static str> {
        self.running(cancelled)?;
        let objects = self.root(objects)?;
        if !self.api.derives_from(self.class_of(&objects)?, self.array) {
            return Err("enumeration returned a non-array");
        }
        Ok(objects)
    }

    fn array_length(&self, objects: &GcRoot, cancelled: &impl Fn()->bool) -> Result<i32, &'static str> {
        self.running(cancelled)?;
        // SAFETY: validated Array.Length, live Array receiver retained by objects root.
        let length = unsafe { self.api.invoke(self.length, self.target(objects)?, std::ptr::null_mut()) }.map_err(|()| "array length threw")?;
        self.running(cancelled)?;
        let length = self.root(length)?;
        let boxed = self.target(&length)?;
        if self.class_of(&length)? != self.int32 { return Err("array length is not Int32"); }
        // SAFETY: verified boxed Int32 retained for the complete unbox/read, native aligned i32 ABI.
        unsafe { self.api.unbox(boxed).filter(|p| !p.is_null()).map(|p| p.cast::<i32>().read()) }
            .filter(|n| (0..=1_000_000).contains(n)).ok_or("invalid array length")
    }

    fn array_element(&self, objects: &GcRoot, mut index: i32, cancelled: &impl Fn()->bool) -> Result<Option<GcRoot>, &'static str> {
        self.running(cancelled)?;
        let mut args=[(&mut index as *mut i32).cast()];
        // SAFETY: checked Array.GetValue(Int32), rooted array and initialized native index; runtime bounds-checks it.
        let value=unsafe {self.api.invoke(self.get_value,self.target(objects)?,args.as_mut_ptr())}.map_err(|()| "array element getter threw")?;
        self.running(cancelled)?;
        if value.is_null() { Ok(None) } else { self.root(value).map(Some) }
    }

    fn disable_object(&self, object: &GcRoot, method: Method, cancelled: &impl Fn()->bool) -> Result<(), &'static str> {
        self.running(cancelled)?;
        let mut enabled=false;let mut args=[(&mut enabled as *mut bool).cast()];
        // SAFETY: private callers checked the instance void(bool) setter and receiver class; root covers invocation.
        unsafe {self.api.invoke(method,self.target(object)?,args.as_mut_ptr())}.map_err(|()| "component disabling threw")?;
        self.running(cancelled)
    }
}

/// Deactivate active named GameObjects through checked Unity methods. Metadata
/// is immutable; String and GameObject roots live only through each invocation.
pub struct NamedGameObjectPruner {
    core: BehaviourPruner,
    game_object: Class,
    string: Class,
    find: Method,
    set_active: Method,
}

impl NamedGameObjectPruner {
    pub fn discover(api: Il2cpp) -> Result<Self, &'static str> {
        let core = BehaviourPruner::discover(api)?;
        let game_object = api.find_class(c"UnityEngine", c"GameObject").ok_or("GameObject unavailable")?;
        let string = api.find_class(c"System", c"String").ok_or("String unavailable")?;
        if !api.has("il2cpp_string_new") { return Err("String creation unavailable"); }
        let find = api.matching_method(game_object, c"Find", true, &[(crate::il2cpp::TYPE_STRING, false)], 0x12)
            .filter(|m| api.method_return_type(*m).and_then(|ty| api.class_from_type(ty)) == Some(game_object))
            .ok_or("GameObject.Find(String) signature unavailable")?;
        let set_active = api.matching_method(game_object, c"SetActive", false, &[(TYPE_BOOLEAN, false)], TYPE_VOID)
            .ok_or("GameObject.SetActive(Boolean) signature unavailable")?;
        Ok(Self { core, game_object, string, find, set_active })
    }

    /// Missing/inactive names are skipped. This never destroys an object or
    /// unloads resources. Cancellation/failure releases every temporary root.
    pub fn disable(&self, names: &[&CStr], cancelled: impl Fn() -> bool) -> Result<usize, &'static str> {
        self.core.running(&cancelled)?;
        let mut disabled = 0;
        for &name in names {
            self.core.running(&cancelled)?;
            if name.to_str().is_err() { return Err("GameObject name is not UTF-8"); }
            // SAFETY: checked owning attached thread; CStr storage spans the
            // call and the returned managed String is rooted immediately.
            let text = unsafe { self.core.api.new_string(name) }.ok_or("name String allocation failed")?;
            let text = self.core.root(text)?;
            self.core.running(&cancelled)?;
            self.core.verify_class(&text, self.string)?;
            let mut args = [self.core.target(&text)?];
            // SAFETY: verified static GameObject(String); rooted String
            // argument covers the call and any callbacks/reentrant collection.
            let object = unsafe { self.core.api.invoke(self.find, std::ptr::null_mut(), args.as_mut_ptr()) }
                .map_err(|()| "GameObject.Find threw")?;
            // Root before running the caller's cancellation predicate: that
            // predicate may itself allocate or enter managed code.
            let object = if object.is_null() { None } else { Some(self.core.root(object)?) };
            self.core.running(&cancelled)?;
            let Some(object) = object else { continue; };
            self.core.verify_class(&object, self.game_object)?;
            self.core.disable_object(&object, self.set_active, &cancelled)?;
            disabled += 1;
        }
        Ok(disabled)
    }
}

/// Sprite filtering changes Renderer.enabled only. GameObjects and their resources stay owned by Unity.
pub struct StaticSpritePruner {
    core: BehaviourPruner,
    sprite: Class,
    transform: Class,
    game_object: Class,
    string: Class,
    component: Class,
    find_active: Method,
    get_game_object: Method,
    get_components: Method,
    get_name: Method,
    set_enabled: Method,
}

impl StaticSpritePruner {
    pub fn discover(api: Il2cpp) -> Result<Self, &'static str> {
        Self::from_behaviour(BehaviourPruner::discover(api)?)
    }

    pub fn from_behaviour(core: BehaviourPruner) -> Result<Self, &'static str> {
        let api=core.api;
        let object=api.find_class(c"UnityEngine",c"Object").ok_or("Object unavailable")?;
        let component=api.find_class(c"UnityEngine",c"Component").ok_or("Component unavailable")?;
        let sprite=api.find_class(c"UnityEngine",c"SpriteRenderer").ok_or("SpriteRenderer unavailable")?;
        let transform=api.find_class(c"UnityEngine",c"Transform").ok_or("Transform unavailable")?;
        let game_object=api.find_class(c"UnityEngine",c"GameObject").ok_or("GameObject unavailable")?;
        let renderer=api.find_class(c"UnityEngine",c"Renderer").ok_or("Renderer unavailable")?;
        let string=api.find_class(c"System",c"String").ok_or("String unavailable")?;
        let type_=api.find_class(c"System",c"Type").ok_or("Type unavailable")?;
        let find_active=api.matching_method(object,c"FindObjectsOfType",true,&[(0x12,false)],0x1d)
            .filter(|m|api.method_param(*m,0).and_then(|ty|api.class_from_type(ty))==Some(type_)).ok_or("active Sprite enumeration signature unavailable")?;
        let get_game_object=api.matching_method(component,c"get_gameObject",false,&[],0x12)
            .filter(|m|api.method_return_type(*m).and_then(|ty|api.class_from_type(ty))==Some(game_object)).ok_or("Component.gameObject signature unavailable")?;
        let get_components=api.matching_method(game_object,c"GetComponents",false,&[(0x12,false)],0x1d)
            .filter(|m|api.method_param(*m,0).and_then(|ty|api.class_from_type(ty))==Some(type_)).ok_or("GameObject.GetComponents(Type) signature unavailable")?;
        let get_name=api.matching_method(object,c"get_name",false,&[],crate::il2cpp::TYPE_STRING).ok_or("Object.name signature unavailable")?;
        let set_enabled=api.matching_method(renderer,c"set_enabled",false,&[(TYPE_BOOLEAN,false)],TYPE_VOID).ok_or("Renderer.enabled signature unavailable")?;
        if !api.derives_from(sprite,renderer) || !api.derives_from(sprite,component) || !api.derives_from(transform,component) {
            return Err("Sprite/Transform hierarchy unavailable");
        }
        Ok(Self {core,sprite,transform,game_object,string,component,find_active,get_game_object,get_components,get_name,set_enabled})
    }

    /// Keep the first `percent` entries of each 100 matched objects, as the C policy does.
    /// The predicate sees owned UTF-8 text. Objects must contain exactly the same Renderer and one Transform.
    pub fn prune(&self, percent: u32, predicate: impl Fn(&str)->bool, cancelled: impl Fn()->bool) -> Result<(usize,usize), &'static str> {
        if !(25..=100).contains(&percent) { return Err("invalid Sprite percentage"); }
        self.core.running(&cancelled)?;
        if percent==100 { return Ok((0,0)); }
        let type_root=self.core.reflection_type(self.sprite,&cancelled)?;
        let component_type=self.core.reflection_type(self.component,&cancelled)?;
        let mut args=[self.core.target(&type_root)?];
        // SAFETY: validated static Object[](Type); rooted reflection argument identifies SpriteRenderer.
        let array=unsafe {self.core.api.invoke(self.find_active,std::ptr::null_mut(),args.as_mut_ptr())}.map_err(|()| "active Sprite enumeration threw")?;
        let array=self.core.array_root(array,&cancelled)?;
        let count=self.core.array_length(&array,&cancelled)?;
        let (mut matched,mut disabled)=(0usize,0usize);
        for index in 0..count {
            let Some(sprite)=self.core.array_element(&array,index,&cancelled)? else { continue; };
            self.core.verify_class(&sprite,self.sprite)?;
            // SAFETY: checked Component.gameObject getter on the rooted SpriteRenderer subclass.
            let object=unsafe {self.core.api.invoke(self.get_game_object,self.core.target(&sprite)?,std::ptr::null_mut())}.map_err(|()| "Sprite GameObject getter threw")?;
            self.core.running(&cancelled)?;let object=self.core.root(object)?;self.core.verify_class(&object,self.game_object)?;
            let mut args=[self.core.target(&component_type)?];
            // SAFETY: checked GameObject.GetComponents(Type), rooted GameObject and Component reflection type.
            let components=unsafe {self.core.api.invoke(self.get_components,self.core.target(&object)?,args.as_mut_ptr())}.map_err(|()| "component enumeration threw")?;
            let components=self.core.array_root(components,&cancelled)?;
            if self.core.array_length(&components,&cancelled)?!=2 { continue; }
            let (mut has_transform,mut has_sprite)=(false,false);
            for slot in 0..2 {
                let Some(item)=self.core.array_element(&components,slot,&cancelled)? else { continue; };
                let class=self.core.class_of(&item)?;
                has_transform|=self.core.api.derives_from(class,self.transform);
                has_sprite|=self.core.api.derives_from(class,self.sprite) && self.core.target(&item)?==self.core.target(&sprite)?;
            }
            if !has_transform || !has_sprite { continue; }
            self.core.running(&cancelled)?;
            // SAFETY: checked Object.name getter on the rooted GameObject.
            let name=unsafe {self.core.api.invoke(self.get_name,self.core.target(&object)?,std::ptr::null_mut())}.map_err(|()| "object name getter threw")?;
            self.core.running(&cancelled)?;if name.is_null() { continue; }let name=self.core.root(name)?;
            self.core.verify_class(&name,self.string)?;
            // SAFETY: actual System.String checked; strong root retained through bounded character copy.
            let units=unsafe {self.core.api.string_utf16(self.core.target(&name)?,1024)};
            let Some(text)=units.and_then(|v|String::from_utf16(&v).ok()) else { continue; };
            if !predicate(&text) { continue; }
            let keep=matched%100 < percent as usize;matched+=1;
            if !keep {self.core.disable_object(&sprite,self.set_enabled,&cancelled)?;disabled+=1;}
        }
        self.core.running(&cancelled)?;
        Ok((matched,disabled))
    }
}
