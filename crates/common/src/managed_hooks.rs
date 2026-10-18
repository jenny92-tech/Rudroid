//! INPUT:  immutable IL2CPP metadata, exact Unity ABI evidence and quiescent target entries
//! OUTPUT:  validated config patch plans and process-pinned scalar/reference ARM64 detours
//! POS:    centralizes native calls, field access and callback-local GC ownership for configuration patches
#![deny(unsafe_op_in_unsafe_fn)]

use crate::host_api::HostApiV0;
use crate::il2cpp::{Class, Field, GcHandles, GcRoot, Il2cpp, Method, NumericValue};
use std::ffi::{c_void, CStr};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex, OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchMode {
    Scale,
    Freeze,
    ForceReturn,
    Trace,
    Bypass,
}

/// Numeric offsets are matched against metadata before a plan exists.
pub struct PatchSpec<'a> {
    pub assembly: &'a CStr,
    pub namespace: &'a CStr,
    pub class: &'a CStr,
    pub method: &'a CStr,
    pub argc: u32,
    pub object_register: usize,
    pub field_offset: Option<usize>,
    pub source_offset: Option<usize>,
    pub multiplier: f64,
    pub value: f64,
    pub integer: bool,
    pub mode: PatchMode,
}

/// A native register packet; assembly initializes all bytes and has no stack arguments.
#[repr(C, align(16))]
struct Frame {
    x: [u64; 10],
    q: [u8; 128],
    result_x: [u64; 2],
    result_q: [u8; 32],
}

/// Immutable metadata plus bounded diagnostic accounting. No managed object
/// survives a dispatch; the counter never retains or addresses guest memory.
pub struct ManagedHookPlan {
    api: Il2cpp,
    roots: GcHandles,
    method: Method,
    entry: usize,
    references: [Option<Class>; 8],
    result_reference: bool,
    method_register: usize,
    object_register: usize,
    action: Action,
    // Success/failure evidence is bounded independently for each installed
    // plan. An accidentally frequent HUD hook must not flood synchronous I/O.
    reports: AtomicUsize,
}
enum Action {
    Scale {
        field: Field,
        multiplier: f32,
    },
    Freeze {
        field: Field,
        source: Option<Field>,
        value: NumericValue,
    },
    ForceReturn {
        integer: u64,
        floating: u64,
    },
    Trace,
    Bypass,
    Hud(Box<crate::hud_layout::HudLayout>),
    Touch(Box<crate::touch_controls::TouchControls>),
}

fn reference(kind: i32) -> bool {
    matches!(kind, 0x0e | 0x12 | 0x14 | 0x1c | 0x1d)
}
fn scalar(kind: i32) -> bool {
    matches!(kind, 2..=13 | 0x18 | 0x19)
}

