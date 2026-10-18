//! INPUT:  texture names in independent and shared native object namespaces
//! OUTPUT:  deletion, reuse, namespace retirement and bounded metadata assertions
//! POS:    adversarial checks for safe texture metadata ownership
use super::*;
fn plan() -> Plan {
    Plan {
        original: (128, 128),
        size: (64, 64),
        original_levels: 8,
        levels: 7,
        bias: 1,
        format: 0x9274,
        immutable: true,
    }
}
#[test]
fn same_texture_name_in_different_groups_never_shares_a_plan() {
    let mut s = State::default();
    s.set((1, 7), Some(plan()));
    assert_eq!(s.get((1, 7)), Some(plan()));
    assert_eq!(s.get((2, 7)), None);
    s.set((2, 7), Some(plan()));
    s.set((1, 7), None);
    assert_eq!(s.get((1, 7)), None);
    assert_eq!(s.get((2, 7)), Some(plan()));
    s.retire(2);
    assert_eq!(s.get((2, 7)), None);
    s.set((3, 7), None);
    assert_eq!(s.get((3, 7)), None);
}
#[test]
fn concurrent_distinct_allocations_reserve_capacity_before_touching_the_gpu() {
    let mut s = State::default();
    for n in 1..=8191 {
        s.set((1, n), Some(plan()));
    }
    assert!(s.reserve((1, 8192)));
    assert!(!s.reserve((1, 8193)));
    assert!(!s.reserve((1, 8192)));
    s.release((1, 8192));
    s.set((1, 8192), Some(plan()));
    assert_eq!(s.get((1, 8192)), Some(plan()));
}
#[test]
fn metadata_capacity_never_resizes_an_unrecordable_texture() {
    let mut s = State::default();
    for n in 1..=8192 {
        s.set((1, n), Some(plan()));
    }
    assert!(!s.room((1, 8193)));
    assert!(s.room((1, 1)));
    assert!(!s.room((0, 1)));
    assert!(!s.room((1, 0)));
    s.set((1, 8193), Some(plan()));
    assert_eq!(s.get((1, 8193)), None);
    s.set((1, 1), None);
    assert!(s.room((1, 8193)));
}
