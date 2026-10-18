//! INPUT:  physical-output and native-render dimensions for an SDL Wayland viewport
//! OUTPUT:  input coordinate regressions for lower and exact Vulkan resolutions
//! POS:    prevents applying output-to-render scaling twice after SDL transforms input
use super::*;

#[test]
fn native_viewport_input_coordinates_are_already_in_render_space() {
    use rudroid_common::display_dimensions::RenderDimensions;
    for (width,height,percent,expected) in [(0,0,100,(960,720)),(0,0,75,(720,540)),
        (0,0,50,(480,360)),(640,480,100,(640,480))] {
        let display = RenderDimensions::resolve(960,720,width,height,percent,false).unwrap();
        assert_eq!((display.output_width,display.output_height),(960,720));
        assert_eq!((display.width,display.height),expected);
        let input = vulkan_input_dimensions(display);
        assert!(!input.scaled());
        assert_eq!(input.point(expected.0-1,expected.1-1),(expected.0-1,expected.1-1));
        assert_eq!(input.point(expected.0/2,expected.1/2),(expected.0/2,expected.1/2));
    }
}
