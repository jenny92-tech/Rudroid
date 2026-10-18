//! INPUT:  Host API input keys and checked Android KeyEvent constant lookup
//! OUTPUT:  controller/dpad/axis/keyboard policies, optional trigger key synthesis, Android identity and bounded startup-confirm schedule
//! POS:    safe reference input dialect shared by the creator-thread SDL event adapter
#![forbid(unsafe_code)]
use rudroid_common::host_api::HostApiV0;
use sdl2::keyboard::Keycode;
use std::{collections::HashMap, ffi::CString};
#[derive(Clone, Debug)]
pub(super) struct InputPolicy {
    pub buttons: [i32; 15],
    pub axes: [Option<i32>; 10],
    pub dpad: [usize; 4],
    pub keyboard: HashMap<Keycode, i32>,
    pub hats: bool,
    pub exit_chord: bool,
    pub touch_mode: bool,
    pub accurate_mouse: bool,
    pub android_xbox360_identity: bool,
    pub startup: Vec<(u64, i32)>,
}
impl Default for InputPolicy {
    fn default() -> Self {
        Self {
            buttons: super::jni_input::DEFAULT_CONTROLLER_CODES,
            axes: [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(104),
                Some(105),
            ],
            dpad: [0, 1, 2, 3],
            keyboard: HashMap::new(),
            hats: true,
            exit_chord: true,
            touch_mode: false,
            accurate_mouse: false,
            android_xbox360_identity: false,
            startup: vec![],
        }
    }
}
impl InputPolicy {
    /// Optional identity of the normalized virtual Android gamepad; no host device is changed.
    pub fn gamepad_identity<'a>(&self, host_name: &'a str) -> (&'a str, i32, i32) {
        if self.android_xbox360_identity {
            ("Xbox 360 Controller", 0x045e, 0x028e)
        } else {
            (host_name, 0, 0)
        }
    }

    pub fn read(
        api: &HostApiV0,
        mut resolve: impl FnMut(&str) -> Result<i32, String>,
    ) -> Result<Self, String> {
        let mut p = Self::default();
        let mut read = |name: &str| -> Result<Option<i32>, String> {
            let key = CString::new(format!("input.remap.{name}")).unwrap();
            api.config_string(&key, 128)
                .map(|value| resolve(&value))
                .transpose()
        };
        for (index, names) in [
            "a",
            "b",
            "x",
            "y",
            "select",
            "guide",
            "start",
            "leftstick",
            "rightstick",
            "leftshoulder",
            "rightshoulder",
            "dpup",
            "dpdown",
            "dpleft",
            "dpright",
        ]
        .iter()
        .enumerate()
        {
            if let Some(code) = read(names)? {
                p.buttons[index] = code;
            }
        }
        for (name, index) in [("back", 4), ("l1", 9), ("r1", 10), ("l3", 7), ("r3", 8)] {
            if let Some(code) = read(name)? {
                p.buttons[index] = code;
            }
        }
        for (index, name) in [
            "lstick_left",
            "lstick_right",
            "lstick_up",
            "lstick_down",
            "rstick_left",
            "rstick_right",
            "rstick_up",
            "rstick_down",
            "l2",
            "r2",
        ]
        .iter()
        .enumerate()
        {
            if let Some(code) = read(name)? {
                p.axes[index] = Some(code);
            }
        }
        // Some players consume trigger axes and interpret the additional key
        // as a separate menu button. Resolve this once; analog motion remains
        // unchanged and the existing Option fast path suppresses the keys.
        if !api.config_bool(c"input.synthesize_trigger_keys", true) {
            p.axes[8..].fill(None);
        }
        for (index, name) in ["up", "down", "left", "right"].iter().enumerate() {
            let key = CString::new(format!("input.dpad_remap.{name}")).unwrap();
            if let Some(value) = api.config_string(&key, 128) {
                p.dpad[index] =
                    direction(&value).ok_or_else(|| format!("invalid {key:?}: {value}"))?;
            }
        }
        for (key, index) in [
            (Keycode::Up, 11),
            (Keycode::Down, 12),
            (Keycode::Left, 13),
            (Keycode::Right, 14),
        ] {
            let direction = index - 11;
            let explicit = api
                .config_string(
                    &CString::new(format!(
                        "input.remap.{}",
                        ["dpup", "dpdown", "dpleft", "dpright"][p.dpad[direction]]
                    ))
                    .unwrap(),
                    128,
                )
                .is_some();
            if p.dpad[direction] != direction || explicit {
                p.keyboard.insert(key, p.buttons[11 + p.dpad[direction]]);
            }
        }
        for (key, names) in [
            (Keycode::Escape, &["select", "back"][..]),
            (Keycode::Return, &["start"][..]),
        ] {
            for name in names {
                if let Some(code) = read(name)? {
                    p.keyboard.insert(key, code);
                }
            }
        }
        p.hats = api.config_bool(c"input.dpad_synthesize_hat", true);
        p.exit_chord = api.config_bool(c"input.start_select_exit", true);
        p.touch_mode = api.config_bool(c"input.touch_mode", false);
        p.accurate_mouse = api.config_bool(c"input.accurate_mode", false);
        p.android_xbox360_identity = api.config_bool(c"input.android_xbox360_identity", false);
        let delay = api
            .config_i64(c"input.startup_confirm_delay_ms", 0)
            .clamp(0, 3_600_000) as u64;
        if delay > 0 {
            let count = api
                .config_i64(c"input.startup_confirm_repeat", 1)
                .clamp(1, 20) as u64;
            let interval = api
                .config_i64(c"input.startup_confirm_interval_ms", 1000)
                .clamp(50, 60_000) as u64;
            let names = api
                .config_string(c"input.startup_confirm_keys", 4096)
                .unwrap_or_else(|| "BUTTON_A".into());
            let keys = names
                .split(|c: char| c.is_ascii_whitespace() || c == ',' || c == '|')
                .filter(|s| !s.is_empty())
                .take(33)
                .map(&mut resolve)
                .collect::<Result<Vec<_>, _>>()?;
            if keys.len() > 32 {
                return Err("startup confirm exceeds 32 keycodes".into());
            }
            for repeat in 0..count {
                for code in &keys {
                    p.startup.push((delay + repeat * interval, *code));
                }
            }
        }
        Ok(p)
    }
}
fn direction(value: &str) -> Option<usize> {
    match value.to_ascii_lowercase().as_str() {
        "up" | "dpup" | "dpad_up" => Some(0),
        "down" | "dpdown" | "dpad_down" => Some(1),
        "left" | "dpleft" | "dpad_left" => Some(2),
        "right" | "dpright" | "dpad_right" => Some(3),
        _ => None,
    }
}
pub(super) fn normalized(value: i16, trigger: bool) -> f32 {
    if trigger {
        (f32::from(value) / 32767.0).clamp(0.0, 1.0)
    } else if value < 0 {
        f32::from(value) / 32768.0
    } else {
        f32::from(value) / 32767.0
    }
}
pub(super) fn direction_down(raw: f32, trigger: bool, positive: bool, pressed: bool) -> bool {
    let value = if trigger {
        raw
    } else if raw.abs() < 0.12 {
        0.0
    } else {
        (raw - raw.signum() * 0.12) / 0.88
    };
    let value = if positive { value } else { -value };
    if pressed {
        value > 0.30
    } else {
        value >= 0.60
    }
}
#[cfg(test)]
#[path = "../tests/unit/input_policy.rs"]
mod tests;
