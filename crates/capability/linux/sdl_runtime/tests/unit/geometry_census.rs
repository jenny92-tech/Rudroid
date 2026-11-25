use super::*;

fn key(object: u32) -> Key {
    Key { op: Op::MapRange, role: Role::ZeroOffsetWriteMap, vao: 3, object,
          target: ELEMENT, detail: 0x36, warm: true }
}

#[test]
fn deletion_does_not_certify_a_reused_numeric_name() {
    let mut state = State::default();
    let row = key(7);
    state.observe(row);
    state.retire(1);
    state.retire(2);
    state.observe(row);
    let observation = &state.objects[&row];
    assert_eq!(observation.count, 2);
    assert_eq!(observation.first, Epoch::default());
    assert_eq!(observation.last, Epoch { buffer: 1, vao: 1 });
}

#[test]
fn attribute_slots_share_a_row_without_losing_the_observed_set() {
    let mut state = State::default();
    let mut row = Key { role: Role::DrawVertexSample, detail: 3, ..key(12) };
    state.observe(row);
    row.detail = 31;
    state.observe(row);
    row.detail = 0;
    assert_eq!(state.objects[&row].count, 2);
    assert_eq!(state.objects[&row].slots, (1 << 31) | (1 << 3));
}

#[test]
fn limits_preserve_existing_rows_and_count_untracked_lookups_separately() {
    let mut state = State::default();
    for name in 0..OBJECT_LIMIT[1] as u32 + 2 { state.observe(key(name)); }
    state.observe(key(0));
    let mut cold = key(0);
    cold.warm = false;
    state.observe(cold);
    assert_eq!(state.objects.len(), OBJECT_LIMIT[1] + 1);
    assert_eq!(state.objects[&key(0)].count, 2);
    assert_eq!(state.object_overflow, [0, 2]);
    for n in 0..RAW_LIMIT + 2 {
        state.raw(format!("glProc{n}").as_bytes(), Route::ElfSource, true);
    }
    state.raw(b"glProc0", Route::EglProc, false);
    assert_eq!(state.raw.len(), RAW_LIMIT);
    assert_eq!(state.raw[b"glProc0".as_slice()], [[0, 1], [1, 0]]);
    assert_eq!(state.raw_overflow, 2);
    assert!(state.covered.is_empty());
}
