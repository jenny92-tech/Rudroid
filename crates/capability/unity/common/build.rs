//! INPUT:  private AAPCS64 managed-hook gateway assembly
//! OUTPUT:  linked `rudroid_managed_gateway` archive for scalar/reference slot dispatch
//! POS:    compiles the Unity managed gateway once per plain-library link copy

fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_managed_gateway"),
        sources: &["src/managed_gateway.S"],
        headers: &[],
        extra_flags: &[],
        link: &[],
    });
}
