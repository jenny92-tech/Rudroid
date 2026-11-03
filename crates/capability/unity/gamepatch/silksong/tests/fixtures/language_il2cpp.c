// INPUT:  base metadata fixture and language/exception/ABI controls
// OUTPUT:  real enum-switch detour, preference writes and checked root ownership
// POS:    synthetic localization oracle; no private game binaries or persistent user data
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
#define il2cpp_domain_get_assemblies legacy_assemblies
#define il2cpp_class_get_method_from_name legacy_method
#define il2cpp_method_get_flags legacy_flags
#define il2cpp_method_get_param legacy_param
#define il2cpp_method_get_return_type legacy_return
#define il2cpp_runtime_invoke legacy_invoke
#define il2cpp_object_get_class legacy_object_class
#define il2cpp_string_new legacy_string_new
#define il2cpp_string_length legacy_length
#define il2cpp_string_chars legacy_chars
#define il2cpp_thread_current legacy_thread
#include "rudroid54_silksong_il2cpp.c"
#undef il2cpp_domain_get_assemblies
#undef il2cpp_class_get_method_from_name
#undef il2cpp_method_get_flags
#undef il2cpp_method_get_param
#undef il2cpp_method_get_return_type
#undef il2cpp_runtime_invoke
#undef il2cpp_object_get_class
#undef il2cpp_string_new
#undef il2cpp_string_length
#undef il2cpp_string_chars
#undef il2cpp_thread_current
#include <assert.h>
#include <string.h>

static int bad, failure, current=44, calls, writes, saves, made, freed;
static void (*callback)(void);
void rudroid60_bad(int value) { bad=value; }
void rudroid60_failure(int value) { failure=value; }
void rudroid60_callback(void (*value)(void)) { callback=value; }
int rudroid60_count(int index) { int values[]={calls,writes,saves,made,freed}; return values[index]; }
static char saved[16];
const char *rudroid60_saved(void) { return saved; }
bool rudroid60_switch_body(int value,void *metadata) {
 (void)metadata; ++calls; if(value!=44&&value!=202&&value!=31)return false; current=value; return true;
}
bool rudroid60_switch(int value) { return rudroid60_switch_body(value,0); }
static void placeholder(void) {}
static const struct Rudroid54Method methods[]={
 {placeholder,"SwitchLanguage",1}, {rudroid60_switch_body,"SwitchLanguage",1}, {placeholder,"CurrentLanguage",0},
 {placeholder,"ToString",0}, {placeholder,"SetString",2}, {placeholder,"Save",0}, {placeholder,"get_unityVersion",0}};
static const struct Rudroid54Class classes[]={
 {"TeamCherry.Localization","Language",methods,3}, {"TeamCherry.Localization","LanguageCode",0,0},
 {"System","Enum",methods+3,1}, {"UnityEngine","PlayerPrefs",methods+4,2}, {"System","String",0,0}};
