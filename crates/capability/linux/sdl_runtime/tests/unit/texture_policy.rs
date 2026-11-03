//! INPUT:  safe allocation plans and padded pixel buffers
//! OUTPUT:  exclusions, bounded mip rebasing, byte strides and filter regressions
//! POS:    checks texture decisions without native pointers or a GPU
use super::*;
fn dimensions() -> RenderDimensions {
    RenderDimensions::resolve(960, 720, 640, 480, 100, true).unwrap()
}
fn caps() -> Policy {
    Policy {
        rgba: 64,
        etc: 64,
        astc: 64,
        ..Default::default()
    }
}
#[test]
fn plans_keep_render_targets_luts_and_direct_compressed_images() {
    let p = caps();
    let d = dimensions();
    for (w, h, l) in [
        (640, 480, 1),
        (960, 720, 5),
        (608, 456, 1),
        (1024, 16, 1),
        (256, 33, 1),
    ] {
        assert!(p.plan(0x8058, w, h, l, true, d).is_none());
    }
    assert!(p.plan(0x9274, 128, 128, 1, true, d).is_none());
    assert!(p.plan(0x9274, 128, 128, 8, false, d).is_none());
    assert!(p.plan(0x8058, 128, 128, 9, true, d).is_none());
    let q = p.plan(0x8058, 128, 96, 8, true, d).unwrap();
    assert_eq!(q.size, (64, 48));
    assert_eq!(q.levels, 7);
    assert_eq!(q.region(0, 120, 88, 8, 8), Some((60, 44, 4, 4)));
    assert!(q.region(0, -1, 0, 10, 10).is_none());
    assert!(q.region(0, 120, 0, 10, 10).is_none());
}
#[test]
fn compressed_mips_are_rebased_only_inside_known_storage() {
    for f in [0x9274, 0x9278, 0x93b0, 0x93bd, 0x93d7] {
        let p = caps().plan(f, 256, 128, 9, true, dimensions()).unwrap();
        assert_eq!((p.size, p.bias, p.levels), ((64, 32), 2, 7));
        assert_eq!(p.level_size(6), Some((1, 1)));
        assert_eq!(p.level_size(7), None);
        assert!(p.bytes(true) > p.bytes(false));
    }
    assert_eq!(block(0x93bd), Some((12, 12, 16)));
    assert_eq!(block(0x9274), Some((4, 4, 8)));
}
#[test]
fn filters_respect_padding_skips_and_use_nearest_for_packed_components() {
    let u = Unpack {
        alignment: 8,
        row: 6,
        rows: 1,
        pixels: 1,
        buffer: 0,
        row_controls: true,
    };
    let l = Layout::new(4, 4, 0x1908, 0x1401, u).unwrap();
    assert_eq!((l.stride, l.start, l.span), (24, 28, 116));
    let mut src = vec![222; l.span];
    for y in 0..4 {
        for x in 0..4 {
            let i = l.start + y * l.stride + x * 4;
            src[i..i + 4].copy_from_slice(&[(x * 20) as u8, (y * 20) as u8, 100, 255]);
        }
    }
    assert_eq!(
        resample(&src, l, 4, 4, 2, 2, Filter::Box).unwrap(),
        vec![10, 10, 100, 255, 50, 10, 100, 255, 10, 50, 100, 255, 50, 50, 100, 255]
    );
    assert_eq!(
        &resample(&src, l, 4, 4, 2, 2, Filter::Nearest).unwrap()[..4],
        &[0, 0, 100, 255]
    );
    assert!(resample(&src[..115], l, 4, 4, 2, 2, Filter::Box).is_none());
    let packed = Layout::new(
        4,
        4,
        0x1907,
        0x8363,
        Unpack {
            row: 0,
            rows: 0,
            pixels: 0,
            ..u
        },
    )
    .unwrap();
    let src: Vec<_> = (0..32).collect();
    assert_eq!(
        resample(&src, packed, 4, 4, 2, 2, Filter::Box).unwrap(),
        vec![0, 1, 4, 5, 16, 17, 20, 21]
    );
}
#[test]
fn invalid_unpack_spans_are_rejected_before_allocation() {
    let u = Unpack {
        alignment: 4,
        row: 0,
        rows: 0,
        pixels: 0,
        buffer: 0,
        row_controls: true,
    };
    for v in [
        Unpack { alignment: 3, ..u },
        Unpack { row: -1, ..u },
        Unpack {
            rows: i32::MAX,
            ..u
        },
        Unpack { pixels: -1, ..u },
    ] {
        assert!(Layout::new(4, 4, 0x1908, 0x1401, v).is_none());
    }
    let overlap = Layout::new(
        4,
        4,
        0x1908,
        0x1401,
        Unpack {
            row: 3,
            pixels: 1,
            ..u
        },
    )
    .unwrap();
    assert_eq!((overlap.start, overlap.stride, overlap.span), (4, 12, 56));
    assert!(Layout::new(i32::MAX, i32::MAX, 0x1908, 0x1401, u).is_none());
    assert!(Layout::new(4, 4, 0x1908, 0xffff, u).is_none());
}
#[test]
fn mip_parameters_keep_exact_integer_precision_and_rebase_lod() {
    let p = caps()
        .plan(0x9274, 128, 128, 8, true, dimensions())
        .unwrap();
    assert_eq!(parameter(p, 0x813c, Param::Int(1)), Param::Int(0));
    assert_eq!(parameter(p, 0x813d, Param::Int(16777217)), Param::Int(6));
    assert_eq!(parameter(p, 0x813c, Param::Int(-1)), Param::Int(-1));
    assert_eq!(parameter(p, 0x813a, Param::Float(1.75)), Param::Float(0.75));
    assert_eq!(Param::Int(16777217).integer(), 16777217);
    assert!(!upload_compatible(0x8058, 0x1908, 0x1406));
    assert!(upload_compatible(0x8058, 0x1908, 0x1401));
}
