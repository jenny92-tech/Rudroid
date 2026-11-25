//! INPUT: owner-thread managed calls and verified Soul Knight source metadata
//! OUTPUT: rooted offline source values, lifecycle observations and Title entry
//! POS: managed operations invoked by the offline hook callbacks
use std::{ffi::CStr, rc::Rc, sync::atomic::Ordering};
use super::{CONFIGURATION, CheckedMethod, Class, DTO, Field, GcRoot, HOST, INPUTS, IRESULT, LOAD_SCENE_PREFIX, LOAD_SCENE_RVA, MANAGER, MANIFEST, ManagedArg, ManagerObservation, Method, OFFLINE_ENTRIES, Ptr, RESULT, OwnerScope, SceneObservation, SourceJavaEnv, TITLE_SCENE, CACHE, class, code_guard, trace, trace_line, trace_stage};

impl OwnerScope<'_> {
    fn class(&self, image: &CStr, namespace: &CStr, name: &CStr) -> Result<Class, &'static str> {
        self.live()?; class(&self.api, image, namespace, name)
    }
    fn name(&self, c: Class) -> Result<String, &'static str> {
        self.api.class_type(c).and_then(|t| self.api.type_name(t)).ok_or("managed class name unavailable")
    }
    fn method(&self, c: Class, name: &CStr, static_: bool, parameters: &[&str], returns: &str)
        -> Result<CheckedMethod, &'static str> {
        self.live()?;
        self.api.matching_checked_method(c, name, static_, parameters, returns)
            .ok_or("offline managed method signature mismatch")
    }
    fn field(&self, c: Class, name: &CStr, ty: Option<&str>, offset: Option<usize>)
        -> Result<Field, &'static str> {
        self.live()?;
        let field = self.api.field(c, name).ok_or("offline field unavailable")?;
        if self.api.field_parent(field) != Some(c)
            || ty.is_some_and(|expected| self.api.field_type(field).and_then(|t| self.api.type_name(t)).as_deref() != Some(expected))
            || offset.is_some_and(|expected| self.api.field_offset(field) != Some(expected)) {
            return Err("offline field declaration mismatch");
        }
        Ok(field)
    }
    /// The hook/stop owner and common's checked tokens guard every operation.
    /// Keep this boundary separate from metadata selection and never store an
    /// unrooted managed pointer in the process-pinned runtime.
    unsafe fn call(&self, method: &CheckedMethod, receiver: Option<&GcRoot>, arguments: &[ManagedArg<'_>])
        -> Result<Option<GcRoot>, &'static str> {
        self.live()?;
        // SAFETY: hook owner, exact checked token and borrowed module-owned roots.
        let result = unsafe { self.api.invoke_checked(method, receiver, arguments, &self.gc) }?;
        self.live()?; Ok(result)
    }
    unsafe fn object(&self, c: Class) -> Result<GcRoot, &'static str> {
        self.live()?;
        let ctor = self.method(c, c".ctor", false, &[], "System.Void")?;
        // SAFETY: exact closed constructor type, attached owner and immediate strong root.
        let object = unsafe { self.api.allocate_object(c, &self.gc) }?;
        // SAFETY: the new receiver stays rooted and its original constructor is checked.
        unsafe { self.call(&ctor, Some(&object), &[]) }?;
        Ok(object)
    }
    unsafe fn string(&self, text: &CStr) -> Result<GcRoot, &'static str> {
        self.live()?;
        // SAFETY: valid owned C string on the attached owner; root immediately.
        let pointer = unsafe { self.api.new_string(text) }.ok_or("managed string allocation failed")?;
        // SAFETY: freshly allocated live managed String, no intervening allocation.
        let root = unsafe { self.gc.root(pointer) }.ok_or("managed string root failed")?;
        self.live()?; Ok(root)
    }
    unsafe fn string_utf16(&self, text: &[u16]) -> Result<GcRoot, &'static str> {
        self.live()?;
        // SAFETY: actual copied Java code units; original owner remains attached.
        let pointer = unsafe { self.api.new_string_utf16(text) }.ok_or("managed UTF16 String allocation failed")?;
        // SAFETY: newly returned String is rooted before any further allocation.
        let root = unsafe { self.gc.root(pointer) }.ok_or("managed UTF16 String root failed")?;
        self.live()?; Ok(root)
    }
    unsafe fn object_class(&self, root: &GcRoot) -> Result<Class, &'static str> {
        self.live()?;
        // SAFETY: all runtime roots came from this binding and stay strongly reachable.
        unsafe { self.api.object_class(root.target()) }.ok_or("managed object class unavailable")
    }
    unsafe fn read_ref(&self, owner: &GcRoot, field: Field) -> Result<Option<GcRoot>, &'static str> {
        self.live()?;
        // SAFETY: original declared reference on a live, module-owned root.
        let result = unsafe { self.api.read_reference_field(owner, field, &self.gc) }?;
        self.live()?; Ok(result)
    }
    unsafe fn read_static(&self, field: Field) -> Result<Option<GcRoot>, &'static str> {
        self.live()?;
        // SAFETY: attached owner; exact original static field, no business getter call.
        let result = unsafe { self.api.static_reference_field(field, &self.gc) }?;
        self.live()?; Ok(result)
    }
    unsafe fn write_ref(&self, owner: &GcRoot, field: Field, value: &GcRoot) -> Result<(), &'static str> {
        self.live()?;
        // SAFETY: explicit candidate objects, checked mutable field and rooted reference.
        unsafe { self.api.write_reference_field(owner, field, Some(value)) }?;
        self.live()
    }
    unsafe fn value(&self, owner: &GcRoot, field: Field) -> Result<Vec<u8>, &'static str> {
        self.live()?;
        // SAFETY: only known fully initialized primitive fields are requested by this module.
        let value = unsafe { self.api.value_field(owner, field) }?;
        self.live()?; Ok(value)
    }
    unsafe fn unboxed(&self, owner: &GcRoot, c: Class) -> Result<Vec<u8>, &'static str> {
        self.live()?;
        // SAFETY: used only for Boolean/Int32/TaskStatus, never DTO padding or references.
        let value = unsafe { self.api.copy_unboxed(owner, c) }?;
        self.live()?; Ok(value)
    }
    unsafe fn boolean_result(&self, value: &GcRoot) -> Result<bool, &'static str> {
        // SAFETY: result root remains live throughout metadata and primitive payload access.
        let c = unsafe { self.object_class(value) }?;
        if self.name(c)? != "System.Boolean" { return Err("managed Boolean result type mismatch"); }
        // SAFETY: Boolean is exactly one initialized byte.
        match unsafe { self.unboxed(value, c) }?.as_slice() {
            [0] => Ok(false), [1] => Ok(true), _ => Err("managed Boolean representation mismatch"),
        }
    }
    unsafe fn set_bool(&self, owner: &GcRoot, field: Field, value: bool) -> Result<(), &'static str> {
        self.live()?;
        // SAFETY: generated configuration entry, metadata-checked Boolean field and owner.
        if !unsafe { self.api.set_numeric_field(owner.target(), field, rudroid_unity_common::il2cpp::NumericValue::Bool(value)) } {
            return Err("configuration Boolean write failed");
        }
        // SAFETY: same generated object's primitive Boolean field.
        if unsafe { self.value(owner, field) }? != [u8::from(value)] { return Err("configuration Boolean roundtrip failed"); }
        Ok(())
    }
    unsafe fn assert_string(&self, root: &GcRoot, expected: &str) -> Result<(), &'static str> {
        self.live()?;
        // SAFETY: caller selected a String field/type and keeps its root alive.
        if unsafe { self.api.string_eq(root.target(), expected) } { Ok(()) }
        else { Err("generated manifest/configuration string mismatch") }
    }
    fn task_class(&self, source: Method) -> Result<Class, &'static str> {
        self.api.method_return_type(source).and_then(|t| self.api.class_from_type(t))
            .ok_or("closed Task type unavailable")
    }
    /// Construct a genuine Task(TResult), then verify completion and identity.
    unsafe fn completed_task(&self, source: Method, result: &GcRoot, result_name: &str)
        -> Result<GcRoot, &'static str> {
        let task_class = self.task_class(source)?;
        let ctor = self.method(task_class, c".ctor", false, &[result_name], "System.Void")?;
        let get = self.method(task_class, c"get_Result", false, &[], result_name)?;
        let base = self.api.parent(task_class).ok_or("closed Task base unavailable")?;
        if self.name(base)? != "System.Threading.Tasks.Task" { return Err("closed Task base mismatch"); }
        let done = self.method(base, c"get_IsCompleted", false, &[], "System.Boolean")?;
        let status = self.method(base, c"get_Status", false, &[], "System.Threading.Tasks.TaskStatus")?;
        self.live()?;
        // SAFETY: closed Task<T> from the exact source's return type on its owner thread.
        let task = unsafe { self.api.allocate_object(task_class, &self.gc) }?;
        // SAFETY: rooted genuine TResult and original closed constructor.
        unsafe { self.call(&ctor, Some(&task), &[ManagedArg::Reference(Some(result))]) }?;
        // SAFETY: original Task completion accessors on the newly constructed root.
        let completed = unsafe { self.call(&done, Some(&task), &[]) }?.ok_or("Task completion result null")?;
        // SAFETY: Boolean returned by the checked completion accessor.
        if !unsafe { self.boolean_result(&completed) }? { return Err("Task did not complete"); }
        // SAFETY: checked TaskStatus value returned on the same owner.
        let state = unsafe { self.call(&status, Some(&task), &[]) }?.ok_or("Task status result null")?;
        // SAFETY: owned boxed TaskStatus payload, known Int32 enum layout.
        let state_class = unsafe { self.object_class(&state) }?;
        // SAFETY: TaskStatus is an initialized four-byte enum, no padding/references.
        if unsafe { self.unboxed(&state, state_class) }? != 5i32.to_le_bytes() { return Err("Task status is not RanToCompletion"); }
        // SAFETY: genuine closed Task result remains reachable through both roots.
        let resolved = unsafe { self.call(&get, Some(&task), &[]) }?.ok_or("Task result null")?;
        if resolved.target() != result.target() { return Err("Task result identity mismatch"); }
        Ok(task)
    }

    pub(super) fn configuration(&self) -> Result<Ptr, &'static str> {
        if let Some(value) = CACHE.with(|slot| slot.borrow().configuration.clone()) {
            return Ok(value.target());
        }
        let force_patch = self.class(c"Assembly-CSharp.dll", c"RGScript.Util.ForcePatch", c"ForcePatchManager")?;
        let last = self.field(force_patch, c"lastDynamicConfig", None, None)?;
        // SAFETY: program configuration source only; never account/save state.
        if unsafe { self.read_static(last) }?.is_some() { return Err("existing dynamic configuration prevents original-default substitution"); }
        let dynamic_class = self.api.field_type(last).and_then(|t| self.api.class_from_type(t))
            .ok_or("dynamic configuration type unavailable")?;
        // SAFETY: exact original default-configuration type and its original constructor.
        let dynamic = unsafe { self.object(dynamic_class) }?;
        let entries_field = self.field(dynamic_class, c"<entries>k__BackingField",
            Some("System.Collections.Generic.List<RGScript.Util.ForcePatch.ForcePatchDynamicConfigEntry>"), Some(16))?;
        let list_class = self.api.field_type(entries_field).and_then(|t| self.api.class_from_type(t))
            .ok_or("configuration entry list unavailable")?;
        let add = self.method(list_class, c"Add", false, &["RGScript.Util.ForcePatch.ForcePatchDynamicConfigEntry"], "System.Void")?;
        let entry_class = self.api.method_param(add.metadata(), 0).and_then(|t| self.api.class_from_type(t))
            .ok_or("configuration entry type unavailable")?;
        let key = self.field(entry_class, c"<key>k__BackingField", Some("System.String"), Some(16))?;
        let open = self.field(entry_class, c"<open>k__BackingField", Some("System.Boolean"), Some(24))?;
        // SAFETY: closed original List<Entry> and constructor on the owner.
        let entries = unsafe { self.object(list_class) }?;
        // Keep the original fallback labels and version values; apply only the
        // explicit cloud-restore policy above, not a current server config or
        // any paid entitlement result.
        for (label, enabled) in OFFLINE_ENTRIES {
            // SAFETY: original entry constructor and generated constant String.
            let entry = unsafe { self.object(entry_class) }?;
            // SAFETY: valid original fallback label, strongly rooted immediately.
            let label = unsafe { self.string(label) }?;
            // SAFETY: newly allocated entry's checked reference and Boolean fields.
            unsafe { self.write_ref(&entry, key, &label) }?;
            // SAFETY: explicit offline entry policy on this generated object.
            unsafe { self.set_bool(&entry, open, enabled) }?;
            // SAFETY: original List<Entry>.Add receives its rooted element.
            unsafe { self.call(&add, Some(&entries), &[ManagedArg::Reference(Some(&entry))]) }?;
        }
        // SAFETY: generated list assigned via the runtime's write barrier.
        unsafe { self.write_ref(&dynamic, entries_field, &entries) }?;
        for (field_name, offset, text) in [
            (c"<gameVersion>k__BackingField", 24, c"2.7.0"),
            (c"<gameBundleVersion>k__BackingField", 32, c"100"),
            (c"<umpTakeEffectBundleVersion>k__BackingField", 40, c"100"),
            (c"<vvpayMaxTakeEffectBundleVersion>k__BackingField", 56, c"100"),
        ] {
            let field = self.field(dynamic_class, field_name, Some("System.String"), Some(offset))?;
            // SAFETY: original fallback constant in a new managed String.
            let value = unsafe { self.string(text) }?;
            // SAFETY: own newly constructed default object, checked String field.
            unsafe { self.write_ref(&dynamic, field, &value) }?;
        }
        let interval = self.field(dynamic_class, c"<vnmMarqueeInterval>k__BackingField", Some("System.Int32"), Some(48))?;
        // SAFETY: primitive constructor default, checked width and field type.
        if unsafe { self.value(&dynamic, interval) }? != 0i32.to_le_bytes() { return Err("original interval default mismatch"); }
        let upgrade = self.field(dynamic_class, c"<upgradeGameInfo>k__BackingField", None, Some(64))?;
        // SAFETY: original optional default reference, never an account value.
        if unsafe { self.read_ref(&dynamic, upgrade) }?.is_some() { return Err("original upgrade default mismatch"); }
        let task_class = self.task_class(self.sources[0])?;
        let get = self.method(task_class, c"get_Result", false, &[], CONFIGURATION)?;
        let config_class = self.api.class_from_type(get.returns()).ok_or("configuration result class unavailable")?;
        // SAFETY: original Configuration constructor, rooted before any managed call.
        let configuration = unsafe { self.object(config_class) }?;
        let config_dynamic = self.field(config_class, c"dynamic", Some(&self.name(dynamic_class)?), None)?;
        let hotfix = self.field(config_class, c"hotfix", None, None)?;
        // SAFETY: own result configuration; preserve null original hotfix default.
        unsafe { self.write_ref(&configuration, config_dynamic, &dynamic) }?;
        // SAFETY: original optional configuration reference, no arbitrary field access.
        if unsafe { self.read_ref(&configuration, hotfix) }?.is_some() { return Err("configuration hotfix default mismatch"); }
        // SAFETY: rooted exact Configuration and original Task<Configuration> constructor.
        let task = unsafe { self.completed_task(self.sources[0], &configuration, CONFIGURATION) }?;
        self.live()?;
        let value = task.target();
        let old = CACHE.with(|slot| slot.borrow_mut().configuration.replace(Rc::new(task)));
        drop(old);
        Ok(value)
    }

    pub(super) fn manifest(&self) -> Result<Ptr, &'static str> {
        if let Some(value) = CACHE.with(|slot| slot.borrow().manifest.clone()) {
            return Ok(value.target());
        }
        let manifest_class = self.class(c"Assembly-CSharp.dll", c"RGScript.Util.HotUpdate", c"HotUpdateManifest")?;
        let json_class = self.class(c"UnityEngine.JSONSerializeModule.dll", c"UnityEngine", c"JsonUtility")?;
        let parser = self.method(json_class, c"FromJson", true, &["System.String", "System.Type"], "System.Object")?;
        let inputs = INPUTS.get().ok_or("offline inputs unavailable")?;
        // SAFETY: original hash-verified manifest, no persisted copy or arbitrary source.
        let json = unsafe { self.string(&inputs.manifest) }?;
        let manifest_type = self.api.class_type(manifest_class).ok_or("manifest metadata type unavailable")?;
        self.live()?;
        // SAFETY: original manifest Type object, immediately rooted before parsing.
        let reflection = unsafe { self.api.type_object(manifest_type) }.ok_or("manifest reflection type unavailable")?;
        // SAFETY: freshly returned live reflection object on the attached owner.
        let reflection = unsafe { self.gc.root(reflection) }.ok_or("manifest reflection root failed")?;
        // SAFETY: original JsonUtility parser receives the two checked rooted references.
        let parsed = unsafe { self.call(&parser, None, &[ManagedArg::Reference(Some(&json)), ManagedArg::Reference(Some(&reflection))]) }?
            .ok_or("original manifest parse returned null")?;
        // SAFETY: the parser result is strongly rooted throughout validation.
        if unsafe { self.object_class(&parsed) }? != manifest_class { return Err("parsed manifest runtime type mismatch"); }
        for (name, expected) in [(c"buildTarget", "Android"), (c"version", "8.7.0"), (c"platform", "Android")] {
            let field = self.field(manifest_class, name, Some("System.String"), None)?;
            // SAFETY: known original manifest String field, no account/save query.
            let value = unsafe { self.read_ref(&parsed, field) }?.ok_or("parsed manifest String null")?;
            // SAFETY: metadata-selected original manifest string remains rooted.
            unsafe { self.assert_string(&value, expected) }?;
        }
        for (name, ty, expected) in [
            (c"isDebug", "System.Boolean", vec![0]),
            (c"timestamp", "System.Int64", vec![0; 8]),
            (c"preTimestamp", "System.Int64", vec![0; 8]),
        ] {
            let field = self.field(manifest_class, name, Some(ty), None)?;
            // SAFETY: initialized primitive manifest fields, exact metadata widths.
            if unsafe { self.value(&parsed, field) }? != expected { return Err("parsed manifest scalar mismatch"); }
        }
        let files_field = self.field(manifest_class, c"fileInfoList",
            Some("System.Collections.Generic.List<RGScript.Util.HotUpdate.HotUpdateFileInfo>"), None)?;
        // SAFETY: original list reference from the generated manifest object.
        let files = unsafe { self.read_ref(&parsed, files_field) }?.ok_or("parsed manifest file list null")?;
        // SAFETY: strong list root, used only to select its original count getter.
        let files_class = unsafe { self.object_class(&files) }?;
        let count = self.method(files_class, c"get_Count", false, &[], "System.Int32")?;
        // SAFETY: original list getter, no resource or save content read.
        let count = unsafe { self.call(&count, Some(&files), &[]) }?.ok_or("manifest count null")?;
        // SAFETY: boxed Int32 has no padding or references.
        let count_class = unsafe { self.object_class(&count) }?;
        // SAFETY: checked count result's four-byte value payload.
        if unsafe { self.unboxed(&count, count_class) }? != 2426i32.to_le_bytes() { return Err("manifest file count mismatch"); }
        // SAFETY: rooted original parsed manifest and genuine closed Task constructor.
        let task = unsafe { self.completed_task(self.sources[1], &parsed, MANIFEST) }?;
        self.live()?;
        let pointer = task.target();
        let old = CACHE.with(|slot| slot.borrow_mut().manifest.replace(Rc::new(task)));
        drop(old);
        Ok(pointer)
    }

    pub(super) fn time(&self) -> Result<Ptr, &'static str> {
        let net_time = self.class(c"Assembly-CSharp.dll", c"", c"NetTime")?;
        let nested = self.api.nested_classes(net_time).ok_or("NetTime nested metadata unavailable")?;
        let mut common = nested.into_iter().filter(|&c| self.api.class_name(c).as_deref() == Some("CommonGetUtcDateTimeResult"));
        let common = common.next().filter(|_| common.next().is_none()).ok_or("NetTime common result type ambiguous")?;
        let result_class = self.api.parent(common).ok_or("NetTime result parent unavailable")?;
        if self.name(result_class)? != RESULT { return Err("NetTime closed result type mismatch"); }
        let ok = self.method(result_class, c"Ok", true, &[DTO], IRESULT)?;
        let is_ok = self.method(result_class, c"IsOk", false, &[], "System.Boolean")?;
        let dto_class = self.api.method_param(ok.metadata(), 0).and_then(|t| self.api.class_from_type(t))
            .ok_or("DateTimeOffset type unavailable")?;
        if self.api.value_layout(dto_class).map(|v| v.0) != Some(16) { return Err("DateTimeOffset value ABI mismatch"); }
        let utc_now = self.method(dto_class, c"get_UtcNow", true, &[], DTO)?;
        // SAFETY: original BCL UtcNow produces a genuine fresh boxed DTO on every source call.
        let utc = unsafe { self.call(&utc_now, None, &[]) }?.ok_or("local UTC result null")?;
        // SAFETY: BoxedValue passes the runtime payload while rooted; no DTO padding is read/copied.
        let result = unsafe { self.call(&ok, None, &[ManagedArg::BoxedValue(&utc)]) }?.ok_or("time Result.Ok null")?;
        // SAFETY: original closed Result success accessor on the rooted actual result.
        let success = unsafe { self.call(&is_ok, Some(&result), &[]) }?.ok_or("time IsOk result null")?;
        // SAFETY: checked Boolean accessor result.
        if !unsafe { self.boolean_result(&success) }? { return Err("original time Result.Ok did not report success"); }
        // SAFETY: genuine IResult payload and original Task<IResult> constructor.
        let task = unsafe { self.completed_task(self.sources[2], &result, IRESULT) }?;
        self.live()?;
        let pointer = task.target();
        let old = CACHE.with(|slot| slot.borrow_mut().time.replace(task));
        drop(old);
        Ok(pointer)
    }

    /// Read the original singleton's existing cache; never call Lazy.Value.
    /// An absent cache during startup means wait, not construct a manager.
    unsafe fn existing_manager(&self) -> Result<Option<GcRoot>, &'static str> {
        self.live()?;
        let manager_class = self.scene_entry.manager_class;
        let parent = self.api.parent(manager_class).ok_or("manager singleton parent unavailable")?;
        if self.name(parent)? != format!("ChillyRoom.Common.Singleton<{MANAGER}>") { return Err("manager singleton parent mismatch"); }
        let lazy_type = format!("System.Lazy<{MANAGER}>");
        let lazy_field = self.field(parent, c"lazy", Some(&lazy_type), Some(0))?;
        if self.api.field_flags(lazy_field) != Some(49) { return Err("manager Lazy field flags mismatch"); }
        // SAFETY: existing original singleton cache only; Lazy.Value/factory never invoked.
        let Some(lazy) = unsafe { self.read_static(lazy_field) }? else { return Ok(None); };
        // SAFETY: strong cached Lazy root; no business getter call.
        let lazy_class = unsafe { self.object_class(&lazy) }?;
        if self.name(lazy_class)? != lazy_type { return Err("manager Lazy runtime type mismatch"); }
        let cache = self.field(lazy_class, c"_value", Some(MANAGER), Some(32))?;
        if self.api.field_flags(cache) != Some(1) { return Err("manager cached field flags mismatch"); }
        // SAFETY: original already-created manager field, not any account/delegate value.
        let Some(manager) = unsafe { self.read_ref(&lazy, cache) }? else { return Ok(None); };
        // SAFETY: exact typed original singleton reference stays rooted.
        if unsafe { self.object_class(&manager) }? != manager_class { return Err("cached manager runtime type mismatch"); }
        Ok(Some(manager))
    }

    /// Read only program state, after the original manager Update returned.
    /// Every root is callback-local; missing/changed state never changes the
    /// source failure flag, SceneOnce, original dispatch or engine fields.
    pub(super) fn observe_manager(&self, receiver: Ptr) -> Result<ManagerObservation, &'static str> {
        self.live()?;
        // SAFETY: original owner; inspect only the existing singleton cache.
        let Some(manager) = unsafe { self.existing_manager() }? else {
            return Ok(ManagerObservation::Stage("manager-update manager-not-cached"));
        };
        if manager.target() != receiver { return Ok(ManagerObservation::Stage("manager-update receiver-not-cached")); }
        let stop = self.manager_is_stop.ok_or("diagnostic IsStop field unavailable")?;
        let mut flags = [false; 4];
        for (index, field) in std::iter::once(stop).chain(self.scene_entry.ready).enumerate() {
            // SAFETY: prepared exact instance Boolean, strongly rooted manager
            // on its original owner; all four payloads are initialized bytes.
            flags[index] = match unsafe { self.value(&manager, field) }?.as_slice() {
                [0] => false, [1] => true, _ => return Err("diagnostic Boolean unavailable"),
            };
        }
        // SAFETY: exact declared ILifeState reference on the fresh manager.
        let current = unsafe { self.read_ref(&manager, self.scene_entry.life_state) }?;
        let current_type = if let Some(current) = current {
            // SAFETY: fresh strong current root, never the old raw receiver.
            let class = unsafe { self.object_class(&current) }?;
            let name = self.name(class)?;
            if name.len() > 256 { return Err("diagnostic current type overlong"); }
            name
        } else { "<no-current>".to_owned() };
        Ok(ManagerObservation::State { current_type, stopped: flags[0], ready: [flags[1], flags[2], flags[3]] })
    }

    pub(super) fn android_id(&self) -> Result<Ptr, &'static str> {
        // JNI attachment is allowed only after the real managed caller passed
        // source/live's existing owner gate. Never attach an IL2CPP thread here.
        self.live()?;
        let host = HOST.get().ok_or("Android ID source host unavailable")?;
        let java = SourceJavaEnv::enter(host)?;
        let units = crate::android_id::read(java.env).map_err(|error| match error {
            crate::android_id::Error::JavaException => "Android ID Java exception",
            crate::android_id::Error::LocalFrame => "Android ID local frame failed",
            crate::android_id::Error::PublicContract => "Android ID public Java contract unavailable",
            crate::android_id::Error::PublicObject => "Android ID public Context object unavailable",
            crate::android_id::Error::StringCopy => "Android ID Java UTF16 copy failed",
        });
        let units = java.finish(units)?;
        self.live()?;
        let root = match units {
            // SAFETY: exact copied UTF-16, including empty/NUL/unpaired units;
            // same original owner, immediately strongly rooted.
            Some(units) => Some(unsafe { self.string_utf16(&units) }?),
            None => None, // Original nullable result, never a generated identity.
        };
        self.live()?;
        let pointer = root.as_ref().map_or(std::ptr::null_mut(), GcRoot::target);
        // Keep the returned String rooted until a subsequent source result or
        // real owner stop. Replacement drops outside the cache mutex; prior
        // managed callers retain their own ordinary references.
        let old = CACHE.with(|slot| std::mem::replace(&mut slot.borrow_mut().android_id, root));
        drop(old);
        Ok(pointer)
    }

    pub(super) fn login(&self) -> Result<(), &'static str> {
        // SAFETY: original Login owner; only the already-created manager cache.
        let manager = unsafe { self.existing_manager() }?.ok_or("manager cache is not initialized")?;
        let manager_class = self.scene_entry.manager_class;
        let publish = self.method(manager_class, c"SetAccountLogin", false, &["System.String"], "System.Void")?;
        if self.api.method_entry(publish.metadata()) != self.api.base().checked_add(0x7d2be64) {
            return Err("original login event entry mismatch");
        }
        code_guard(&self.api, publish.metadata(), 0x7d2be64,
            "fe5fbda9f65701a9f44f02a9753404b0a86a7139f40301aaf30300aa08030037")?;
        let inputs = INPUTS.get().ok_or("offline profile inputs unavailable")?;
        // SAFETY: generated fixed local namespace, not a server account or credential.
        let id = unsafe { self.string(&inputs.profile_id) }?;
        self.live()?;
        if self.login_claimed.swap(true, Ordering::AcqRel) { return Ok(()); }
        // SAFETY: original Login timing registered the original callbacks already.
        // The original setter owns event order, migration/load and state change;
        // this adapter never writes ready flags or calls Wait.OnAccountLogin.
        unsafe { self.call(&publish, Some(&manager), &[ManagedArg::Reference(Some(&id))]) }?;
        Ok(())
    }

    unsafe fn scene_ready(&self, receiver: Ptr) -> Result<Option<(GcRoot, GcRoot)>, &'static str> {
        self.live()?;
        // SAFETY: original managed Update owner, cached singleton only.
        let Some(manager) = unsafe { self.existing_manager() }? else {
            trace_stage("update manager-not-cached"); return Ok(None);
        };
        // SAFETY: exact ILifeState reference on the rooted cached manager.
        let current = unsafe { self.read_ref(&manager, self.scene_entry.life_state) }?;
        if trace().wants_scene() {
            // SAFETY: diagnostics stay on this real owner and use these roots.
            // Observer failures are recorded, never returned to scene gating.
            unsafe { self.observe_scene(&manager, current.as_ref(), receiver) };
        }
        let Some(current) = current else { return Ok(None); };
        if current.target() != receiver { return Ok(None); }
        // SAFETY: use only the freshly rooted current state, never raw receiver.
        if unsafe { self.object_class(&current) }? != self.scene_entry.playing_class { return Ok(None); }
        for field in self.scene_entry.ready {
            // SAFETY: prepared exact instance Boolean field, initialized byte.
            match unsafe { self.value(&manager, field) }?.as_slice() {
                [0] => return Ok(None), [1] => {}, _ => return Err("offline scene readiness Boolean invalid"),
            }
        }
        Ok(Some((manager, current)))
    }

    unsafe fn observe_scene(&self, manager: &GcRoot, current: Option<&GcRoot>, receiver: Ptr) {
        let observation = (|| {
            let mut ready = [false; 3];
            for (index, field) in self.scene_entry.ready.into_iter().enumerate() {
                // SAFETY: same prepared primitive fields, rooted manager/owner.
                ready[index] = match unsafe { self.value(manager, field) }?.as_slice() {
                    [0] => false, [1] => true, _ => return Err("diagnostic Boolean unavailable"),
                };
            }
            let current_type = if let Some(current) = current {
                // SAFETY: actual rooted current; no old raw receiver access.
                let class = unsafe { self.object_class(current) }?;
                let name = self.name(class)?;
                if name.len() <= 192 { name } else { "<overlong-metadata-type>".to_owned() }
            } else { "<no-current>".to_owned() };
            Ok(SceneObservation::State { current_type, ready,
                receiver_current: current.is_some_and(|root| root.target() == receiver) })
        })().unwrap_or(SceneObservation::Stage("update observation-unavailable"));
        if let Some(line) = trace().observe(observation) { trace_line(&line); }
    }

    pub(super) fn scene(&self, receiver: Ptr) -> Result<(), &'static str> {
        // SAFETY: source's live/installed owner gate precedes all observations.
        let ready = self.live().and_then(|_| unsafe { self.scene_ready(receiver) });
        self.scene_once.run(ready, |(_manager, _playing)| {
            self.live()?;
            code_guard(&self.api, self.scene_entry.load.metadata(), LOAD_SCENE_RVA, LOAD_SCENE_PREFIX)?;
            // The phone experiment independently named the original success
            // branch's compiled literal as "Title" and verified this same
            // public static CoreModule overload/RVA/prefix. Reuse that fixed
            // scene via its checked MI; no account IResult is manufactured.
            // SAFETY: attached owner, immediately rooted new fixed String.
            let title = unsafe { self.string(TITLE_SCENE) }?;
            // SAFETY: original LoadScene(String), rooted argument and owner.
            // runtime_invoke captures managed exceptions. Manager/current/
            // String roots unwind here on the same owner on success or error;
            // none escape into the process-pinned cache. Reentrant stopping
            // is caught by call/live before any subsequent managed access.
            if let Some(line) = trace().title(0) { trace_line(line); }
            // SAFETY: same checked original overload and rooted argument above.
            let result = unsafe { self.call(&self.scene_entry.load, None, &[ManagedArg::Reference(Some(&title))]) };
            if let Some(line) = trace().title(if result.is_ok() { 1 } else { 2 }) { trace_line(line); }
            result?;
            Ok(())
        })?;
        Ok(())
    }
}
