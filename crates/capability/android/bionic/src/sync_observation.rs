//! INPUT: owned synchronization identities, received/returned metadata and caller scalar
//! OUTPUT: opt-in bounded long-wait observations with anonymous object identities
//! POS: no guest memory/stack/code reads, no wakeups or native-result substitution
#![forbid(unsafe_code)]
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, Instant};

const OBJECTS: usize = 4096;
const THREADS: usize = 128;
const RECORDS: usize = 128;
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| rudroid_common::logging::diagnostics_enabled()
        && std::env::var_os("RUDROID_DIAGNOSTIC_SYNC").is_some_and(|v|v=="1"))
}
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(||Mutex::new(State::new()))
}
fn thread_id() -> Option<u32> {
    thread_local! {static TID: std::cell::OnceCell<Option<u32>> = const { std::cell::OnceCell::new() };}
    TID.with(|slot|*slot.get_or_init(||std::fs::read_link("/proc/thread-self").ok()?
        .file_name()?.to_str()?.parse().ok()))
}

#[derive(Default)]
struct Object { id:u64, notifications:u64, notifier:u32 }
struct Wait {
    seq:u64, op:&'static str, object:usize, object_id:u64, mutex_id:u64,
    caller_ra:usize,
    since:Instant, initial_notifications:u64, reported:Option<u64>, active:Arc<AtomicU64>,
}
struct State {
    objects:HashMap<usize,Object>, waits:HashMap<u32,Wait>, threads:HashMap<u32,Arc<AtomicU64>>, next:u64, sequence:u64,
    signals:Vec<Arc<AtomicU64>>, records:usize, status_records:u8, last_drop:u64, reported_full:bool,
}
impl State {
    fn new()->Self {Self {objects:HashMap::with_capacity(OBJECTS),waits:HashMap::with_capacity(THREADS),threads:HashMap::with_capacity(THREADS),
        signals:(0..THREADS).map(|_|Arc::new(AtomicU64::new(0))).collect(),next:1,sequence:1,records:0,status_records:0,last_drop:0,reported_full:false}}
    fn register(&mut self, native:usize) {
        if self.objects.len()>=OBJECTS && !self.objects.contains_key(&native) {DROPPED.fetch_add(1,Ordering::Relaxed);return;}
        let id=self.next;self.next+=1;
        self.objects.insert(native,Object{id,..Object::default()});
    }
    fn begin(&mut self, op:&'static str, object:usize, mutex:usize, tid:u32, caller_ra:usize)->Option<Ticket> {
        let Some(o)=self.objects.get(&object) else {DROPPED.fetch_add(1,Ordering::Relaxed);return None;};
        let (id,initial)=(o.id,o.notifications);
        let mutex_id=self.objects.get(&mutex).map_or(0,|m|m.id);
        let seq=self.sequence;self.sequence+=1;
        let active=if blocks(op) {
            if self.waits.len()>=THREADS && !self.waits.contains_key(&tid) {DROPPED.fetch_add(1,Ordering::Relaxed);return None;}
            if self.threads.len()>=THREADS && !self.threads.contains_key(&tid) {DROPPED.fetch_add(1,Ordering::Relaxed);return None;}
            if !self.threads.contains_key(&tid) {self.threads.insert(tid,Arc::clone(&self.signals[self.threads.len()]));}
            let active=Arc::clone(&self.threads[&tid]);
            if active.compare_exchange(0,seq,Ordering::AcqRel,Ordering::Relaxed).is_err() {DROPPED.fetch_add(1,Ordering::Relaxed);return None;}
            self.waits.insert(tid,Wait {seq,op,object,object_id:id,mutex_id,caller_ra,since:Instant::now(),initial_notifications:initial,reported:None,active:Arc::clone(&active)});
            Some(active)
        } else {None};
        Some(Ticket {seq,op,object,object_id:id,tid,active})
    }
    fn end(&mut self,t:Ticket,result:i32) {
        t.complete();
        if self.waits.get(&t.tid).is_some_and(|w|w.seq==t.seq) {self.waits.remove(&t.tid);}
        if result==0 {
            if let Some(o)=self.objects.get_mut(&t.object).filter(|o|o.id==t.object_id) {
                match t.op {
                    "pthread_cond_signal"|"pthread_cond_broadcast"=>{o.notifications=o.notifications.saturating_add(1);o.notifier=t.tid;}
                    _=>{}
                }
            }
        }
    }
    fn collect_rows(&mut self,age:Duration,rows:&mut Vec<Row>) {
        self.waits.retain(|_,w|w.active.load(Ordering::Acquire)==w.seq);
        for (&tid,w) in &mut self.waits {
            if w.active.load(Ordering::Acquire)!=w.seq {continue;}
            let Some(o)=self.objects.get(&w.object).filter(|o|o.id==w.object_id) else {continue;};
            let version=o.notifications;
            if w.since.elapsed()<age || w.reported==Some(version) || self.records>=RECORDS {continue;}
            w.reported=Some(version);self.records+=1;
            rows.push(Row {tid,op:w.op,object:o.id,mutex:w.mutex_id,
                initial:w.initial_notifications,notifications:o.notifications,notifier:o.notifier,ms:w.since.elapsed().as_millis(),caller_ra:w.caller_ra});
        }
    }
    #[cfg(test)]
    fn rows(&mut self,age:Duration)->Vec<Row> {let mut out=Vec::with_capacity(THREADS);self.collect_rows(age,&mut out);out}
}
fn blocks(op:&str)->bool { matches!(op,"pthread_cond_wait"|"pthread_cond_timedwait") }
pub(super) struct Ticket {seq:u64,op:&'static str,object:usize,object_id:u64,tid:u32,active:Option<Arc<AtomicU64>>}
impl Ticket {
    fn complete(&self) {if let Some(active)=&self.active {let _=active.compare_exchange(self.seq,0,Ordering::AcqRel,Ordering::Relaxed);}}
}
struct Row {tid:u32,op:&'static str,object:u64,mutex:u64,initial:u64,notifications:u64,notifier:u32,ms:u128,caller_ra:usize}

// This is arithmetic on a received address scalar, not a code read or an
// assertion that the address belongs to a known/executable guest image.
fn call_pc(caller_ra:usize)->Option<usize> {
    caller_ra.checked_sub(4).filter(|_|caller_ra%4==0)
}

// Cold init/destroy serialize lifetime registration; no I/O, allocation or
// native call happens under that lock. Hot sampling uses try_lock, and a
// missed sample is not negative proof. No lock spans a native wait or logging.
pub(super) fn register(native:usize) {
    if !enabled() {return;}
    state().lock().unwrap_or_else(|e|e.into_inner()).register(native);
}
pub(super) fn retire(native:usize) {
    if !enabled() {return;}
    state().lock().unwrap_or_else(|e|e.into_inner()).objects.remove(&native);
}
pub(super) fn begin(op:&'static str,object:usize,mutex:usize,caller_ra:usize)->Option<Ticket> {
    let tid=thread_id()?;
    if let Ok(mut s)=state().try_lock() {s.begin(op,object,mutex,tid,caller_ra)} else {DROPPED.fetch_add(1,Ordering::Relaxed);None}
}
pub(super) fn end(ticket:Option<Ticket>,result:i32) {
    if let Some(t)=ticket {
        // Completion is published even if observer bookkeeping is contended:
        // a missed end record must never manufacture a permanently stuck wait.
        t.complete();
        if let Ok(mut s)=state().try_lock() {s.end(t,result);} else {DROPPED.fetch_add(1,Ordering::Relaxed);}
    }
}
pub(super) fn snapshot() {
    if !enabled() {return;}
    let mut rows=Vec::with_capacity(THREADS);
    let status=if let Ok(mut s)=state().try_lock() {
        s.collect_rows(Duration::from_secs(1),&mut rows);
        let dropped=DROPPED.load(Ordering::Relaxed);let full=s.records>=RECORDS;
        if s.status_records<16 && (s.status_records==0 || dropped!=s.last_drop || full!=s.reported_full) {
            s.status_records+=1;s.last_drop=dropped;s.reported_full=full;
            Some((dropped,full,s.status_records==16))
        } else {None}
    } else {return;};
    if let Some((dropped,full,status_full))=status {
        rudroid_common::diagnostic!("[BIONIC] sync observer status: scope=cond metadata_dropped={dropped} row_budget_exhausted={full} status_budget_exhausted={status_full} row_limit=128 status_limit=16");
    }
    for r in rows {
        let name=std::fs::read_to_string(format!("/proc/self/task/{}/comm",r.tid)).unwrap_or_default();
        // Public kernel thread metadata only; no process command line or stack.
        let name=name.trim();
        let pc=call_pc(r.caller_ra).map_or_else(||"unknown".to_owned(),|pc|format!("{pc:#x}"));
        rudroid_common::diagnostic!("[BIONIC] sync wait snapshot: tid={} name={:?} api={} object={} mutex={} notifications_at_entry={} notifications_now={} last_notifier={} elapsed_ms={} metadata_dropped={} caller_ra={:#x} call_pc={}",
            r.tid,name,r.op,r.object,r.mutex,r.initial,r.notifications,r.notifier,r.ms,DROPPED.load(Ordering::Relaxed),r.caller_ra,pc);
    }
}
extern "C" fn snapshot_jni(_env:rudroid_common::jni_env::JniEnv,_class:rudroid_common::jni_env::JObject) {snapshot();}
pub(super) fn register_jni(api:&rudroid_common::host_api::HostApiV0)->i32 {
    if !enabled() {return 0;}
    let _=state();
    let Some(env)=api.java_vm().and_then(|vm|vm.get_env(rudroid_common::jni_env::JNI_VERSION_1_6)) else {return -4;};
    env.register_class(c"org/rudroid/runtime/SyncDiagnostics",&[
        rudroid_common::jni_env::Native::new0(c"snapshot",c"()V",snapshot_jni)])
}

#[cfg(test)]
pub(super) fn test_has_condition_wait()->bool {
    state().lock().unwrap().waits.values().any(|w|w.op=="pthread_cond_wait" && w.active.load(Ordering::Acquire)==w.seq)
}
#[cfg(test)]
pub(super) fn test_dropped()->u64 {DROPPED.load(Ordering::Relaxed)}
#[cfg(test)]
pub(super) fn test_condition_wait_callers()->Vec<usize> {
    state().lock().unwrap().waits.values()
        .filter(|w|w.op=="pthread_cond_wait" && w.active.load(Ordering::Acquire)==w.seq)
        .map(|w|w.caller_ra).collect()
}

#[cfg(test)]
#[path = "../tests/unit/sync_observation.rs"]
mod tests;
