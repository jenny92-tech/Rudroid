//! INPUT:  compiled display-backend feature bits from a validated same-kit capability
//! OUTPUT:  bounded, canonical capability tokens for launcher negotiation
//! POS:    cold graphics support contract; does not probe a device, window or game
#![forbid(unsafe_code)]

pub const SDL2: u64 = 1;
pub const GLES: u64 = 1 << 1;
pub const GLES_SCALE: u64 = 1 << 2;
pub const GLES_SHARP: u64 = 1 << 3;
pub const GLES_SHADER: u64 = 1 << 4;
pub const GLES_TEXTURE: u64 = 1 << 5;
pub const VULKAN: u64 = 1 << 6;
pub const VULKAN_SCALE: u64 = 1 << 7;
pub const VULKAN_LINEAR: u64 = 1 << 8;

pub fn tokens(bits: u64) -> Result<String, &'static str> {
    let features = [(SDL2,"sdl2"),(GLES,"gles"),(GLES_SCALE,"gles-scale"),
        (GLES_SHARP,"gles-sharp"),(GLES_SHADER,"gles-shader"),(GLES_TEXTURE,"gles-texture"),
        (VULKAN,"vulkan"),(VULKAN_SCALE,"vulkan-scale"),(VULKAN_LINEAR,"vulkan-linear")];
    if bits & !features.iter().fold(0, |all,(bit,_)|all|bit) != 0 {
        return Err("unknown graphics feature bits");
    }
    if bits & (GLES_SCALE|GLES_SHARP|GLES_SHADER|GLES_TEXTURE) != 0 && bits&GLES == 0
        || bits & (VULKAN_SCALE|VULKAN_LINEAR) != 0 && bits&VULKAN == 0
        || bits & (GLES|VULKAN) != 0 && bits&SDL2 == 0 {
        return Err("graphics features lack their owning backend");
    }
    Ok(features.iter().filter(|(bit,_)|bits&bit!=0).map(|(_,name)|*name).collect::<Vec<_>>().join(","))
}

#[cfg(test)]
#[path = "../tests/unit/graphics_features.rs"]
mod tests;
