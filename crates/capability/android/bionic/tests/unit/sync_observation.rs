use super::*;

#[test]
fn anonymous_lifetimes_notifications_and_completion_are_separate() {
    let mut s=State::new();s.register(11);s.register(22);
    let old=s.objects[&11].id;
    let wait=s.begin("pthread_cond_wait",11,22,7,0x1004).unwrap();
    let rows=s.rows(Duration::ZERO);assert_eq!(rows.len(),1);
    assert_eq!((rows[0].object,rows[0].mutex,rows[0].notifications),(1,2,0));
    assert_eq!((rows[0].caller_ra,call_pc(rows[0].caller_ra)),(0x1004,Some(0x1000)));
    assert!(s.rows(Duration::ZERO).is_empty());
    let notify=s.begin("pthread_cond_signal",11,0,8,0).unwrap();s.end(notify,0);
    let rows=s.rows(Duration::ZERO);assert_eq!(rows.len(),1);
    assert_eq!((rows[0].initial,rows[0].notifications,rows[0].notifier),(0,1,8));
    assert_eq!(rows[0].caller_ra,0x1004);
    // A successful notification is not proof that the waiter returned.
    assert!(s.waits.contains_key(&7));
    wait.complete(); // end's metadata lock may be unavailable after this.
    assert!(s.rows(Duration::ZERO).is_empty());
    s.end(wait,0);
    s.objects.remove(&11);s.register(11);assert_ne!(s.objects[&11].id,old);
}

#[test]
fn failed_notifications_do_not_manufacture_wakes() {
    let mut s=State::new();s.register(11);
    let wait=s.begin("pthread_cond_wait",11,0,7,0).unwrap();
    let notify=s.begin("pthread_cond_signal",11,0,8,0).unwrap();s.end(notify,-1);
    let row=s.rows(Duration::ZERO).remove(0);
    assert_eq!((row.notifications,row.notifier),(0,0));
    s.end(wait,0);assert!(s.rows(Duration::ZERO).is_empty());
}

#[test]
fn old_call_completion_cannot_notify_a_reused_native_address() {
    let mut s=State::new();s.register(11);
    let old=s.begin("pthread_cond_signal",11,0,7,0).unwrap();let old_id=old.object_id;
    s.objects.remove(&11);s.register(11);
    assert_ne!(s.objects[&11].id,old_id);
    s.end(old,0);
    assert_eq!((s.objects[&11].notifications,s.objects[&11].notifier),(0,0));
}

#[test]
fn observation_budget_never_exceeds_the_explicit_row_limit() {
    let mut s=State::new();s.register(11);
    for _ in 0..RECORDS+3 {
        let wait=s.begin("pthread_cond_wait",11,0,7,0).unwrap();
        let rows=s.rows(Duration::ZERO);
        assert_eq!(rows.len(),usize::from(s.records<=RECORDS && !rows.is_empty()));
        s.end(wait,0);
    }
    assert_eq!(s.records,RECORDS);
}

#[test]
fn caller_metadata_is_not_reused_after_completion_or_new_object_generation() {
    let mut s=State::new();s.register(11);
    let old=s.begin("pthread_cond_wait",11,0,7,0x2004).unwrap();
    let old_id=old.object_id;
    assert_eq!(s.rows(Duration::ZERO).remove(0).caller_ra,0x2004);
    // A contended end may leave old bookkeeping after publishing completion.
    old.complete();
    s.objects.remove(&11);s.register(11);
    let current=s.begin("pthread_cond_timedwait",11,0,7,0x3008).unwrap();
    assert_ne!(current.object_id,old_id);
    s.end(old,0); // Old completion cannot clear or overwrite the new call.
    let row=s.rows(Duration::ZERO).remove(0);
    assert_eq!((row.object,row.caller_ra,call_pc(row.caller_ra)),(current.object_id,0x3008,Some(0x3004)));
    s.end(current,libc::ETIMEDOUT);
    assert!(s.rows(Duration::ZERO).is_empty());
}

#[test]
fn missing_or_unaligned_return_addresses_have_no_derived_call_pc() {
    for address in [0,1,2,3,5,usize::MAX] {assert_eq!(call_pc(address),None);}
    assert_eq!(call_pc(4),Some(0));
    assert_eq!(call_pc(0x1234),Some(0x1230));
}
