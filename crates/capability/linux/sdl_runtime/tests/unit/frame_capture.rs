//! INPUT: capture schedule on an explicitly diagnostic build
//! OUTPUT: due points across swaps and elapsed time
//! POS: unit coverage for the GLES frame probe
use super::*;
#[test]
fn diagnostic_captures_are_bounded_and_independent_of_frame_rate() {
    let capture = FrameCapture {
        path: "unused.ppm".into(),
        points: vec![
            CapturePoint::ElapsedMs(50_000),
            CapturePoint::ElapsedMs(56_000),
        ],
        started: Some(std::time::Instant::now()),
        completed: AtomicU32::new(0),
    };
    assert_eq!(capture.take_due(3000, 49_999), None);
    assert_eq!(capture.take_due(3001, 50_005), Some(0));
    assert_eq!(capture.take_due(3002, 50_010), None);
    assert_eq!(capture.take_due(3300, 56_002), Some(1));
    assert_eq!(capture.take_due(3301, 90_000), None);
    let legacy = FrameCapture {
        path: "unused.ppm".into(),
        points: vec![CapturePoint::Swap(120)],
        started: None,
        completed: AtomicU32::new(0),
    };
    assert_eq!(legacy.take_due(119, 99_000), None);
    assert_eq!(legacy.take_due(120, 99_000), Some(0));
    assert_eq!(legacy.take_due(120, 99_000), None);
    assert_eq!(legacy.take_due(121, 99_000), None);
}
