//! INPUT:  legacy ui_layout Host API configuration
//! OUTPUT:  owned validated HUD requests borrowing a common HudSpec at installation
//! POS:    safe configuration reader shared by the production initialization path and regressions
#![forbid(unsafe_code)]
use rudroid_common::{host_api::HostApiV0, hud_layout::HudSpec};
use std::ffi::{CStr, CString};
pub(super) struct LayoutRequest {
    assembly: CString,
    namespace: CString,
    class: CString,
    method: CString,
    argc: u32,
    hide: Vec<usize>,
    from: Option<usize>,
    to: Option<usize>,
    dx: i32,
    dy: i32,
    margin: Option<i32>,
}
impl LayoutRequest {
    pub(super) fn read(host: &HostApiV0) -> Result<Option<Self>, &'static str> {
        if !host.config_bool(c"ui_layout.enabled", true) {
            return Ok(None);
        }
        fn text(host: &HostApiV0, key: &CStr, default: &str) -> Result<CString, &'static str> {
            CString::new(
                host.config_string(key, 512)
                    .unwrap_or_else(|| default.into()),
            )
            .map_err(|_| "HUD string contains NUL")
        }
        let class = text(host, c"ui_layout.class", "")?;
        if class.as_bytes().is_empty() {
            return Ok(None);
        }
        let assembly = text(host, c"ui_layout.assembly", "Assembly-CSharp")?;
        let namespace = text(host, c"ui_layout.namespace", "")?;
        let method = text(host, c"ui_layout.method", "Start")?;
        let argc = host.config_i64(c"ui_layout.argc", 0);
        if !(0..=6).contains(&argc)
            || method.as_bytes().is_empty()
            || assembly.as_bytes().is_empty()
        {
            return Err("invalid HUD target/arity");
        }
        let mut hide = Vec::new();
        for index in 0..host.config_len(c"ui_layout.hide").min(16) {
            let key =
                CString::new(format!("ui_layout.hide.{index}")).map_err(|_| "invalid HUD key")?;
            let offset = host.config_i64(&key, -1);
            if offset >= 0 {
                hide.push(usize::try_from(offset).map_err(|_| "HUD offset overflow")?);
            }
        }
        let from = usize::try_from(host.config_i64(c"ui_layout.move_from", -1)).ok();
        let to = usize::try_from(host.config_i64(c"ui_layout.move_to", -1)).ok();
        if hide.is_empty() && from.is_none() && to.is_none() {
            return Ok(None);
        }
        if from.is_some() != to.is_some() {
            return Err("both HUD move fields must be configured");
        }
        let dx = i32::try_from(host.config_i64(c"ui_layout.move_dx", 0))
            .map_err(|_| "HUD x displacement overflow")?;
        let dy = i32::try_from(host.config_i64(c"ui_layout.move_dy", 0))
            .map_err(|_| "HUD y displacement overflow")?;
        let margin = host.config_i64(c"ui_layout.right_margin", -1);
        let margin = if margin < 0 {
            None
        } else {
            Some(i32::try_from(margin).map_err(|_| "HUD margin overflow")?)
        };
        Ok(Some(Self {
            assembly,
            namespace,
            class,
            method,
            argc: argc as u32,
            hide,
            from,
            to,
            dx,
            dy,
            margin,
        }))
    }
    pub(super) fn spec(&self) -> HudSpec<'_> {
        HudSpec {
            assembly: &self.assembly,
            namespace: &self.namespace,
            class: &self.class,
            method: &self.method,
            argc: self.argc,
            hide: &self.hide,
            move_from: self.from,
            move_to: self.to,
            dx: self.dx,
            dy: self.dy,
            right_margin: self.margin,
        }
    }
}