impl ManagedHookPlan {
    pub fn touch_controls(api: Il2cpp) -> Result<Self, &'static str> {
        let class = api
            .find_class(c"InControl", c"TouchManager")
            .ok_or("InControl.TouchManager unavailable")?;
        let method = api
            .unique_method(class, c"set_controlsEnabled", 1)
            .ok_or("TouchManager setter absent/ambiguous")?;
        if !api.method_signature(method, false, &[(2, false)], 1) {
            return Err("TouchManager setter signature mismatch");
        }
        let mut plan = Self::signature(api, class, method)?;
        plan.action = Action::Touch(Box::new(crate::touch_controls::TouchControls::discover(
            api, plan.roots, class,
        )?));
        Ok(plan)
    }
    pub fn hud(api: Il2cpp, spec: &crate::hud_layout::HudSpec<'_>) -> Result<Self, &'static str> {
        let image = api
            .exact_image(spec.assembly.to_str().map_err(|_| "invalid HUD assembly")?)
            .ok_or("HUD assembly unavailable")?;
        let class = api
            .class_from_name(image, spec.namespace, spec.class)
            .ok_or("HUD class unavailable")?;
        let method = api
            .unique_method(class, spec.method, spec.argc)
            .ok_or("HUD method absent or overload ambiguous")?;
        if api.method_flags(method).is_none_or(|f| f & 0x10 != 0) {
            return Err("HUD method must have an instance receiver");
        }
        let mut plan = Self::signature(api, class, method)?;
        plan.action = Action::Hud(Box::new(crate::hud_layout::HudLayout::discover(
            api, plan.roots, class, spec,
        )?));
        Ok(plan)
    }
    pub fn patch(api: Il2cpp, spec: &PatchSpec<'_>) -> Result<Self, &'static str> {
        if !api.thread_attached() {
            return Err("IL2CPP thread not attached");
        }
        let image = api
            .exact_image(spec.assembly.to_str().map_err(|_| "invalid assembly")?)
            .ok_or("assembly unavailable")?;
        let class = api
            .class_from_name(image, spec.namespace, spec.class)
            .ok_or("class unavailable")?;
        let method = api
            .unique_method(class, spec.method, spec.argc)
            .ok_or("method absent or overload ambiguous")?;
        let mut plan = Self::signature(api, class, method)?;
        plan.object_register = spec.object_register;
        let object = plan.references.get(spec.object_register).copied().flatten();
        let numeric =
            |offset: Option<usize>, integer: bool, writable: bool| -> Result<Field, &'static str> {
                let class = object.ok_or("selected argument is not a managed reference")?;
                let field = api
                    .field_at_offset(class, offset.ok_or("field offset absent")?)
                    .ok_or("offset does not identify a unique metadata field")?;
                let ty = api.field_type(field).ok_or("field type absent")?;
                if api.type_byref(ty) != Some(false)
                    || api.type_kind(ty) != Some(if integer { 8 } else { 12 })
                    || api
                        .field_flags(field)
                        .is_none_or(|f| f & if writable { 0x70 } else { 0x10 } != 0)
                    || !api
                        .field_parent(field)
                        .is_some_and(|parent| api.derives_from(class, parent))
                {
                    return Err("field type/staticness/mutability/declaring class mismatch");
                }
                if !api.has("il2cpp_field_get_value")
                    || (writable && !api.has("il2cpp_field_set_value"))
                {
                    return Err("field access exports unavailable");
                }
                Ok(field)
            };
        plan.action = match spec.mode {
            PatchMode::Scale => {
                if !spec.multiplier.is_finite() || !(spec.multiplier as f32).is_finite() {
                    return Err("invalid scale multiplier");
                }
                Action::Scale {
                    field: numeric(spec.field_offset, false, true)?,
                    multiplier: spec.multiplier as f32,
                }
            }
            PatchMode::Freeze => {
                let field = numeric(spec.field_offset, spec.integer, true)?;
                let source = spec
                    .source_offset
                    .map(|offset| numeric(Some(offset), spec.integer, false))
                    .transpose()?;
                let value = if spec.integer {
                    if !spec.value.is_finite()
                        || spec.value < i32::MIN as f64
                        || spec.value > i32::MAX as f64
                    {
                        return Err("i32 freeze value out of range");
                    }
                    NumericValue::I32(spec.value as i32)
                } else {
                    if !spec.value.is_finite() || !(spec.value as f32).is_finite() {
                        return Err("f32 freeze value out of range");
                    }
                    NumericValue::F32(spec.value as f32)
                };
                Action::Freeze {
                    field,
                    source,
                    value,
                }
            }
            PatchMode::ForceReturn => {
                let kind = api
                    .type_kind(api.method_return_type(method).ok_or("return type absent")?)
                    .ok_or("return kind absent")?;
                let (integer, floating) = encode_return(kind, spec.value)?;
                Action::ForceReturn { integer, floating }
            }
            PatchMode::Trace => Action::Trace,
            PatchMode::Bypass => Action::Bypass,
        };
        Ok(plan)
    }

    fn signature(api: Il2cpp, class: Class, method: Method) -> Result<Self, &'static str> {
        if !api.ordinary_method(method) {
            return Err("generic/shared or unverified method metadata");
        }
        let argc = api
            .method_param_count(method)
            .filter(|n| *n <= 6)
            .ok_or("unsupported method arity")?;
        let static_ = api.method_flags(method).ok_or("method flags absent")? & 0x10 != 0;
        let mut references = [None; 8];
        let mut integer = if static_ {
            0
        } else {
            references[0] = Some(class);
            1
        };
        let mut floating = 0;
        for index in 0..argc {
            let ty = api
                .method_param(method, index)
                .ok_or("parameter metadata absent")?;
            let kind = api.type_kind(ty).ok_or("parameter kind absent")?;
            if api.type_byref(ty) != Some(false) {
                return Err("byref parameters are unsupported");
            }
            if matches!(kind, 12 | 13) {
                floating += 1;
            } else if reference(kind) {
                references[integer] = Some(
                    api.class_from_type(ty)
                        .ok_or("reference parameter class absent")?,
                );
                integer += 1;
            } else if scalar(kind) {
                integer += 1;
            } else {
                return Err("value-type/pointer/generic parameters are unsupported");
            }
        }
        if integer >= 8 || floating > 8 {
            return Err("stack parameters are unsupported");
        }
        let ret = api
            .method_return_type(method)
            .ok_or("return metadata absent")?;
        let kind = api.type_kind(ret).ok_or("return kind absent")?;
        if api.type_byref(ret) != Some(false) || !(kind == 1 || scalar(kind) || reference(kind)) {
            return Err("byref/value-type/pointer/generic return is unsupported");
        }
        let app = api
            .find_class(c"UnityEngine", c"Application")
            .ok_or("Application unavailable")?;
        let roots = api.verified_gc_handles(app)?;
        let entry = api.method_entry(method).ok_or("method entry absent")?;
        Ok(Self {
            api,
            roots,
            method,
            entry,
            reports: AtomicUsize::new(0),
            references,
            result_reference: reference(kind),
            method_register: integer,
            object_register: 0,
            action: Action::Trace,
        })
    }

    /// Address used only to detect conflicting hooks; code access remains private.
    pub fn target(&self) -> usize {
        self.entry
    }

    fn inputs(&self, frame: &mut Frame) -> Result<[Option<GcRoot>; 8], &'static str> {
        if !self.api.thread_attached() {
            return Err("thread detached");
        }
        let metadata = frame.x[self.method_register] as usize;
        if metadata != 0 && metadata != self.method.as_ptr() as usize {
            return Err("shared entry MethodInfo mismatch");
        }
        let mut roots: [Option<GcRoot>; 8] = std::array::from_fn(|_| None);
        for (index, class) in self.references.iter().enumerate() {
            let Some(class) = class else {
                continue;
            };
            let object = frame.x[index] as *mut c_void;
            if object.is_null() {
                continue;
            }
            // SAFETY: exact reference register from the checked native method's live input.
            let root = unsafe { self.roots.root(object) }.ok_or("input root creation failed")?;
            // SAFETY: root retains the receiver during the metadata query.
            let actual = unsafe { self.api.object_class(root.target()) }
                .ok_or("input object class absent")?;
            if !self.api.derives_from(actual, *class) {
                return Err("input object class mismatch");
            }
            frame.x[index] = root.target() as u64;
            roots[index] = Some(root);
        }
        for (index, root) in roots.iter().enumerate() {
            if let Some(root) = root {
                frame.x[index] = root.target() as u64;
            }
        }
        Ok(roots)
    }

    fn numeric(&self, object: *mut c_void, field: Field) -> Option<NumericValue> {
        // SAFETY: dispatch supplies only the selected, type-checked callback-local root.
        unsafe { self.api.numeric_field(object, field) }
    }
    fn write(&self, object: *mut c_void, field: Field, value: NumericValue) {
        // SAFETY: metadata plan verified instance type and mutability; dispatch retains owner.
        unsafe { self.api.set_numeric_field(object, field, value) };
    }

    fn run(&self, original: usize, host: &HostApiV0, frame: &mut Frame) {
        if !ACTIVE.load(Ordering::Acquire) && matches!(self.action, Action::Touch(_)) {
            return;
        }
        let inputs = if ACTIVE.load(Ordering::Acquire) {
            self.inputs(frame).ok()
        } else {
            None
        };
        if inputs.is_none() {
            call_original(original, frame);
            return;
        }
        let inputs = inputs.expect("checked input owner");
        let object = || {
            inputs
                .get(self.object_register)
                .and_then(|r| r.as_ref())
                .map_or(std::ptr::null_mut(), GcRoot::target)
        };
        if let Action::Touch(ref controls) = self.action {
            if let Some(owner) = inputs[0].as_ref() {
                match controls.before(owner, frame.x[1] & 0xff != 0) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(reason) => {
                        self.report(host, || format!("TouchManager fallback stopped: {reason}"));
                        return;
                    }
                }
            }
        }
        if let Action::Scale { field, multiplier } = self.action {
            if let Some(NumericValue::F32(value)) = self.numeric(object(), field) {
                self.write(object(), field, NumericValue::F32(value * multiplier));
            }
        }
        if matches!(self.action, Action::Trace) {
            host.log_line(c"IL2PATCH", "configured method enter");
        }
        if matches!(self.action, Action::Bypass) {
            frame.result_x = [0; 2];
            frame.result_q = [0; 32];
            return;
        }
        call_original(original, frame);
        // Root the original reference result before post-hook managed calls can trigger GC.
        let result = if self.result_reference && frame.result_x[0] != 0 {
            // SAFETY: just-returned live managed reference from the checked method ABI.
            unsafe { self.roots.root(frame.result_x[0] as *mut c_void) }
        } else {
            None
        };
        if self.result_reference && frame.result_x[0] != 0 && result.is_none() {
            return;
        }
        if ACTIVE.load(Ordering::Acquire) {
            match self.action {
                Action::Freeze {
                    field,
                    source,
                    value,
                } => {
                    let value = source.map_or(Some(value), |source| self.numeric(object(), source));
                    if let Some(value) = value {
                        self.write(object(), field, value);
                    }
                }
                Action::ForceReturn { integer, floating } => {
                    frame.result_x[0] = integer;
                    frame.result_q[..8].copy_from_slice(&floating.to_le_bytes());
                }
                Action::Trace => host.log_line(c"IL2PATCH", "configured method exit"),
                Action::Hud(ref layout) => {
                    if let Some(owner) = inputs[0].as_ref() {
                        match layout.apply(owner) {
                            Ok(count) => {
                                self.report(host, || format!("layout applied: hidden={count}"))
                            }
                            Err(reason) => {
                                self.report(host, || format!("layout stopped: {reason}"))
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !matches!(self.action, Action::ForceReturn { .. }) {
            if let Some(result) = result {
                frame.result_x[0] = result.target() as u64;
            }
        }
    }

    fn report(&self, host: &HostApiV0, message: impl FnOnce() -> String) {
        if self.reports.load(Ordering::Relaxed) >= 3 {
            return;
        }
        if self.reports.fetch_add(1, Ordering::Relaxed) < 3 {
            host.log_line(c"UI", &message());
        }
    }
}

fn encode_return(kind: i32, value: f64) -> Result<(u64, u64), &'static str> {
    if !value.is_finite() {
        return Err("non-finite return value");
    }
    if kind == 12 {
        let f = value as f32;
        if !f.is_finite() {
            return Err("f32 return out of range");
        }
        return Ok((0, f.to_bits() as u64));
    }
    if kind == 13 {
        return Ok((0, value.to_bits()));
    }
    if kind == 1 || reference(kind) {
        return if value == 0.0 {
            Ok((0, 0))
        } else {
            Err("only zero void/null reference return is supported")
        };
    }
    let (min, max) = match kind {
        2 => (0.0, 1.0),
        3 | 7 => (0.0, u16::MAX as f64),
        4 => (i8::MIN as f64, i8::MAX as f64),
        5 => (0.0, u8::MAX as f64),
        6 => (i16::MIN as f64, i16::MAX as f64),
        8 => (i32::MIN as f64, i32::MAX as f64),
        9 => (0.0, u32::MAX as f64),
        10 | 0x18 => (-9223372036854775808.0, 9223372036854775808.0),
        11 | 0x19 => (0.0, 18446744073709551616.0),
        _ => return Err("unsupported scalar return"),
    };
    if value < min || value > max || (matches!(kind, 10 | 11 | 0x18 | 0x19) && value == max) {
        return Err("integer return out of range");
    }
    Ok((
        if min < 0.0 {
            value as i64 as u64
        } else {
            value as u64
        },
        0,
    ))
}

struct Slot {
    plan: ManagedHookPlan,
    host: HostApiV0,
    original: AtomicUsize,
}
static SLOTS: [OnceLock<Slot>; 16] = [const { OnceLock::new() }; 16];
static INSTALL: Mutex<()> = Mutex::new(());
static ACTIVE: AtomicBool = AtomicBool::new(true);
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
}

/// # Safety
/// The trusted method entry is idle throughout patch/publication; code and host stay mapped.
/// All calls use the exact ordinary method signature checked by this plan.
/// If near-page allocation fails, the ordinary entry must have no alternate interior
/// entry into its first 16 bytes, and all covered instructions stay idle until publication.
pub unsafe fn install(plan: ManagedHookPlan, host: HostApiV0) -> Result<usize, String> {
    let _guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if SLOTS
        .iter()
        .filter_map(OnceLock::get)
        .any(|s| s.plan.entry == plan.entry)
    {
        return Err("target already has a configured hook".into());
    }
    let index = SLOTS
        .iter()
        .position(|s| s.get().is_none())
        .ok_or("configured hook slots exhausted")?;
    let entry = plan.entry;
    let destination = slot_address(index).ok_or("managed gateways require GNU ARM64 Linux")?;
    SLOTS[index]
        .set(Slot {
            plan,
            host,
            original: AtomicUsize::new(0),
        })
        .map_err(|_| "hook publication conflict")?;
    // SAFETY: checked signature, assembly gateway, process pinning and caller's idle target window.
    let original = unsafe {
        match crate::arm64_detour::install(entry, destination) {
            Ok(detour) => Ok(detour.trampoline),
            Err(crate::arm64_detour::DetourError::NoNearTrampoline { .. }) => {
                let page = libc::sysconf(libc::_SC_PAGESIZE);
                if page < 16 || entry % (page as usize) > (page as usize) - 16 {
                    Err(crate::arm64_detour::DetourError::Protection(
                        "startup redirect would cross an unverified code page".into(),
                    ))
                } else {
                    // SAFETY: the entry's readable executable page contains the whole bounded prefix.
                    // Reject early terminal instructions so short functions/adjacent entries are not overwritten.
                    let words =
                        std::array::from_fn(|i| (entry as *const u32).add(i).read_volatile());
                    if !startup_prefix(&words) {
                        Err(crate::arm64_detour::DetourError::Protection(
                            "startup redirect requires four non-terminal entry instructions".into(),
                        ))
                    } else {
                        // Caller keeps all covered instructions idle through publication; the existing
                        // common relocator validates every instruction before writing anything.
                        crate::arm64_detour::install_quiescent(entry, destination)
                    }
                }
            }
            Err(error) => Err(error),
        }
    }
    .map_err(|e| e.to_string())?;
    SLOTS[index]
        .get()
        .ok_or("hook publication lost")?
        .original
        .store(original, Ordering::Release);
    Ok(index)
}

fn startup_prefix(words: &[u32; 4]) -> bool {
    words[..3].iter().all(|&word| {
        // Unconditional B and all indirect branches/returns terminate the ordinary method prefix.
        word & 0xfc00_0000 != 0x1400_0000 && word & 0xfe00_0000 != 0xd600_0000
    })
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
macro_rules! gateways {($($name:ident),*)=>{
    extern "C-unwind" {$(fn $name();)* fn rudroid_managed_call_original(address:usize,frame:&mut Frame);}
    fn slot_address(index:usize)->Option<usize>{[$($name as *const () as usize),*].get(index).copied()}
};}
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
gateways!(
    rudroid_managed_slot_0,
    rudroid_managed_slot_1,
    rudroid_managed_slot_2,
    rudroid_managed_slot_3,
    rudroid_managed_slot_4,
    rudroid_managed_slot_5,
    rudroid_managed_slot_6,
    rudroid_managed_slot_7,
    rudroid_managed_slot_8,
    rudroid_managed_slot_9,
    rudroid_managed_slot_10,
    rudroid_managed_slot_11,
    rudroid_managed_slot_12,
    rudroid_managed_slot_13,
    rudroid_managed_slot_14,
    rudroid_managed_slot_15
);
#[cfg(not(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu")))]
fn slot_address(_: usize) -> Option<usize> {
    None
}

fn call_original(address: usize, frame: &mut Frame) {
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    // SAFETY: private dispatch passes the checked method's trampoline and complete initialized packet;
    // scalar/reference-only, register-fitting metadata forbids stack/aggregate/sret prototypes.
    unsafe {
        rudroid_managed_call_original(address, frame)
    };
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu")))]
    {
        let _ = (address, frame);
    }
}

#[no_mangle]
extern "C-unwind" fn rudroid_managed_dispatch(index: usize, frame: &mut Frame) {
    let Some(slot) = SLOTS.get(index).and_then(OnceLock::get) else {
        return;
    };
    let original = slot.original.load(Ordering::Acquire);
    if original != 0 {
        slot.plan.run(original, &slot.host, frame);
    }
}

#[cfg(test)]
#[path = "../tests/unit/managed_hooks.rs"]
mod tests;
