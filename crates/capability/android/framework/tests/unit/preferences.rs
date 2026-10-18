//! INPUT: private preference state and an isolated temporary filesystem
//! OUTPUT: failed flush retains dirty data; retry persists it before exit
//! POS: process-exit preference durability regression without JNI mocks
use super::*;

#[test]
fn checked_flush_rejects_failed_writes_and_recovers_without_losing_changes() {
    let root=std::env::temp_dir().join(format!("rudroid-prefs-flush-{}",std::process::id()));
    assert!(!root.exists());
    std::fs::create_dir(&root).unwrap();
    let blocked=root.join("blocked");
    std::fs::write(&blocked,b"a file cannot be a preference directory").unwrap();
    let path=blocked.join("prefs.json");
    let data=json!({"version":1,"strings":{"M2H_lastLanguage":"Chinese"}});
    let serial={
        let mut state=state().lock().unwrap();state.next+=1;let serial=state.next;
        state.stores.insert(serial,Store{path:path.clone(),data:data.clone(),global:0,dirty:true,revision:1});
        serial
    };
    assert!(!flush_checked(),"failure must not authorize process exit");
    assert!(state().lock().unwrap().stores[&serial].dirty);
    std::fs::rename(&blocked,root.join("blocker-retired")).unwrap();
    assert!(flush_checked());
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),data);
    assert!(!state().lock().unwrap().stores[&serial].dirty);
    assert!(flush_checked(),"an already clean store needs no write");
    state().lock().unwrap().stores.remove(&serial);
    std::fs::remove_dir_all(root).unwrap();
}
