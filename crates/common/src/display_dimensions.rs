//! INPUT:  output geometry and the existing gpu render-width/height/scale keys
//! OUTPUT:  validated output/render dimensions and output-to-render coordinate mapping
//! POS:    pure shared geometry for the loader, EGL/NDK, Android display and SDL input
#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderDimensions {
    pub output_width: i32,
    pub output_height: i32,
    pub width: i32,
    pub height: i32,
    pub sharp: bool,
}

impl RenderDimensions {
    pub fn resolve(output_width: i64, output_height: i64, width: i64, height: i64,
        percent: i64, sharp: bool) -> Result<Self, &'static str> {
        if !(1..=32768).contains(&output_width) || !(1..=32768).contains(&output_height) {
            return Err("device display dimensions must be in 1..=32768");
        }
        if !matches!(percent, 50 | 75 | 100) {
            return Err("gpu.renderScalePercent must be 50, 75 or 100");
        }
        if !(0..=32768).contains(&width) || !(0..=32768).contains(&height) || (width == 0) != (height == 0) {
            return Err("gpu.renderWidth/renderHeight must both be zero or both in 1..=32768");
        }
        let scaled = |output: i64, exact| if exact > 0 { exact } else { (output * percent / 100).max(1) } as i32;
        Ok(Self { output_width: output_width as i32, output_height: output_height as i32,
            width: scaled(output_width, width), height: scaled(output_height, height), sharp })
    }

    pub fn from_host(api: &crate::host_api::HostApiV0) -> Result<Self, &'static str> {
        Self::resolve(api.config_i64(c"device.displayWidth", 640), api.config_i64(c"device.displayHeight", 480),
            api.config_i64(c"gpu.renderWidth", 0), api.config_i64(c"gpu.renderHeight", 0),
            api.config_i64(c"gpu.renderScalePercent", 100), api.config_bool(c"gpu.renderScaleSharp", true))
    }

    pub fn scaled(self) -> bool { self.width != self.output_width || self.height != self.output_height }

    pub fn point(self, x: i32, y: i32) -> (i32, i32) {
        let map = |value, output, render| (i64::from(value) * i64::from(render) / i64::from(output))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        (map(x, self.output_width, self.width), map(y, self.output_height, self.height))
    }
}

#[cfg(test)]
#[path = "../tests/unit/display_dimensions.rs"]
mod tests;
