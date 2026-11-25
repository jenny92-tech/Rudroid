use super::*;

#[test]
fn scheduler_fields_require_complete_kernel_counters() {
    assert_eq!(counters(b"4771 214 9\n"), Some([4771, 214, 9]));
    for invalid in [b"".as_slice(), b"4771 214 9", b"4771 -1 9\n", b"4771 214\n", b"4771 214 9 extra\n"] {
        assert_eq!(counters(invalid), None);
    }
}

#[test]
fn scheduler_pairs_reject_changing_identity_and_distinguish_unobserved_delay() {
    let start = Read { tid: 7, handle: 2, start_ticks: 5, before: 1, after: 2, counters: Some([2, 0, 4]), status: "ok" };
    let end = Read { before: 5, after: 6, counters: Some([4, 0, 5]), ..start };
    assert_eq!(start.delta(end), (Some([2, 0, 1]), "run-delay-unobserved"));
    let awake = Read { counters: Some([4, 1, 5]), ..end };
    assert_eq!(start.delta(awake), (Some([2, 1, 1]), "ok"));
    assert_eq!(start.delta(Read { handle: 3, ..end }), (None, "identity"));
    assert_eq!(start.delta(Read { start_ticks: 6, ..end }), (None, "identity"));
    assert_eq!(start.delta(Read::missing("lost")), (None, "missing"));
    assert_eq!(start.delta(Read { counters: Some([1, 0, 4]), ..end }), (None, "counter-regressed"));
    assert_eq!(start.delta(Read { before: 1, ..end }), (None, "clock"));
}
