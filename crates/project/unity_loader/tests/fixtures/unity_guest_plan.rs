//! Test-only helper: populate a `GuestPlan` with the standard Unity ARM64
//! IL2CPP guest library order from `rudroid-unity-common` when a synthetic kit
//! does not register an `EngineProfile`.
use rudroid_common::registry::{
    RUDROID_GUEST_ENTRY_AFTER_FMOD, RUDROID_GUEST_ENTRY_FMOD_JNI_OK,
};
use rudroid_common::ModuleKind;
use rudroid_unity_loader::guest::{EntryGate, GuestEntry, GuestPlan};

pub fn push_unity_arm64_entries(plan: &mut GuestPlan) {
    for spec in rudroid_unity_common::unity_arm64_guest_entries(false) {
        let mut gates = Vec::new();
        if spec.after_role_id != 0 {
            gates.push(EntryGate::KindLoaded(
                ModuleKind::from_u32(spec.after_role_id).expect("valid role id"),
            ));
        }
        if spec.flags & RUDROID_GUEST_ENTRY_AFTER_FMOD != 0 {
            gates.push(EntryGate::KindLoaded(ModuleKind::Fmod));
        }
        if spec.flags & RUDROID_GUEST_ENTRY_FMOD_JNI_OK != 0 {
            gates.push(EntryGate::FmodJniOk);
        }
        plan.entries.push(GuestEntry {
            kind: (spec.role_id != 0)
                .then(|| ModuleKind::from_u32(spec.role_id).expect("valid role id")),
            relative_path: spec.relative_path,
            required: spec.required,
            gates,
        });
    }
}
