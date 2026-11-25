use super::*;

#[test]
fn distinct_resolver_results_keep_their_exact_native_entry() {
    let trace = Trace::new();
    let key = b"glMapBufferRange";
    assert_eq!(trace.resolve(key, Route::ElfSource, 0), 0);
    let wrapper = trace.resolve(key, Route::ElfSource, 7);
    assert_eq!(wrapper, Op::MapRange.wrapper());
    assert_eq!(trace.resolve(key, Route::EglProc, 7), wrapper);
    assert_eq!(trace.resolve(key, Route::EglProc, 11), 11);
    assert_eq!(
        trace.resolve(b"glMapBufferRangeEXT", Route::EglProc, 11),
        Op::MapRangeExt.wrapper()
    );
    assert_eq!(trace.resolve(b"glGetError", Route::EglProc, 19), 19);
    let stats = trace.stats.lock().unwrap();
    assert_eq!(stats.lookups[Op::MapRange as usize], [[1, 1, 0], [0, 1, 1]]);
    assert_eq!(stats.lookups[Op::MapRangeExt as usize], [[0, 0, 0], [0, 1, 0]]);
}

#[test]
fn exact_flag_buckets_are_capped_without_losing_existing_counts() {
    let mut stats = Stats::default();
    let key = |access| Key {
        operation: Op::MapRange,
        binding: Binding::Primary,
        after_120_swaps: true,
        args: Arguments::range(0x8f37, access, 0, 4096),
    };
    for access in 0..(BUCKET_LIMIT as u32 + 3) {
        stats.record(key(access));
    }
    stats.record(key(0x36));
    assert_eq!(stats.calls.len(), BUCKET_LIMIT);
    assert_eq!(stats.overflow, 3);
    assert_eq!(stats.calls[&key(0x36)], 2);
    assert_eq!(key(0x36).args.offset, Span::Zero);
    assert_eq!(key(0x36).args.length, Span::Positive);
}