static const struct Rudroid54Image image={"TeamCherry.Localization.dll",classes,5};
static const struct Rudroid54Assembly assembly={&image};
static const struct Rudroid54Assembly *const domain[]={&rudroid54_assemblies[0],&rudroid54_assemblies[1],&rudroid54_assemblies[2],&assembly};
void **il2cpp_domain_get_assemblies(void *d,size_t *count) {
 if(d!=(void*)rudroid54_domain)return 0;*count=bad==1?3:4;return (void**)domain;
}
void *il2cpp_class_get_method_from_name(void *k,const char *name,int argc) {
 if(rudroid54_streq(name,"get_unityVersion"))return (void*)&methods[6];return legacy_method(k,name,argc);
}
void *il2cpp_class_get_parent(void *k) { return k==&classes[1]?(void*)&classes[2]:0; }
void *il2cpp_class_from_type(void *type) { return type==(void*)0x11?(void*)&classes[1]:0; }
void *il2cpp_class_enum_basetype(void *k) { return k==&classes[1]?(void*)(uintptr_t)(bad==2?10:8):0; }
static int own(void *m) { for(int i=0;i<7;i++)if(m==&methods[i])return 1;return 0; }
uint32_t il2cpp_method_get_flags(void *m,uint32_t *impl) {
 if(!own(m))return legacy_flags(m,impl);return m==&methods[3]||(bad==3&&m==&methods[1])?0:0x10;
}
void *il2cpp_method_get_return_type(void *m) {
 if(!own(m))return legacy_return(m);
 if(m==&methods[0]||m==&methods[1])return (void*)(uintptr_t)(bad==4?1:2);
 if(m==&methods[2])return (void*)(uintptr_t)(bad==5?8:0x11);
 if(m==&methods[3]||m==&methods[6])return (void*)0x0e;
 return (void*)1;
}
void *il2cpp_method_get_param(void *m,uint32_t index) {
 if(!own(m))return legacy_param(m,index);
 if(m==&methods[0])return (void*)0x0e;
 if(m==&methods[1])return (void*)(uintptr_t)(bad==6?0x111:0x11);
 if(m==&methods[4])return (void*)(uintptr_t)(bad==7?8:0x0e);return 0;
}
struct Object { const struct Rudroid54Class *klass; const char *text; };
static struct Object version={&classes[4],"6000.0.58f1"}, boxed={&classes[1],0}, name={&classes[4],0}, key={&classes[4],"M2H_lastLanguage"};
static void *handles[16];
static int rooted(void *obj) { for(int i=0;i<16;i++)if(handles[i]==obj)return 1;return 0; }
uintptr_t il2cpp_gchandle_new(void *obj,bool pinned) {
 (void)pinned;if(failure==2)return 0;for(int i=0;i<16;i++)if(!handles[i]){handles[i]=obj;++made;return ((uintptr_t)(i+1)<<32)|7;}assert(0);return 0;
}
void *il2cpp_gchandle_get_target(uintptr_t handle) { size_t i=(handle>>32)-1;assert(i<16&&handles[i]);return handles[i]; }
void il2cpp_gchandle_free(uintptr_t handle) { size_t i=(handle>>32)-1;assert(i<16&&handles[i]);handles[i]=0;++freed; }
void *il2cpp_object_get_class(void *obj) { return (void*)((struct Object*)obj)->klass; }
void *il2cpp_string_new(const char *value) { assert(!strcmp(value,key.text));return failure==5?0:&key; }
int32_t il2cpp_string_length(void *obj) { return (int32_t)strlen(((struct Object*)obj)->text); }
const uint16_t *il2cpp_string_chars(void *obj) {
 static uint16_t text[64];const char *s=((struct Object*)obj)->text;size_t i=0;
 do {text[i]=(unsigned char)s[i];}while(s[i++]&&i<64);return text;
}
void *il2cpp_thread_current(void) { return &current; }
void *il2cpp_runtime_invoke(void *m,void *self,void **args,void **exc) {
 *exc=0;
 if(m==&methods[6])return &version;
 if(m==&methods[2]) {
  if(failure==1){*exc=&boxed;return 0;}boxed.klass=&classes[failure==8?4:1];
  name.text=current==202?"ZH":current==31?"DE":"EN";return &boxed;
 }
 if(m==&methods[3]) {
  assert(self==&boxed&&rooted(self));if(failure==3){*exc=&boxed;return 0;}
  if(failure==4)name.text="202";
  if(callback){void(*cb)(void)=callback;callback=0;cb();}return &name;
 }
 if(m==&methods[4]) {
  assert(!self&&args[0]==&key&&args[1]==&name&&rooted(&key)&&rooted(&name));
  if(failure==6){*exc=&boxed;return 0;}++writes;strcpy(saved,name.text);return 0;
 }
 if(m==&methods[5]) { if(failure==7){*exc=&boxed;return 0;}++saves;return 0; }
 return legacy_invoke(m,self,args,exc);
}
