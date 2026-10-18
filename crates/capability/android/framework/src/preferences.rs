//! INPUT: SharedPreferences JNI calls, shared VM identities and legacy JSON files
//! OUTPUT: one cached store per name, isolated editor changes and atomic persistence
//! POS: Android persistence service; JNI objects contain integer IDs, never Rust pointers
#![forbid(unsafe_code)]
use std::collections::HashMap;
use std::ffi::{c_void,CStr,CString};
use std::sync::{Mutex,OnceLock};
use rudroid_common::jni_abi::declare_field;
use rudroid_common::jni_env::{JObject,JniEnv,Native};
use serde_json::{Value,json};

struct Store {path:std::path::PathBuf, data:Value, global:usize,dirty:bool,revision:u64}
struct Editor {store:u64,weak:usize,changes:HashMap<String,Option<(String,Value)>>,clear:bool}
#[derive(Default)]
struct State {names:HashMap<String,u64>,stores:HashMap<u64,Store>,editors:HashMap<u64,Editor>,next:u64}
static STATE:OnceLock<Mutex<State>>=OnceLock::new();
static SAVE:Mutex<()>=Mutex::new(());
static PREF_ID:OnceLock<usize>=OnceLock::new();
static EDIT_ID:OnceLock<usize>=OnceLock::new();
fn state()->&'static Mutex<State>{STATE.get_or_init(||Mutex::new(State::default()))}
pub(super) fn text(env:JniEnv,string:*mut c_void)->Option<String>{
    let units=env.string_utf16(string)?;
    match String::from_utf16(&units){Ok(text)=>Some(text),Err(_)=>{error(env,"Java string contains an unpaired UTF-16 surrogate");None}}
}
pub(super) fn string(env:JniEnv,text:&str)->*mut c_void {
    let units:Vec<u16>=text.encode_utf16().collect();env.new_string(&units)
}
fn local(env:JniEnv,reference:usize)->*mut c_void{env.new_local_ref(reference as *mut c_void)}
pub(super) fn delete(env:JniEnv,reference:*mut c_void){env.delete_local_ref(reference)}
fn id(env:JniEnv,object:*mut c_void,field:&OnceLock<usize>)->u64 {
    env.get_long_field(object,*field.get().unwrap() as *mut c_void) as u64
}
pub(super) fn error(env:JniEnv,message:&str){
    let class=env.find_class(c"java/io/IOException");let message=CString::new(message.replace('\0'," ")).unwrap();env.throw_new(class,&message);delete(env,class);
}
fn object(env:JniEnv,class:&CStr,id:u64,field:&OnceLock<usize>)->*mut c_void {
    let object=super::java::allocate(env,class);
    env.set_long_field(object,*field.get().unwrap() as *mut c_void,id as i64);object
}
fn empty()->Value{json!({"version":1,"ints":{},"longs":{},"floats":{},"bools":{},"strings":{}})}
fn load(path:&std::path::Path)->Result<Value,String>{
    match std::fs::read(path) {
        Ok(bytes)=>{let data:Value=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
            if data.get("version").and_then(Value::as_u64)!=Some(1) || !data.is_object(){return Err("unsupported preferences file format".into());}Ok(data)},
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(empty()),
        Err(e)=>Err(e.to_string())
    }
}
extern "C" fn preferences(env:JniEnv,_context:JObject,name:JObject,_mode:i32)->JObject {
    let Some(name)=text(env,name) else{return std::ptr::null_mut();};
    if name.is_empty() || name=="." || name==".." || name.contains(['/', '\\', '\0']){error(env,"invalid preferences name");return std::ptr::null_mut();}
    if let Some(global)={let state=state().lock().unwrap();state.names.get(&name).map(|id|state.stores[id].global)}{return local(env,global);}
    let host=super::HOST.get().unwrap();let files=host.config_string(c"paths.android_files",4096);
    let root=std::path::PathBuf::from(files.as_deref().unwrap_or(".")).join("shared_prefs");
    let path=root.join(format!("{name}.json"));let data=match load(&path){Ok(data)=>data,Err(e)=>{error(env,&e);return std::ptr::null_mut();}};
    let serial={let mut state=state().lock().unwrap();state.next+=1;state.next};
    let result=object(env,c"android/content/SharedPreferences",serial,&PREF_ID);if result.is_null(){return result;}
    let global=env.new_global_ref(result) as usize;if global==0{delete(env,result);return std::ptr::null_mut();}
    let existing={let mut state=state().lock().unwrap();
        if let Some(id)=state.names.get(&name){Some(state.stores[id].global)}else{
            state.names.insert(name,serial);state.stores.insert(serial,Store{path,data,global,dirty:false,revision:0});None}};
    if let Some(existing)=existing {env.delete_global_ref(global as *mut c_void);delete(env,result);local(env,existing)}else{result}
}
extern "C" fn contains(env:JniEnv,prefs:JObject,key:JObject)->u8 {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return 0;};let state=state().lock().unwrap();
    u8::from(state.stores.get(&store).is_some_and(|s|["strings","ints","longs","floats","bools"].iter().any(|section|s.data.get(section).and_then(|v|v.get(&key)).is_some())))
}
extern "C" fn get_all(env: JniEnv, prefs: JObject) -> JObject {
    let store = id(env, prefs, &PREF_ID);
    let data = state().lock().unwrap().stores.get(&store).map(|s|s.data.clone());
    data.map(|data|super::collections::snapshot(env,data)).unwrap_or(std::ptr::null_mut())
}
extern "C" fn get_string(env:JniEnv,prefs:JObject,key:JObject,default:JObject)->JObject {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return local(env,default as usize);};
    let value=state().lock().unwrap().stores.get(&store).and_then(|s|s.data.get("strings")).and_then(|v|v.get(&key)).and_then(Value::as_str).map(str::to_owned);
    value.map(|v|string(env,&v)).unwrap_or_else(||local(env,default as usize))
}
extern "C" fn get_int(env:JniEnv,prefs:JObject,key:JObject,default:i32)->i32 {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return default;};
    state().lock().unwrap().stores.get(&store).and_then(|s|s.data.get("ints")).and_then(|v|v.get(&key)).and_then(Value::as_i64).and_then(|v|i32::try_from(v).ok()).unwrap_or(default)
}
extern "C" fn get_float(env:JniEnv,prefs:JObject,key:JObject,default:f32)->f32 {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return default;};
    state().lock().unwrap().stores.get(&store).and_then(|s|s.data.get("floats"))
        .and_then(|v|v.get(&key)).and_then(Value::as_f64).map(|v|v as f32).unwrap_or(default)
}
extern "C" fn get_long(env:JniEnv,prefs:JObject,key:JObject,default:i64)->i64 {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return default;};
    state().lock().unwrap().stores.get(&store).and_then(|s|s.data.get("longs"))
        .and_then(|v|v.get(&key)).and_then(Value::as_i64).unwrap_or(default)
}
extern "C" fn get_boolean(env:JniEnv,prefs:JObject,key:JObject,default:u8)->u8 {
    let store=id(env,prefs,&PREF_ID);let Some(key)=text(env,key) else{return u8::from(default!=0);};
    u8::from(state().lock().unwrap().stores.get(&store).and_then(|s|s.data.get("bools"))
        .and_then(|v|v.get(&key)).and_then(Value::as_bool).unwrap_or(default!=0))
}
fn collect_editors(env:JniEnv){
    let weak:Vec<_>=state().lock().unwrap().editors.iter().map(|(&id,e)|(id,e.weak)).collect();
    for (id,reference) in weak {let live=local(env,reference);if live.is_null(){if env.exception_check(){return;}let removed=state().lock().unwrap().editors.remove(&id);if removed.is_some(){env.delete_weak_global_ref(reference as *mut c_void);}}else{delete(env,live);}}
}
extern "C" fn edit(env:JniEnv,prefs:JObject)->JObject {
    let store=id(env,prefs,&PREF_ID);collect_editors(env);if env.exception_check(){return std::ptr::null_mut();}
    let serial={let mut state=state().lock().unwrap();if !state.stores.contains_key(&store){return std::ptr::null_mut();}state.next+=1;state.next};
    let object=object(env,c"android/content/SharedPreferences$Editor",serial,&EDIT_ID);if object.is_null(){return object;}
    let weak=env.new_weak_global_ref(object) as usize;if weak==0{delete(env,object);return std::ptr::null_mut();}
    state().lock().unwrap().editors.insert(serial,Editor{store,weak,changes:HashMap::new(),clear:false});object
}
extern "C" fn put_string(env:JniEnv,editor:JObject,key:JObject,value:JObject)->JObject {
    let serial=id(env,editor,&EDIT_ID);let Some(key)=text(env,key) else{return std::ptr::null_mut();};let null=value.is_null();let value=text(env,value);if !null && value.is_none(){return std::ptr::null_mut();}
    if let Some(editor)=state().lock().unwrap().editors.get_mut(&serial){editor.changes.insert(key,value.map(|value|("strings".into(),Value::String(value))));}local(env,editor as usize)
}
extern "C" fn put_int(env: JniEnv, editor: JObject, key: JObject, value: i32) -> JObject {
    put_scalar(env,editor,key,"ints",Value::from(value))
}
fn put_scalar(env:JniEnv,editor:JObject,key:JObject,section:&str,value:Value)->JObject {
    let serial=id(env,editor,&EDIT_ID);if key.is_null(){return local(env,editor as usize);}
    let Some(key)=text(env,key) else{return std::ptr::null_mut();};
    if let Some(editor)=state().lock().unwrap().editors.get_mut(&serial){editor.changes.insert(key,Some((section.into(),value)));}
    local(env,editor as usize)
}
extern "C" fn put_long(env:JniEnv,editor:JObject,key:JObject,value:i64)->JObject {put_scalar(env,editor,key,"longs",Value::from(value))}
extern "C" fn put_boolean(env:JniEnv,editor:JObject,key:JObject,value:u8)->JObject {put_scalar(env,editor,key,"bools",Value::Bool(value!=0))}
extern "C" fn put_float(env:JniEnv,editor:JObject,key:JObject,value:f32)->JObject {
    let Some(value)=serde_json::Number::from_f64(f64::from(value)) else{
        error(env,"non-finite preference float cannot be stored in the legacy JSON format");return std::ptr::null_mut();
    };
    put_scalar(env,editor,key,"floats",Value::Number(value))
}
extern "C" fn remove(env:JniEnv,editor:JObject,key:JObject)->JObject{put_string(env,editor,key,std::ptr::null_mut())}
extern "C" fn clear(env:JniEnv,editor:JObject)->JObject {
    let serial=id(env,editor,&EDIT_ID);if let Some(editor)=state().lock().unwrap().editors.get_mut(&serial){editor.clear=true;}local(env,editor as usize)
}
fn apply_changes(serial:u64)->Option<u64>{
    let mut state=state().lock().unwrap();let editor=state.editors.get_mut(&serial)?;
    let store=editor.store;let clear=std::mem::take(&mut editor.clear);let changes=std::mem::take(&mut editor.changes);
    let target=state.stores.get_mut(&store)?;if clear{target.data=empty();}
    for (key,value) in changes {
        for section in ["strings","ints","longs","floats","bools"]{if let Some(section)=target.data.get_mut(section).and_then(Value::as_object_mut){section.remove(&key);}}
        if let Some((section,value))=value{if !target.data.get(&section).is_some_and(Value::is_object){target.data[&section]=json!({});}target.data[&section][key]=value;}
    }
    target.dirty=true;target.revision+=1;Some(store)
}
fn save(store:u64)->std::io::Result<()> {
    use std::io::Write;
    let _serial=SAVE.lock().unwrap();
    let (path,data,revision)={let state=state().lock().unwrap();let Some(store)=state.stores.get(&store) else{return Ok(());};if !store.dirty{return Ok(());} (store.path.clone(),serde_json::to_vec(&store.data)?,store.revision)};
    std::fs::create_dir_all(path.parent().unwrap())?;let tmp=path.with_extension("json.tmp");let mut file=std::fs::File::create(&tmp)?;file.write_all(&data)?;file.sync_all()?;drop(file);std::fs::rename(tmp,path)?;
    if let Some(store)=state().lock().unwrap().stores.get_mut(&store){if store.revision==revision{store.dirty=false;}}Ok(())
}
extern "C" fn commit(env:JniEnv,editor:JObject)->u8 {
    apply_changes(id(env,editor,&EDIT_ID)).is_some_and(|id|save(id).is_ok()) as u8
}
extern "C" fn apply(env:JniEnv,editor:JObject){
    apply_changes(id(env,editor,&EDIT_ID));
}
pub(super) fn flush(){
    let _ = flush_checked();
}
pub(super) fn flush_checked()->bool {
    let dirty:Vec<_>=state().lock().unwrap().stores.iter().filter(|(_,s)|s.dirty).map(|(&id,_)|id).collect();
    let mut success=true;
    for store in dirty {if let Err(e)=save(store){success=false;if let Some(host)=super::HOST.get(){host.log_line(c"android_base",&format!("preferences save failed: {e}"));}}}
    success && !state().lock().unwrap().stores.values().any(|store|store.dirty)
}
pub(super) fn register(env:JniEnv)->i32 {
    if super::collections::register(env)!=0{return -1;}
    for (class,cache) in [(c"android/content/SharedPreferences",&PREF_ID),(c"android/content/SharedPreferences$Editor",&EDIT_ID)] {
        if declare_field(env,class,c"storeId",c"J",false)!=0{return -1;}
        let class=env.find_class(class);let value=env.get_field_id(class,c"storeId",c"J");delete(env,class);if value.is_null(){return -1;}cache.get_or_init(||value as usize);
    }
    for (class,methods) in [
        (c"android/content/Context",vec![Native::new2(c"getSharedPreferences",c"(Ljava/lang/String;I)Landroid/content/SharedPreferences;",preferences)]),
        (c"android/content/SharedPreferences",vec![
            Native::new0(c"getAll",c"()Ljava/util/Map;",get_all),
            Native::new2(c"getInt",c"(Ljava/lang/String;I)I",get_int),
            Native::new2(c"getFloat",c"(Ljava/lang/String;F)F",get_float),
            Native::new2(c"getLong",c"(Ljava/lang/String;J)J",get_long),
            Native::new2(c"getBoolean",c"(Ljava/lang/String;Z)Z",get_boolean),
            Native::new1(c"contains",c"(Ljava/lang/String;)Z",contains),
            Native::new2(c"getString",c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",get_string),
            Native::new0(c"edit",c"()Landroid/content/SharedPreferences$Editor;",edit),
        ]),
        (c"android/content/SharedPreferences$Editor",vec![
            Native::new2(c"putInt",c"(Ljava/lang/String;I)Landroid/content/SharedPreferences$Editor;",put_int),
            Native::new2(c"putLong",c"(Ljava/lang/String;J)Landroid/content/SharedPreferences$Editor;",put_long),
            Native::new2(c"putFloat",c"(Ljava/lang/String;F)Landroid/content/SharedPreferences$Editor;",put_float),
            Native::new2(c"putBoolean",c"(Ljava/lang/String;Z)Landroid/content/SharedPreferences$Editor;",put_boolean),
            Native::new2(c"putString",c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",put_string),
            Native::new1(c"remove",c"(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",remove),
            Native::new0(c"clear",c"()Landroid/content/SharedPreferences$Editor;",clear),
            Native::new0(c"apply",c"()V",apply),
            Native::new0(c"commit",c"()Z",commit),
        ])
    ]{
        if env.register_class(class,&methods)!=0{return -1;}}0
}

#[cfg(test)]
#[path = "../tests/unit/preferences.rs"]
mod tests;
