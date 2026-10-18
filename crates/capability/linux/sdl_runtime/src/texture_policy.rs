//! INPUT:  immutable GPU configuration, render dimensions and owned pixel slices
//! OUTPUT:  checked texture plans, unpack spans, mip rebasing and CPU downsampling
//! POS:    safe texture policy; native GL ownership and pointers stay in texture_gl
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
// Policy/filter behavior follows Rudroid's texture_caps and texture_downsample.
#![forbid(unsafe_code)]
use rudroid_common::{display_dimensions::RenderDimensions, host_api::HostApiV0};
use std::sync::OnceLock;

pub(super) const MAX_BYTES: usize = 128 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    Box,
    Nearest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Policy {
    pub rgba: i32,
    pub etc: i32,
    pub astc: i32,
    pub filter: Filter,
    pub stats: bool,
    pub skip_narrow: bool,
    pub skip_display: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            rgba: 0,
            etc: 0,
            astc: 0,
            filter: Filter::Box,
            stats: false,
            skip_narrow: true,
            skip_display: true,
        }
    }
}
static POLICY: OnceLock<Policy> = OnceLock::new();
pub(super) fn configure(api: &HostApiV0) -> Result<(), String> {
    let dim = |k, d| api.config_i64(k, d).clamp(0, 32768) as i32;
    let filter = match api
        .config_string(c"gpu.textureDownsampleFilter", 32)
        .as_deref()
    {
        Some("nearest") => Filter::Nearest,
        None | Some("box") => Filter::Box,
        Some(_) => {
            eprintln!("[GLES] unknown textureDownsampleFilter; using box");
            Filter::Box
        }
    };
    let p = Policy {
        rgba: dim(
            c"gpu.textureMaxDimRGBA8",
            i64::from(dim(c"gpu.textureMaxDim", 0)),
        ),
        etc: dim(c"gpu.textureMaxDimETC2", 0),
        astc: dim(c"gpu.textureMaxDimASTC", 0),
        filter,
        stats: api.config_bool(c"gpu.textureCapsStats", false),
        skip_narrow: api.config_bool(c"gpu.textureCapsSkipNarrow", true),
        skip_display: api.config_bool(c"gpu.textureCapsSkipDisplaySized", true),
    };
    if POLICY.get_or_init(|| p.clone()) != &p {
        return Err("texture policy already differs".into());
    }
    eprintln!(
        "[GLES] texture policy: RGBA8={} ETC2={} ASTC={} filter={:?} stats={}",
        p.rgba, p.etc, p.astc, p.filter, p.stats
    );
    Ok(())
}
pub(super) fn policy() -> &'static Policy {
    POLICY.get_or_init(Default::default)
}
pub(super) fn enabled() -> bool {
    POLICY
        .get()
        .is_some_and(|p| p.rgba > 0 || p.etc > 0 || p.astc > 0 || p.stats)
}
pub(super) fn raw(format: u32) -> bool {
    matches!(format, 0x8058 | 0x8051 | 0x1908 | 0x1907 | 0x8c41 | 0x8c43)
}
pub(super) fn block(format: u32) -> Option<(i32, i32, i32)> {
    match format {
        0x9270 | 0x9271 | 0x9274..=0x9277 => Some((4, 4, 8)),
        0x9272 | 0x9273 | 0x9278 | 0x9279 => Some((4, 4, 16)),
        0x93b0..=0x93bd | 0x93d0..=0x93dd => {
            let index = if format >= 0x93d0 {
                format - 0x93d0
            } else {
                format - 0x93b0
            } as usize;
            let sizes = [
                (4, 4),
                (5, 4),
                (5, 5),
                (6, 5),
                (6, 6),
                (8, 5),
                (8, 6),
                (8, 8),
                (10, 5),
                (10, 6),
                (10, 8),
                (10, 10),
                (12, 10),
                (12, 12),
            ];
            let (w, h) = sizes[index];
            Some((w, h, 16))
        }
        _ => None,
    }
}
pub(super) fn mip_count(w: i32, h: i32) -> i32 {
    if w <= 0 || h <= 0 {
        0
    } else {
        32 - (w.max(h) as u32).leading_zeros() as i32
    }
}
fn surface(w: i32, h: i32, sw: i32, sh: i32, near: bool) -> bool {
    if (w, h) == (sw, sh) {
        return true;
    }
    near && w <= sw
        && h <= sh
        && i64::from(w) * 10 >= i64::from(sw) * 9
        && i64::from(h) * 10 >= i64::from(sh) * 9
        && (i64::from(w) * i64::from(sh) - i64::from(h) * i64::from(sw)).abs() * 100
            <= i64::from(sw) * i64::from(sh) * 2
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub original: (i32, i32),
    pub size: (i32, i32),
    pub original_levels: i32,
    pub levels: i32,
    pub bias: i32,
    pub format: u32,
    pub immutable: bool,
}
impl Policy {
    pub fn cap(&self, format: u32) -> i32 {
        if raw(format) {
            self.rgba
        } else if (0x9270..=0x9279).contains(&format) {
            self.etc
        } else if block(format).is_some() {
            self.astc
        } else {
            0
        }
    }
    pub fn plan(
        &self,
        format: u32,
        w: i32,
        h: i32,
        levels: i32,
        immutable: bool,
        d: RenderDimensions,
    ) -> Option<Plan> {
        // Refuse invalid or incomplete allocation descriptions before changing the guest call.
        if w <= 0 || h <= 0 || w > 32768 || h > 32768 || levels <= 0 || levels > mip_count(w, h) {
            return None;
        }
        let cap = self.cap(format);
        let long = w.max(h);
        let short = w.min(h);
        if cap <= 0
            || long <= cap
            || (self.skip_narrow && (short <= 32 || i64::from(long) > 4 * i64::from(short)))
        {
            return None;
        }
        if self.skip_display
            && (surface(w, h, d.width, d.height, raw(format) && levels == 1)
                || surface(
                    w,
                    h,
                    d.output_width,
                    d.output_height,
                    raw(format) && levels == 1,
                ))
        {
            return None;
        }
        if raw(format) {
            // Never grow a dimension even when the configured cap is below four pixels.
            let resize = |v: i32| {
                ((i64::from(v) * i64::from(cap) / i64::from(long)) as i32 / 4 * 4)
                    .max(4)
                    .min(v)
            };
            let size = (resize(w), resize(h));
            if size == (w, h) {
                return None;
            }
            Some(Plan {
                original: (w, h),
                size,
                original_levels: levels,
                levels: levels.min(mip_count(size.0, size.1)),
                bias: 0,
                format,
                immutable,
            })
        } else if immutable && block(format).is_some() {
            let mut size = (w, h);
            let mut bias = 0;
            while size.0.max(size.1) > cap && bias + 1 < levels {
                size = (1.max(size.0 / 2), 1.max(size.1 / 2));
                bias += 1;
            }
            // With no usable mip, forward the original allocation and retain its base image.
            (bias > 0).then_some(Plan {
                original: (w, h),
                size,
                original_levels: levels,
                levels: levels - bias,
                bias,
                format,
                immutable,
            })
        } else {
            None
        }
    }
}
impl Plan {
    pub fn old_size(self, level: i32) -> Option<(i32, i32)> {
        if !(0..mip_count(self.original.0, self.original.1)).contains(&level) {
            None
        } else {
            Some((
                (self.original.0 >> level).max(1),
                (self.original.1 >> level).max(1),
            ))
        }
    }
    pub fn level_size(self, level: i32) -> Option<(i32, i32)> {
        if !(0..self.levels).contains(&level) {
            None
        } else {
            Some(((self.size.0 >> level).max(1), (self.size.1 >> level).max(1)))
        }
    }
    /// Validate guest bounds before resampling, using independent horizontal/vertical ratios.
    pub fn region(
        self,
        level: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Option<(i32, i32, i32, i32)> {
        let (ow, oh) = self.old_size(level)?;
        let (nw, nh) = self.level_size(level)?;
        if x < 0 || y < 0 || w <= 0 || h <= 0 || x.checked_add(w)? > ow || y.checked_add(h)? > oh {
            return None;
        }
        let scale = |v: i32, n: i32, o: i32| (i64::from(v) * i64::from(n) / i64::from(o)) as i32;
        let nx = scale(x, nw, ow);
        let ny = scale(y, nh, oh);
        let ex = scale(x + w, nw, ow).max(nx + 1).min(nw);
        let ey = scale(y + h, nh, oh).max(ny + 1).min(nh);
        (nx < nw && ny < nh).then_some((nx, ny, ex - nx, ey - ny))
    }
    pub fn bytes(self, original: bool) -> u64 {
        let (mut w, mut h) = if original { self.original } else { self.size };
        let levels = if !self.immutable {
            1
        } else if original {
            self.original_levels
        } else {
            self.levels
        };
        let mut total = 0;
        for _ in 0..levels {
            total += if let Some((bw, bh, b)) = block(self.format) {
                ((w + bw - 1) / bw) as u64 * ((h + bh - 1) / bh) as u64 * b as u64
            } else {
                w as u64
                    * h as u64
                    * if matches!(self.format, 0x8051 | 0x1907 | 0x8c41) {
                        3
                    } else {
                        4
                    }
            };
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        total
    }
}
pub(super) const PARAMETERS: [u32; 4] = [0x813c, 0x813d, 0x813a, 0x813b];
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Param {
    Int(i32),
    Float(f32),
}
impl Param {
    pub fn integer(self) -> i32 {
        match self {
            Self::Int(v) => v,
            Self::Float(v) => v.round() as i32,
        }
    }
    pub fn float(self) -> f32 {
        match self {
            Self::Int(v) => v as f32,
            Self::Float(v) => v,
        }
    }
}
pub(super) fn parameter(plan: Plan, name: u32, value: Param) -> Param {
    if matches!(name, 0x813c | 0x813d) {
        let v = value.integer();
        if v >= 0 {
            return Param::Int(
                (i64::from(v) - i64::from(plan.bias)).clamp(0, i64::from(plan.levels - 1)) as i32,
            );
        }
    } else if matches!(name, 0x813a | 0x813b) && value.float().is_finite() {
        return Param::Float(value.float() - plan.bias as f32);
    }
    value
}

/// Only inspect CPU bytes for combinations supported by these normalized color formats.
/// Other combinations retain native validation; unsized floating-point textures are forwarded.
pub(super) fn upload_compatible(internal: u32, format: u32, kind: u32) -> bool {
    let rgb = matches!(internal, 0x8051 | 0x1907 | 0x8c41);
    let rgba = matches!(internal, 0x8058 | 0x1908 | 0x8c43);
    if (rgb && format == 0x1907) || (rgba && format == 0x1908) {
        kind == 0x1401
            || (internal == 0x1907 && kind == 0x8363)
            || (internal == 0x1908 && matches!(kind, 0x8033 | 0x8034))
    } else {
        false
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Unpack {
    pub alignment: i32,
    pub row: i32,
    pub rows: i32,
    pub pixels: i32,
    pub buffer: i32,
    pub row_controls: bool,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub start: usize,
    pub stride: usize,
    pub span: usize,
    pub bpp: usize,
    pub byte_channels: bool,
}
impl Layout {
    pub fn new(w: i32, h: i32, format: u32, kind: u32, u: Unpack) -> Option<Self> {
        if w <= 0
            || h <= 0
            || !matches!(u.alignment, 1 | 2 | 4 | 8)
            || u.row < 0
            || u.rows < 0
            || u.pixels < 0
        {
            return None;
        }
        let comps = match format {
            0x1908 | 0x80e1 => 4,
            0x1907 => 3,
            0x1903 | 0x1906 | 0x1909 => 1,
            0x8227 | 0x190a => 2,
            _ => return None,
        };
        let (bpp, byte_channels) = match kind {
            0x1401 => (comps, true),
            0x1403 | 0x140b => (comps * 2, false),
            0x1405 | 0x1406 => (comps * 4, false),
            0x8363 if format == 0x1907 => (2, false),
            0x8033 | 0x8034 if format == 0x1908 => (2, false),
            0x8368 if format == 0x1908 => (4, false),
            _ => return None,
        };
        let row = if u.row == 0 {
            w as usize
        } else {
            u.row as usize
        };
        // Row length controls stride, not the number of pixels transferred. Legal
        // overlapping rows and pixel skips are covered by the full checked span.
        let a = u.alignment as usize;
        let stride = row.checked_mul(bpp)?.checked_add(a - 1)? & !(a - 1);
        let start = (u.rows as usize)
            .checked_mul(stride)?
            .checked_add((u.pixels as usize).checked_mul(bpp)?)?;
        let span = start
            .checked_add((h as usize - 1).checked_mul(stride)?)?
            .checked_add((w as usize).checked_mul(bpp)?)?;
        (span <= MAX_BYTES).then_some(Self {
            start,
            stride,
            span,
            bpp,
            byte_channels,
        })
    }
}
pub(super) fn resample(
    source: &[u8],
    l: Layout,
    w: i32,
    h: i32,
    nw: i32,
    nh: i32,
    filter: Filter,
) -> Option<Vec<u8>> {
    if source.len() < l.span || nw <= 0 || nh <= 0 || nw > w || nh > h {
        return None;
    }
    let len = (nw as usize).checked_mul(nh as usize)?.checked_mul(l.bpp)?;
    if len > MAX_BYTES {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(len).ok()?;
    out.resize(len, 0);
    for dy in 0..nh as usize {
        for dx in 0..nw as usize {
            let x = dx * w as usize / nw as usize;
            let y = dy * h as usize / nh as usize;
            let dest = (dy * nw as usize + dx) * l.bpp;
            if filter == Filter::Nearest || !l.byte_channels {
                let pos = l.start + y * l.stride + x * l.bpp;
                out[dest..dest + l.bpp].copy_from_slice(&source[pos..pos + l.bpp]);
            } else {
                let ex = ((dx + 1) * w as usize / nw as usize).max(x + 1);
                let ey = ((dy + 1) * h as usize / nh as usize).max(y + 1);
                let count = ((ex - x) * (ey - y)) as u64;
                let mut sums = [0u64; 16];
                for sy in y..ey {
                    for sx in x..ex {
                        let pos = l.start + sy * l.stride + sx * l.bpp;
                        for ch in 0..l.bpp {
                            sums[ch] += u64::from(source[pos + ch]);
                        }
                    }
                }
                for ch in 0..l.bpp {
                    out[dest + ch] = (sums[ch] / count) as u8;
                }
            }
        }
    }
    Some(out)
}
#[cfg(test)]
#[path = "../tests/unit/texture_policy.rs"]
mod tests;
