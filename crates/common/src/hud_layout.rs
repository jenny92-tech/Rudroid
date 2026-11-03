//! INPUT:  checked HUD/Unity metadata, legacy field selectors and callback-local strong roots
//! OUTPUT:  rooted control hiding, pivot copying and screen-position/scale updates
//! POS:    common managed-object boundary for configurable handheld HUD layouts
#![deny(unsafe_op_in_unsafe_fn)]
use crate::il2cpp::{Class, Field, GcHandles, GcRoot, Il2cpp, Method};
use std::{
    ffi::{c_void, CStr},
    thread::ThreadId,
};
type Ptr = *mut c_void;

pub struct HudSpec<'a> {
    pub assembly: &'a CStr,
    pub namespace: &'a CStr,
    pub class: &'a CStr,
    pub method: &'a CStr,
    pub argc: u32,
    pub hide: &'a [usize],
    pub move_from: Option<usize>,
    pub move_to: Option<usize>,
    pub dx: i32,
    pub dy: i32,
    pub right_margin: Option<i32>,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vector2 {
    x: f32,
    y: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vector3 {
    x: f32,
    y: f32,
    z: f32,
}

pub(crate) struct HudLayout {
    api: Il2cpp,
    roots: GcHandles,
    owner: ThreadId,
    component: Class,
    game_object: Class,
    transform: Class,
    rect_transform: Class,
    vector2: Class,
    vector3: Class,
    int32: Class,
    hidden: Vec<Field>,
    move_pair: Option<(Field, Field)>,
    dx: i32,
    dy: i32,
    right_margin: Option<i32>,
    get_object: Method,
    set_active: Method,
    get_transform: Method,
    get_pivot: Option<Method>,
    set_pivot: Option<Method>,
    get_position: Option<Method>,
    set_position: Option<Method>,
    set_scale: Method,
    width: Option<Method>,
}

impl HudLayout {
    pub(crate) fn discover(
        api: Il2cpp,
        roots: GcHandles,
        hud: Class,
        spec: &HudSpec<'_>,
    ) -> Result<Self, &'static str> {
        if spec.hide.len() > 16 {
            return Err("HUD hidden-control limit is 16");
        }
        if spec.move_from.is_some() != spec.move_to.is_some() {
            return Err("both HUD move fields are required");
        }
        let component = api
            .find_class(c"UnityEngine", c"Component")
            .ok_or("Component absent")?;
        let game_object = api
            .find_class(c"UnityEngine", c"GameObject")
            .ok_or("GameObject absent")?;
        let transform = api
            .find_class(c"UnityEngine", c"Transform")
            .ok_or("Transform absent")?;
        let rect_transform = api
            .find_class(c"UnityEngine", c"RectTransform")
            .ok_or("RectTransform absent")?;
        let vector2 = api
            .find_class(c"UnityEngine", c"Vector2")
            .ok_or("Vector2 absent")?;
        let vector3 = api
            .find_class(c"UnityEngine", c"Vector3")
            .ok_or("Vector3 absent")?;
        let int32 = api.find_class(c"System", c"Int32").ok_or("Int32 absent")?;
        let field = |offset: usize| {
            let f = api
                .field_at_offset(hud, offset)
                .ok_or("HUD offset is not a metadata field")?;
            let ty = api.field_type(f).ok_or("HUD field type absent")?;
            let class = api.class_from_type(ty).ok_or("HUD field class absent")?;
            if api.type_kind(ty) != Some(0x12)
                || api.type_byref(ty) != Some(false)
                || api.field_flags(f).is_none_or(|v| v & 0x10 != 0)
                || !api.derives_from(class, component)
                || !api
                    .field_parent(f)
                    .is_some_and(|parent| api.derives_from(hud, parent))
            {
                return Err("HUD field is not an instance Component reference");
            }
            Ok(f)
        };
        let hidden = spec
            .hide
            .iter()
            .map(|&offset| field(offset))
            .collect::<Result<Vec<_>, _>>()?;
        let move_pair = spec
            .move_from
            .zip(spec.move_to)
            .map(|(from, to)| Ok((field(from)?, field(to)?)))
            .transpose()?;
        let checked = |class, name, params: &[(i32, bool)], ret| {
            api.matching_method(class, name, false, params, ret)
                .ok_or("HUD method signature mismatch")
        };
        let get_object = checked(component, c"get_gameObject", &[], 0x12)?;
        let get_transform = checked(component, c"get_transform", &[], 0x12)?;
        if api
            .method_return_type(get_object)
            .and_then(|ty| api.class_from_type(ty))
            != Some(game_object)
            || api
                .method_return_type(get_transform)
                .and_then(|ty| api.class_from_type(ty))
                != Some(transform)
        {
            return Err("HUD getter reference class mismatch");
        }
        let set_active = checked(game_object, c"SetActive", &[(2, false)], 1)?;
        let set_scale = checked(transform, c"set_localScale", &[(0x11, false)], 1)?;
        if api
            .method_param(set_scale, 0)
            .and_then(|ty| api.class_from_type(ty))
            != Some(vector3)
        {
            return Err("localScale is not Vector3");
        }
        let (get_pivot, set_pivot, get_position, set_position, width) = if move_pair.is_some() {
            let gp = checked(rect_transform, c"get_pivot", &[], 0x11)?;
            let sp = checked(rect_transform, c"set_pivot", &[(0x11, false)], 1)?;
            let gpos = checked(transform, c"get_position", &[], 0x11)?;
            let spos = checked(transform, c"set_position", &[(0x11, false)], 1)?;
            if api
                .method_return_type(gp)
                .and_then(|ty| api.class_from_type(ty))
                != Some(vector2)
                || api
                    .method_param(sp, 0)
                    .and_then(|ty| api.class_from_type(ty))
                    != Some(vector2)
                || api
                    .method_return_type(gpos)
                    .and_then(|ty| api.class_from_type(ty))
                    != Some(vector3)
                || api
                    .method_param(spos, 0)
                    .and_then(|ty| api.class_from_type(ty))
                    != Some(vector3)
            {
                return Err("HUD vector method class mismatch");
            }
            let width = if spec.right_margin.is_some() {
                let screen = api
                    .find_class(c"UnityEngine", c"Screen")
                    .ok_or("Screen absent")?;
                Some(
                    api.matching_method(screen, c"get_width", true, &[], 8)
                        .ok_or("Screen.width signature mismatch")?,
                )
            } else {
                None
            };
            (Some(gp), Some(sp), Some(gpos), Some(spos), width)
        } else {
            (None, None, None, None, None)
        };
        if !api.has("il2cpp_field_get_value")
            || !api.has("il2cpp_runtime_invoke")
            || !api.has("il2cpp_object_unbox")
        {
            return Err("HUD runtime exports absent");
        }
        Ok(Self {
            api,
            roots,
            owner: std::thread::current().id(),
            component,
            game_object,
            transform,
            rect_transform,
            vector2,
            vector3,
            int32,
            hidden,
            move_pair,
            dx: spec.dx,
            dy: spec.dy,
            right_margin: spec.right_margin,
            get_object,
            set_active,
            get_transform,
            get_pivot,
            set_pivot,
            get_position,
            set_position,
            set_scale,
            width,
        })
    }
    fn running(&self) -> Result<(), &'static str> {
        if self.owner != std::thread::current().id()
            || !self.api.thread_attached()
            || !crate::managed_hooks::active()
        {
            Err("HUD owner changed/detached/stopped")
        } else {
            Ok(())
        }
    }
    fn root(&self, value: Ptr) -> Result<GcRoot, &'static str> {
        if value.is_null() {
            return Err("HUD getter returned null");
        }
        // SAFETY: private helper receives a live reference just returned by a verified managed/field call.
        unsafe { self.roots.root(value) }.ok_or("HUD root creation failed")
    }
    fn verify(&self, root: &GcRoot, want: Class) -> Result<(), &'static str> {
        // SAFETY: callback-local root pins a live instance while querying its class.
        let class =
            unsafe { self.api.object_class(root.target()) }.ok_or("HUD object class absent")?;
        if self.api.derives_from(class, want) {
            Ok(())
        } else {
            Err("HUD object has wrong class")
        }
    }
    fn field(&self, owner: &GcRoot, field: Field) -> Result<Option<GcRoot>, &'static str> {
        self.running()?;
        // SAFETY: selected field belongs to the checked HUD receiver, retained by its input root.
        let value = unsafe { self.api.reference_field(owner.target(), field) }
            .ok_or("HUD field read failed")?;
        if value.is_null() {
            Ok(None)
        } else {
            let value = self.root(value)?;
            self.verify(&value, self.component)?;
            Ok(Some(value))
        }
    }
    fn invoke(
        &self,
        method: Method,
        object: Option<&GcRoot>,
        args: &mut [Ptr],
    ) -> Result<Ptr, &'static str> {
        self.running()?;
        // SAFETY: every method/argument is privately signature checked; object and reference arguments stay rooted.
        let value = unsafe {
            self.api.invoke(
                method,
                object.map_or(std::ptr::null_mut(), GcRoot::target),
                args.as_mut_ptr(),
            )
        }
        .map_err(|_| "HUD managed call threw")?;
        self.running()?;
        Ok(value)
    }
    fn getter(
        &self,
        method: Method,
        object: &GcRoot,
        class: Class,
    ) -> Result<GcRoot, &'static str> {
        let value = self.invoke(method, Some(object), &mut [])?;
        let value = self.root(value)?;
        self.verify(&value, class)?;
        Ok(value)
    }
    fn vector2(&self, root: &GcRoot) -> Result<Vector2, &'static str> {
        self.verify(root, self.vector2)?;
        // SAFETY: checked boxed UnityEngine.Vector2 root with the exact native two-f32 value ABI.
        unsafe {
            self.api
                .unbox(root.target())
                .filter(|p| !p.is_null())
                .map(|p| p.cast::<Vector2>().read())
        }
        .ok_or("Vector2 unbox failed")
    }
    fn vector3(&self, root: &GcRoot) -> Result<Vector3, &'static str> {
        self.verify(root, self.vector3)?;
        // SAFETY: checked boxed UnityEngine.Vector3 root with the exact native three-f32 value ABI.
        unsafe {
            self.api
                .unbox(root.target())
                .filter(|p| !p.is_null())
                .map(|p| p.cast::<Vector3>().read())
        }
        .ok_or("Vector3 unbox failed")
    }
    pub(crate) fn apply(&self, owner: &GcRoot) -> Result<usize, &'static str> {
        self.running()?;
        if let Some((from, to)) = self.move_pair {
            if let (Some(source), Some(destination)) =
                (self.field(owner, from)?, self.field(owner, to)?)
            {
                let source = self.getter(self.get_transform, &source, self.transform)?;
                let destination = self.getter(self.get_transform, &destination, self.transform)?;
                self.verify(&source, self.rect_transform)?;
                self.verify(&destination, self.rect_transform)?;
                let pivot = self.getter(
                    self.get_pivot.ok_or("pivot getter absent")?,
                    &source,
                    self.vector2,
                )?;
                let mut pivot = self.vector2(&pivot)?;
                self.invoke(
                    self.set_pivot.ok_or("pivot setter absent")?,
                    Some(&destination),
                    &mut [(&mut pivot as *mut Vector2).cast()],
                )?;
                let position = self.getter(
                    self.get_position.ok_or("position getter absent")?,
                    &source,
                    self.vector3,
                )?;
                let mut position = self.vector3(&position)?;
                if let Some(margin) = self.right_margin {
                    let width =
                        self.invoke(self.width.ok_or("screen getter absent")?, None, &mut [])?;
                    let width = self.root(width)?;
                    self.verify(&width, self.int32)?;
                    // SAFETY: checked boxed Int32 retained through unbox/read.
                    let width = unsafe {
                        self.api
                            .unbox(width.target())
                            .filter(|p| !p.is_null())
                            .map(|p| p.cast::<i32>().read())
                    }
                    .ok_or("screen width unbox failed")?;
                    if width <= 0 {
                        return Err("invalid screen width");
                    }
                    position.x = width as f32 - margin as f32;
                } else {
                    position.x += self.dx as f32;
                }
                position.y += self.dy as f32;
                self.invoke(
                    self.set_position.ok_or("position setter absent")?,
                    Some(&destination),
                    &mut [(&mut position as *mut Vector3).cast()],
                )?;
            }
        }
        let mut count = 0;
        for &field in &self.hidden {
            let Some(component) = self.field(owner, field)? else {
                continue;
            };
            let object = self.getter(self.get_object, &component, self.game_object)?;
            let mut active = false;
            self.invoke(
                self.set_active,
                Some(&object),
                &mut [(&mut active as *mut bool).cast()],
            )?;
            let transform = self.getter(self.get_transform, &component, self.transform)?;
            let mut zero = Vector3::default();
            self.invoke(
                self.set_scale,
                Some(&transform),
                &mut [(&mut zero as *mut Vector3).cast()],
            )?;
            count += 1;
        }
        Ok(count)
    }
}
