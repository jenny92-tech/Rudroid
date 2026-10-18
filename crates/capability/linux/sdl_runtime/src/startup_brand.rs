//! INPUT:  embedded Jenny92 half-body character artwork and measured drawable dimensions
//! OUTPUT:  bounded startup vertices and immutable RGBA8 mascot pixels
//! POS:    lays out the one-shot RUDROID loading frame without native calls or runtime image decoding

#![forbid(unsafe_code)]

pub(super) const MASCOT_SIDE: i32 = 300;
pub(super) const MASCOT: &[u8; 300 * 300 * 4] = include_bytes!("../assets/mascot.rgba");
pub(super) const TITLE: &str = "RUDROID";
const SUBTITLE: &str = "UNITY LOADER";

/// One attribute carries clip coordinates and either image UVs or a solid
/// color selector. The fresh startup context submits this batch only once.
pub(super) fn vertices(width: u32, height: u32) -> Vec<[f32; 4]> {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let unit = w.min(h);
    let mut out = Vec::with_capacity(2048);
    out.extend([
        [-1.0, -1.0, -1.0, 0.0],
        [3.0, -1.0, -1.0, 0.0],
        [-1.0, 3.0, -1.0, 0.0],
    ]);
    let side = unit * 0.55;
    // Retain the artwork's transparent border for padding from the screen
    // edge. Verification samples the background outside all brand content.
    quad(&mut out, w, h, [(w - side) * 0.5, h * 0.5 + unit * 0.06 - side, side, side], None);
    text(&mut out, w, h, TITLE, h * 0.5 + unit * 0.055, unit / 100.0, -2.0);
    text(&mut out, w, h, SUBTITLE, h * 0.5 + unit * 0.175, unit / 220.0, -3.0);
    out
}

fn quad(out: &mut Vec<[f32; 4]>, w: f32, h: f32, rect: [f32; 4], solid: Option<f32>) {
    let [x, y, width, height] = rect;
    let point = |u: f32, v: f32| [
        (x + u * width) * 2.0 / w - 1.0,
        1.0 - (y + v * height) * 2.0 / h,
        solid.unwrap_or(u),
        if solid.is_some() { 0.0 } else { v },
    ];
    out.extend([point(0.0, 0.0), point(0.0, 1.0), point(1.0, 0.0),
        point(1.0, 0.0), point(0.0, 1.0), point(1.0, 1.0)]);
}

fn text(out: &mut Vec<[f32; 4]>, w: f32, h: f32, value: &str, top: f32, scale: f32, solid: f32) {
    let left = (w - (value.len() * 6 - 1) as f32 * scale) * 0.5;
    for (column, letter) in value.bytes().enumerate() {
        for (row, bits) in glyph(letter).into_iter().enumerate() {
            // Merge adjacent pixels into runs to keep the single batch small.
            let mut x = 0;
            while x < 5 {
                if bits & (1 << (4 - x)) == 0 { x += 1; continue; }
                let start = x;
                while x < 5 && bits & (1 << (4 - x)) != 0 { x += 1; }
                quad(out, w, h, [left + (column * 6 + start) as f32 * scale,
                    top + row as f32 * scale, (x - start) as f32 * scale, scale], Some(solid));
            }
        }
    }
}

fn glyph(letter: u8) -> [u8; 7] {
    match letter {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'I' => [31, 4, 4, 4, 4, 4, 31],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'N' => [17, 25, 25, 21, 19, 19, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        _ => [0; 7],
    }
}
