//! INPUT:  shared render geometry resolver
//! OUTPUT:  exact/percentage sizing, invalid-policy and coordinate-boundary regressions
//! POS:    tests the same geometry contract used by the native and Java facades
use super::*;

#[test]
fn output_and_render_geometry_are_distinct_and_exact_sizes_take_precedence() {
    for (percent, expected) in [(100, (960,720)),(75,(720,540)),(50,(480,360))] {
        let size=RenderDimensions::resolve(960,720,0,0,percent,true).unwrap();
        assert_eq!((size.width,size.height),expected);
        assert_eq!(size.point(960,720),expected);
        assert_eq!(size.point(480,360),(expected.0/2,expected.1/2));
    }
    let exact=RenderDimensions::resolve(960,720,512,384,75,false).unwrap();
    assert_eq!((exact.width,exact.height),(512,384));
    assert!(exact.scaled());
    assert!(!exact.sharp);
    assert_eq!(RenderDimensions::resolve(1,1,0,0,50,true).unwrap().width,1);
}

#[test]
fn malformed_policies_are_rejected_and_coordinates_never_overflow() {
    for (w,h,p) in [(0,384,75),(512,0,75),(-1,384,75),(32769,384,75),(0,0,74)] {
        assert!(RenderDimensions::resolve(960,720,w,h,p,true).is_err());
    }
    assert!(RenderDimensions::resolve(0,720,0,0,75,true).is_err());
    let size=RenderDimensions::resolve(1,1,32768,32768,100,true).unwrap();
    assert_eq!(size.point(i32::MAX,i32::MIN),(i32::MAX,i32::MIN));
}
