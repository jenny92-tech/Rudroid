//! INPUT:  checked Android launcher, title features, accepted scene request and cancellation
//! OUTPUT:  startup-panel and mobile GameObject deactivation through common's rooted adapter
//! POS:    safe startup policy; no managed references survive, no destruction or unloading
#![forbid(unsafe_code)]
use super::SilksongFeatures;
use rudroid_common::{il2cpp::Il2cpp, unity_objects::NamedGameObjectPruner};
use std::ffi::CStr;

/// Hide only the verified launcher's child panel after its original Start and
/// before the first frame. The launcher stays active for button events and its
/// deferred transition; no gameplay frame subscription or polling is added.
pub(super) fn hide_launcher_prompt(api: Il2cpp, cancelled: impl Fn()->bool) {
    if cancelled() { return; }
    let result = NamedGameObjectPruner::discover(api)
        .and_then(|pruner| pruner.disable(&[c"/Launcher/MsgBox/Panel"], cancelled));
    match result {
        Ok(1) => super::host_log_raw("launcher disclaimer panel hidden before first frame"),
        Ok(_) => super::host_log_raw("launcher disclaimer panel not found; native startup retained"),
        Err("cancelled") => {},
        Err(reason) => super::host_log_raw(&format!("launcher panel hiding skipped: {reason}")),
    }
}

#[derive(Clone, Copy)]
pub(super) struct Selection { camera: bool, controls: bool, watermark: bool, destroy_requested: bool }
impl Selection {
    pub(super) fn from_features(features: &SilksongFeatures) -> Self {
        Self { camera:features.disable_touch_camera, controls:features.disable_touch_controls,
            watermark:features.disable_watermark_canvas, destroy_requested:features.destroy_bootstrap_objects }
    }
    pub(super) fn run(self, api: Il2cpp, cancelled: impl Fn()->bool) {
        let mut names: Vec<&CStr> = Vec::new();
        if self.camera { names.push(c"Touch Camera"); }
        if self.controls {
            names.extend([c"Stick", c"Jump_Action1", c"Cast_Action2", c"Attack_Action3", c"DownAttack_Action5",
                c"Dash_RightTrigger", c"QuickCast_RightBumper", c"QuickMap_LeftBumper", c"SuperDash_LeftTrigger",
                c"DreamNail_Action4", c"Taunt_RightStickButton", c"Options", c"View"]);
        }
        if self.watermark { names.push(c"WaterMark"); }
        if names.is_empty() || cancelled() { return; }
        if self.destroy_requested {
            super::host_log_raw("bootstrap destruction remains unavailable; applying configured deactivation only");
        }
        let result = NamedGameObjectPruner::discover(api).and_then(|pruner| pruner.disable(&names, cancelled));
        match result {
            Ok(disabled) => super::host_log_raw(&format!("disabled bootstrap game objects={disabled} destroyed=0")),
            Err("cancelled") => {},
            Err(reason) => super::host_log_raw(&format!("bootstrap deactivation stopped without retry: {reason}")),
        }
    }
}
