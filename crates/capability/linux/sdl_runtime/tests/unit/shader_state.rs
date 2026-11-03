//! INPUT:  pending-source and successful executable lifecycle transitions
//! OUTPUT:  shared-group/recompile/delete/binary isolation assertions
//! POS:    checks skip metadata independently of native shader/program names
use super::*;
#[test]
fn pending_and_failed_compiles_never_mutate_the_linked_executable() {
    let mut s = State::default();
    s.source(1, 10, false);
    s.compiled(1, 10);
    s.linked(1, 20, &[10]);
    assert_eq!(s.program(1, 20), Some(false));
    s.source(1, 10, true);
    s.linked(1, 20, &[10]);
    assert_eq!(s.program(1, 20), Some(false));
    s.compiled(1, 10);
    assert_eq!(s.program(1, 20), Some(false));
    s.linked(1, 20, &[10]);
    assert_eq!(s.program(1, 20), Some(true));
    s.source(1, 10, false);
    assert_eq!(s.program(1, 20), Some(true));
    s.set_program(1, 20, false);
    assert_eq!(s.program(1, 20), Some(false));
}
#[test]
fn shared_groups_keep_deferred_shader_metadata_and_isolate_reused_names() {
    let mut s = State::default();
    s.add_context(2, 1);
    s.source(2, 10, true);
    s.compiled(2, 10);
    s.linked(1, 20, &[10]);
    assert_eq!(s.program(2, 20), Some(true));
    s.source(3, 10, false);
    s.compiled(3, 10);
    s.linked(3, 20, &[10]);
    assert_eq!(s.program(3, 20), Some(false));
    s.forget_context(2);
    assert_eq!(s.program(1, 20), Some(true));
    s.shader_binary(1, 10);
    s.linked(1, 20, &[10]);
    assert_eq!(s.program(1, 20), Some(false));
    s.forget_context(1);
    assert_eq!(s.program(1, 20), None);
    assert_eq!(s.program(3, 20), Some(false));
}

#[test]
fn context_handle_reuse_and_shared_relink_preserve_each_bound_executable() {
    let mut s = State::default();
    s.add_context(2, 1);
    s.source(1, 10, false);
    s.compiled(1, 10);
    s.linked(1, 20, &[10]);
    assert!(!s.binding(2, 20, true));
    s.source(1, 10, true);
    s.compiled(1, 10);
    s.linked(1, 20, &[10]);
    assert!(
        !s.binding(2, 20, false),
        "another context's relink cannot replace a saved binding without UseProgram"
    );
    assert!(s.binding(2, 20, true));
    s.forget_context(1);
    s.add_context(1, 0);
    assert!(
        !s.binding(1, 20, true),
        "reused native pointer gets a fresh share-group identity"
    );
    assert!(s.binding(2, 20, false));
    s.delete_program(2, 20);
    assert!(
        s.binding(2, 20, false),
        "deferred deletion keeps a bound executable"
    );
    assert!(!s.binding(2, 0, true));
}

#[test]
fn optional_peer_propagation_never_keeps_a_discard_skip_after_relinking_to_ordinary_code() {
    let mut s = State::default();
    s.add_context(2, 1);
    s.source(1, 10, true);
    s.compiled(1, 10);
    s.linked(1, 20, &[10]);
    assert!(s.binding(2, 20, true));
    s.source(1, 10, false);
    s.compiled(1, 10);
    s.linked(1, 20, &[10]);
    assert!(!s.binding(2,20,false),"peer must forward regardless of whether its driver keeps the old executable or propagates the new one");
}
