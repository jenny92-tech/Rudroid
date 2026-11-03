//! INPUT:  shared VM player fields, key events and registered Unity keyboard natives
//! OUTPUT:  bounded text entry, key consumption, confirm/cancel and session cleanup
//! POS:    safe legacy Unity Java keyboard bridge; idle frames do no keyboard work
#![forbid(unsafe_code)]
use rudroid_common::{host_api::HostApiV0,jni_abi::declare_field,jni_env::{Arg,JObject,JniEnv,Native}};
use std::{ffi::CStr,sync::{OnceLock,atomic::{AtomicBool,AtomicU64,Ordering}}};

const PLAYER:&CStr=c"com/unity3d/player/UnityPlayer";
const MAX_UNITS:usize=32_768;
static FIELDS:OnceLock<[usize;3]>=OnceLock::new();
static HOST:OnceLock<HostApiV0>=OnceLock::new();
// Only an activity hint; text/limits belong to the VM's actual player object.
static ACTIVE:AtomicBool=AtomicBool::new(false);
static GENERATION:AtomicU64=AtomicU64::new(0);

fn optional(env:JniEnv,player:JObject,name:&CStr,sig:&CStr,args:&[Arg])->bool {
    if env.exception_check(){return false;}
    let class=env.get_object_class(player);let method=env.method(class,name,sig);env.delete_local_ref(class);
    let Some(method)=method else {env.exception_clear();return false;};
    env.call_void(player,method,args);!env.exception_check()
}
fn flag(env:JniEnv,player:JObject)->bool {
    FIELDS.get().is_some_and(|f|env.get_boolean_field(player,f[2] as JObject)!=0)
}
fn limit(env:JniEnv,player:JObject)->usize {
    let n=env.get_int_field(player,FIELDS.get().unwrap()[1] as JObject);
    if n>0 {(n as usize).min(MAX_UNITS)}else{MAX_UNITS}
}
fn bounded(mut text:Vec<u16>,limit:usize)->Vec<u16> {
    if text.len()>limit {
        let mut end=limit;
        if end>0&&(0xd800..=0xdbff).contains(&text[end-1])&&(0xdc00..=0xdfff).contains(&text[end]){end-=1;}
        text.truncate(end);
    }text
}
fn selected(env:JniEnv,requested:JObject)->Option<Vec<u16>> {
    let host=HOST.get()?;
    let default=std::env::var("BD_SOFT_INPUT_DEFAULT").ok().filter(|s|!s.is_empty())
        .or_else(||host.config_string(c"soft_input.default",MAX_UNITS*4)).unwrap_or_default();
    let force=std::env::var_os("BD_SOFT_INPUT_FORCE_DEFAULT").is_some_and(|s|s!=""&&s!="0")
        ||host.config_bool(c"soft_input.force_default",false);
    let requested=if requested.is_null(){Vec::new()}else{env.with_string_utf16(requested,|s|s.iter().copied().take(MAX_UNITS+1).collect())?};
    Some(if (!force&&!requested.is_empty())||default.is_empty(){requested}else{default.encode_utf16().take(MAX_UNITS+1).collect()})
}
fn publish(env:JniEnv,player:JObject,units:&[u16])->JObject {
    let text=env.new_string(units);env.set_object_field(player,FIELDS.get().unwrap()[0] as JObject,text);text
}
fn apply(env:JniEnv,player:JObject,text:JObject) {
    let length=env.get_string_length(text);
    if optional(env,player,c"nativeSetInputString",c"(Ljava/lang/String;)V",&[Arg::object(text)]) {
        optional(env,player,c"nativeSetInputSelection",c"(II)V",&[Arg::int(length),Arg::int(length)]);
    }
}
pub(super) fn reset(env:JniEnv,player:JObject) {
    ACTIVE.store(false,Ordering::Release);GENERATION.fetch_add(1,Ordering::Relaxed);
    if let Some(f)=FIELDS.get(){env.set_boolean_field(player,f[2] as JObject,0);env.set_object_field(player,f[0] as JObject,std::ptr::null_mut());}
}
fn finish(env:JniEnv,player:JObject,cancel:bool) {
    let f=FIELDS.get().unwrap();let text=env.get_object_field(player,f[0] as JObject);
    reset(env,player);let generation=GENERATION.load(Ordering::Relaxed);
    if !cancel&&!text.is_null(){apply(env,player,text);}
    if GENERATION.load(Ordering::Relaxed)!=generation{env.delete_local_ref(text);return;}
    optional(env,player,c"nativeSetKeyboardIsVisible",c"(Z)V",&[Arg::bool(false)]);
    if GENERATION.load(Ordering::Relaxed)!=generation{env.delete_local_ref(text);return;}
    if cancel {
        if !optional(env,player,c"nativeSoftInputCanceled",c"()V",&[]){optional(env,player,c"nativeSoftInputClosed",c"()V",&[]);}
    }else{
        optional(env,player,c"nativeSoftInputClosed",c"()V",&[]);
        // Some players clear the edit buffer on close. Preserve the reference's
        // final text publication, unless a callback opened a newer keyboard.
        if GENERATION.load(Ordering::Relaxed)==generation&&!text.is_null(){apply(env,player,text);}
    }
    env.delete_local_ref(text);
}
extern "C" fn show(env:JniEnv,player:JObject,text:JObject,_kind:i32,_correct:u8,_multi:u8,_secure:u8,_alert:u8,
    _placeholder:JObject,character_limit:i32,_reuse:u8,_hide:u8) {
    let f=FIELDS.get().unwrap();env.set_int_field(player,f[1] as JObject,character_limit);
    let Some(value)=selected(env,text)else{return;};let value=bounded(value,limit(env,player));
    let text=publish(env,player,&value);env.set_boolean_field(player,f[2] as JObject,1);
    ACTIVE.store(true,Ordering::Release);let generation=GENERATION.fetch_add(1,Ordering::Relaxed).wrapping_add(1);
    apply(env,player,text);env.delete_local_ref(text);
    if GENERATION.load(Ordering::Relaxed)==generation&&flag(env,player){optional(env,player,c"nativeSetKeyboardIsVisible",c"(Z)V",&[Arg::bool(true)]);}
}
extern "C" fn hide(env:JniEnv,player:JObject) {
    reset(env,player);optional(env,player,c"nativeSetKeyboardIsVisible",c"(Z)V",&[Arg::bool(false)]);
}
fn update(env:JniEnv,player:JObject,text:JObject)->Option<u64> {
    let value=bounded(selected(env,text)?,limit(env,player));
    let text=publish(env,player,&value);env.set_boolean_field(player,FIELDS.get().unwrap()[2] as JObject,1);
    ACTIVE.store(true,Ordering::Release);let generation=GENERATION.fetch_add(1,Ordering::Relaxed).wrapping_add(1);
    apply(env,player,text);env.delete_local_ref(text);Some(generation)
}
extern "C" fn set_text(env:JniEnv,player:JObject,text:JObject) {update(env,player,text);}
extern "C" fn set_action(env:JniEnv,player:JObject,text:JObject,_action:i32) {
    if update(env,player,text).is_some_and(|g|GENERATION.load(Ordering::Relaxed)==g){finish(env,player,false);}
}
extern "C" fn layout(_env:JniEnv,_player:JObject)->i32{0}

fn key_code(name:&str)->Option<i32> {
    let name=name.trim().to_ascii_uppercase();let name=name.strip_prefix("KEYCODE_").unwrap_or(&name);
    Some(match name {
        "ENTER"=>66,"NUMPAD_ENTER"=>160,"DPAD_CENTER"=>23,"BUTTON_A"|"A"=>96,"BUTTON_B"|"B"=>97,
        "BUTTON_X"|"X"=>99,"BUTTON_Y"|"Y"=>100,"BUTTON_START"|"START"=>108,"BUTTON_SELECT"|"SELECT"=>109,
        "BUTTON_R1"|"R1"|"RB"=>103,"BUTTON_L1"|"L1"|"LB"=>102,"BUTTON_THUMBR"|"THUMBR"|"R3"=>107,
        "BUTTON_THUMBL"|"THUMBL"|"L3"=>106,"BACK"=>4,"ESCAPE"|"ESC"=>111,_=>return None,
    })
}
static KEYS:OnceLock<(Vec<i32>,Vec<i32>)>=OnceLock::new();
fn keys()->&'static (Vec<i32>,Vec<i32>){KEYS.get_or_init(||{
    let parse=|name:&str,key:&CStr,fallback:&str|std::env::var(name).ok().filter(|s|!s.is_empty())
        .or_else(||HOST.get().unwrap().config_string(key,1024)).unwrap_or_else(||fallback.into()).split(',').filter_map(key_code).collect();
    (parse("BD_SOFT_INPUT_CONFIRM_KEYS",c"soft_input.confirm_keys","ENTER,NUMPAD_ENTER,DPAD_CENTER,BUTTON_A,BUTTON_START,BUTTON_R1,BUTTON_THUMBR"),
     parse("BD_SOFT_INPUT_CANCEL_KEYS",c"soft_input.cancel_keys","BACK,ESCAPE,BUTTON_B,BUTTON_SELECT"))
})}
pub(super) fn consume(env:JniEnv,player:JObject,event:JObject)->Result<bool,()> {
    if !ACTIVE.load(Ordering::Acquire)||!flag(env,player){return Ok(false);}
    let class=env.find_class(c"android/view/KeyEvent");let is_key=env.is_instance_of(event,class);
    if !is_key {env.delete_local_ref(class);return if env.exception_check(){Err(())}else{Ok(false)};}
    let action=env.method(class,c"getAction",c"()I");let code=env.method(class,c"getKeyCode",c"()I");env.delete_local_ref(class);
    let (Some(action),Some(code))=(action,code)else{return Err(());};
    let action=env.call_int(event,action,&[]);let code=env.call_int(event,code,&[]);
    if env.exception_check(){return Err(());}
    if action==1 {
        let keys=keys();
        if keys.0.contains(&code){finish(env,player,false);}
        else if keys.1.contains(&code){finish(env,player,true);}
        else {
            let typed=match code {100|52=>Some(b'x'),99|53=>Some(b'y'),29..=54=>Some(b'a'+(code-29) as u8),7..=16=>Some(b'0'+(code-7) as u8),62=>Some(b' '),_=>None};
            if typed.is_none()&&code!=67{return Ok(true);}
            let old=env.get_object_field(player,FIELDS.get().unwrap()[0] as JObject);
            let mut value=if old.is_null(){Vec::new()}else{env.string_utf16(old).unwrap_or_default()};env.delete_local_ref(old);
            if code==67 {
                if let Some(last)=value.pop(){if (0xdc00..=0xdfff).contains(&last)&&value.last().is_some_and(|v|(0xd800..=0xdbff).contains(v)){value.pop();}}
            }else if let Some(c)=typed{if value.len()<limit(env,player){value.push(u16::from(c));}}
            else{return Ok(true);}
            let text=publish(env,player,&value);apply(env,player,text);env.delete_local_ref(text);
        }
    }
    if env.exception_check(){Err(())}else{Ok(true)}
}
extern "C" fn inject(env:JniEnv,player:JObject,event:JObject)->u8 {
    match consume(env,player,event){Ok(true)=>1,Err(())=>0,Ok(false)=>{
        let class=env.get_object_class(player);let method=env.method(class,c"nativeInjectEvent",c"(Landroid/view/InputEvent;)Z");env.delete_local_ref(class);
        method.map_or(0,|m|u8::from(env.call_boolean(player,m,&[Arg::object(event)])))
    }}
}
pub(super) fn register(env:JniEnv,host:&HostApiV0)->i32 {
    let _=HOST.set(*host);
    let mut fields=[0;3];
    let class=env.find_class(PLAYER);
    for (i,(name,sig)) in [(c"_rudroidSoftInputText",c"Ljava/lang/String;"),(c"_rudroidSoftInputLimit",c"I"),(c"_rudroidSoftInputActive",c"Z")].into_iter().enumerate(){
        if declare_field(env,PLAYER,name,sig,false)!=0{env.delete_local_ref(class);return -1;}
        fields[i]=env.get_field_id(class,name,sig) as usize;
    }
    env.delete_local_ref(class);if env.exception_check(){return -1;}let _=FIELDS.set(fields);
    env.register_class(PLAYER,&[
        Native::new10(c"showSoftInput",c"(Ljava/lang/String;IZZZZLjava/lang/String;IZZ)V",show),
        Native::new0(c"hideSoftInput",c"()V",hide),Native::new1(c"setSoftInputStr",c"(Ljava/lang/String;)V",set_text),
        Native::new2(c"setSoftInputStr",c"(Ljava/lang/String;I)V",set_action),Native::new0(c"getKeyboardLayout",c"()I",layout),
        Native::new1(c"injectEvent",c"(Landroid/view/InputEvent;)Z",inject),
    ])
}
