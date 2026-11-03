// INPUT:  existing Silksong IL2CPP fixture and native callback probes
// OUTPUT:  checked launcher metadata, button/scene/mobile-object calls and root/failure/reentry controls
// POS:    exercises startup transitions through the production hooks without game offsets
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
#define il2cpp_domain_get_assemblies legacy_domain_assemblies
#define il2cpp_class_get_type legacy_class_type
#define il2cpp_class_get_field_from_name legacy_class_field
#define il2cpp_field_get_value legacy_field_value
#define il2cpp_runtime_invoke legacy_invoke
#define il2cpp_method_get_flags legacy_method_flags
#define il2cpp_method_get_return_type legacy_return_type
#define il2cpp_method_get_param legacy_method_param
#define il2cpp_type_get_type legacy_type_kind
#define il2cpp_type_is_byref legacy_type_byref
#define il2cpp_class_get_method_from_name legacy_class_method
#define il2cpp_string_new legacy_string_new
#define il2cpp_object_get_class legacy_object_class
#define il2cpp_string_length legacy_string_length
#define il2cpp_string_chars legacy_string_chars
#include "rudroid54_silksong_il2cpp.c"
#undef il2cpp_domain_get_assemblies
#undef il2cpp_class_get_type
#undef il2cpp_class_get_field_from_name
#undef il2cpp_field_get_value
#undef il2cpp_runtime_invoke
#undef il2cpp_method_get_flags
#undef il2cpp_method_get_return_type
#undef il2cpp_method_get_param
#undef il2cpp_type_get_type
#undef il2cpp_type_is_byref
#undef il2cpp_class_get_method_from_name
#undef il2cpp_string_new
#undef il2cpp_object_get_class
#undef il2cpp_string_length
#undef il2cpp_string_chars
#include <assert.h>
#include <string.h>

static int launcher_bad, launcher_error, clicks, loads, deactivations, cinematic_begins, cinematic_skips;
static int prompt_mode, prompt_queries, prompt_hidden, prompt_visible, prompt_field;
static int prompt_sequence, prompt_hide_order, prompt_click_order;
void rudroid59_set_mode(int v) { prompt_mode=v; }
int rudroid59_queries(void) { return prompt_queries; }
int rudroid59_hidden(void) { return prompt_hidden; }
int rudroid59_visible(void) { return prompt_visible; }
int rudroid59_hide_order(void) { return prompt_hide_order; }
int rudroid59_click_order(void) { return prompt_click_order; }
static int cinematic_receiver;
void rudroid55_cinematic_begin(void* receiver,void* metadata) { (void)receiver;(void)metadata;++cinematic_begins; }
void rudroid55_cinematic_skip(void* receiver,void* metadata) { (void)receiver;(void)metadata; }
void rudroid55_call_cinematic(void) { rudroid55_cinematic_begin(&cinematic_receiver,0); }
int rudroid55_cinematic_begins(void) { return cinematic_begins; }
int rudroid55_cinematic_skips(void) { return cinematic_skips; }
static void (*launcher_callback)(void);
static int receiver, button_object, click_object, game_object, handle_object, field_object;
void rudroid55_set_bad(int v) { launcher_bad=v; }
void rudroid55_set_error(int v) { launcher_error=v; }
void rudroid55_set_callback(void (*v)(void)) { launcher_callback=v; }
int rudroid55_clicks(void) { return clicks; }
int rudroid55_loads(void) { return loads; }
int rudroid55_deactivations(void) { return deactivations; }
void rudroid55_start(void* self, void* mi) { (void)self; (void)mi; prompt_visible=1; }
void rudroid55_update(void* self, void* mi) { (void)self; (void)mi; }
void rudroid55_placeholder(void) {}
void rudroid55_call_start(void) { rudroid55_start(&receiver, 0); }
void rudroid55_call_update(void) { rudroid55_update(&receiver, 0); }

static const struct Rudroid54Method launcher_methods[]={{rudroid55_start,"Start",0},{rudroid55_update,"Update",0}};
static const struct Rudroid54Method button_methods[]={{rudroid55_placeholder,"get_onClick",0}};
static const struct Rudroid54Method event_methods[]={{rudroid55_placeholder,"Invoke",0}};
static const struct Rudroid54Method component_methods[]={{rudroid55_placeholder,"get_gameObject",0}};
static const struct Rudroid54Method object_methods[]={{rudroid55_placeholder,"SetActive",1},{rudroid55_placeholder,"Find",1},{rudroid55_placeholder,"Find",1}};
static const struct Rudroid54Method resource_methods[]={{rudroid55_placeholder,"FindObjectsOfTypeAll",1}};
static const struct Rudroid54Method behaviour_methods[]={{rudroid55_placeholder,"set_enabled",1}};
static const struct Rudroid54Method array_methods[]={{rudroid55_placeholder,"get_Length",0},{rudroid55_placeholder,"GetValue",1}};
static const struct Rudroid54Method version_method={rudroid55_placeholder,"get_unityVersion",0};
static const struct Rudroid54Method address_methods[]={{rudroid55_placeholder,"LoadSceneAsync",5}};
static const struct Rudroid54Method cinematic_methods[]={{rudroid55_cinematic_begin,"Begin",0},{rudroid55_cinematic_skip,"Skip",0}};
static const struct Rudroid54Class extra_classes[]={
 {"","GameLauncher",launcher_methods,2},
 {"UnityEngine.UI","Button",button_methods,1},
 {"UnityEngine.UI","ButtonClickedEvent",event_methods,1},
 {"UnityEngine","Component",component_methods,1},
 {"UnityEngine","GameObject",object_methods,3},
 {"UnityEngine.AddressableAssets","Addressables",address_methods,1},
 {"UnityEngine.SceneManagement","LoadSceneMode",0,0},
 {"UnityEngine.ResourceManagement.ResourceProviders","SceneReleaseMode",0,0},
 {"UnityEngine.ResourceManagement.AsyncOperations","AsyncOperationHandle`1",0,0},
 {"","CinematicSequence",cinematic_methods,2},
 {"UnityEngine","Resources",resource_methods,1},
 {"UnityEngine","Behaviour",behaviour_methods,1},
 {"System","Array",array_methods,2},{"System","Int32",0,0},
 {"System","Type",0,0},{"System","String",0,0},{"System","Object[]",0,0},
};
static const struct Rudroid54Image extra_image={"Launcher",extra_classes,17};
static const struct Rudroid54Assembly extra_assembly={&extra_image};
static const struct Rudroid54Assembly* const extended_domain[]={&rudroid54_assemblies[0],&rudroid54_assemblies[1],&rudroid54_assemblies[2],&extra_assembly};
void** il2cpp_domain_get_assemblies(void* domain,size_t* count) {
 if(domain!=(void*)rudroid54_domain)return 0;
 *count=4;return (void**)extended_domain;
}
struct T { int kind; const struct Rudroid54Class* klass; int byref; };
static const struct T types[]={
 {0x12,&extra_classes[1],0},{0x12,&extra_classes[2],0},{0x12,&extra_classes[4],0},
 {0x11,&extra_classes[6],0},{0x11,&extra_classes[7],0},{0x15,&extra_classes[8],0},
 {8,0,0},{0x1c,0,0},{2,0,0},{1,0,0},{0x1c,0,1},{0x12,&extra_classes[1],1},{10,0,0},
 {0x0e,&extra_classes[15],0},{0x12,&extra_classes[14],0},{0x1d,&extra_classes[16],0},
 {0x0e,&extra_classes[15],1},
};
static const struct T* type_record(void* raw) {
 for(size_t i=0;i<sizeof(types)/sizeof(types[0]);++i)if(raw==&types[i])return &types[i];
 return 0;
}
void* il2cpp_class_get_type(void* klass) {
 for(size_t i=0;i<6;++i)if(types[i].klass==klass)return (void*)&types[i];
 return legacy_class_type(klass);
}
void* il2cpp_class_from_type(void* raw) {
 const struct T* ty=type_record(raw);return ty?(void*)ty->klass:0;
}
void* il2cpp_class_enum_basetype(void* klass) {
 return klass==&extra_classes[6]||klass==&extra_classes[7]?(void*)&types[launcher_bad==3?12:6]:0;
}
int32_t il2cpp_type_get_type(void* raw) {
 const struct T* ty=type_record(raw);return ty?ty->kind:legacy_type_kind(raw);
}
bool il2cpp_type_is_byref(void* raw) {
 const struct T* ty=type_record(raw);return ty?ty->byref:legacy_type_byref(raw);
}
void* il2cpp_class_get_field_from_name(void* klass,const char* name) {
 if(klass==&extra_classes[0]&&rudroid54_streq(name,"MsgBox")&&prompt_mode)return &prompt_field;
 return klass==&extra_classes[0]&&rudroid54_streq(name,"BtnOk")?&field_object:0;
}
void* il2cpp_field_get_type(void* field) {
 if(field==&prompt_field)return (void*)&types[prompt_mode==6?6:prompt_mode==8?11:2];
 assert(field==&field_object);return (void*)&types[launcher_bad==1?6:launcher_bad==2?11:0];
}
int32_t il2cpp_field_get_flags(void* field) {
 if(field==&prompt_field)return prompt_mode==7?0x10:0;
 assert(field==&field_object);return launcher_bad==4?0x10:0;
}
void il2cpp_field_get_value(void* object,void* field,void* out) {
 assert(object==&receiver&&field==&field_object);*(void**)out=&button_object;
}
static int new_method(void* raw) {
 if(raw==&version_method)return 1;
 for(size_t i=0;i<17;++i)for(int j=0;j<extra_classes[i].method_count;++j)
  if(raw==&extra_classes[i].methods[j])return 1;
 return 0;
}
uint32_t il2cpp_method_get_flags(void* raw,uint32_t* impl) {
 if(!new_method(raw))return legacy_method_flags(raw,impl);
 if(impl)*impl=0;
 extern int bootstrap_bad;
 if(raw==&object_methods[2]&&bootstrap_bad==3)return 0;
 return raw==&address_methods[0]||raw==&object_methods[1]||raw==&object_methods[2]||raw==&resource_methods[0]||raw==&version_method?0x10:0;
}
void* il2cpp_method_get_return_type(void* raw) {
 if(!new_method(raw))return legacy_return_type(raw);
 if(raw==&cinematic_methods[0]&&launcher_bad==7)return (void*)&types[8];
 if(raw==&button_methods[0])return (void*)&types[1];
 if(raw==&component_methods[0])return (void*)&types[2];
 if(raw==&address_methods[0])return (void*)&types[launcher_bad==6?6:5];
 extern int bootstrap_bad;
 if(raw==&object_methods[1]||raw==&object_methods[2])return (void*)&types[bootstrap_bad==1?13:2];
 if(raw==&version_method)return (void*)&types[13];
 if(raw==&resource_methods[0])return (void*)&types[15];
 if(raw==&array_methods[0])return (void*)&types[6];
 if(raw==&array_methods[1])return (void*)&types[7];
 return (void*)&types[9];
}
void* il2cpp_method_get_param(void* raw,uint32_t index) {
 if(!new_method(raw))return legacy_method_param(raw,index);
 if(raw==&object_methods[0])return index==0?(void*)&types[8]:0;
 if(raw==&address_methods[0]&&index<5){static int order[]={7,3,8,6,4};return (void*)&types[index==0&&launcher_bad==5?10:order[index]];}
 extern int bootstrap_bad;
 if(raw==&object_methods[1])return index?0:(void*)&types[6];
 if(raw==&object_methods[2])return index?0:(void*)&types[bootstrap_bad==2?16:13];
 if(raw==&resource_methods[0])return index?0:(void*)&types[14];
 if(raw==&behaviour_methods[0])return index?0:(void*)&types[8];
 if(raw==&array_methods[1])return index?0:(void*)&types[6];
 return 0;
}
int bootstrap_bad;
static int bootstrap_error, bootstrap_find_calls, bootstrap_disabled[15], bootstrap_created, bootstrap_freed, bootstrap_string_calls;
static int bootstrap_callback_phase;
static void (*bootstrap_callback)(void);
static const char *const bootstrap_names[]={"Touch Camera","Stick","Jump_Action1","Cast_Action2","Attack_Action3","DownAttack_Action5","Dash_RightTrigger","QuickCast_RightBumper","QuickMap_LeftBumper","SuperDash_LeftTrigger","DreamNail_Action4","Taunt_RightStickButton","Options","View","WaterMark"};
struct BootstrapObject {const struct Rudroid54Class *klass;int index;};
struct BootstrapString {const struct Rudroid54Class *klass;char text[64];};
static struct BootstrapObject bootstrap_objects[15],bootstrap_wrong,prompt_object;
static struct BootstrapString bootstrap_strings[32];
static int bootstrap_version;
static void *bootstrap_handles[64];
void rudroid58_set_bad(int v) {bootstrap_bad=v;}
void rudroid58_set_error(int v) {bootstrap_error=v;}
void rudroid58_set_callback(int phase,void (*fn)(void)) {bootstrap_callback_phase=phase;bootstrap_callback=fn;}
int rudroid58_find_calls(void) {return bootstrap_find_calls;}
int rudroid58_string_calls(void) {return bootstrap_string_calls;}
int rudroid58_disabled(int i) {assert(i>=0&&i<15);return bootstrap_disabled[i];}
int rudroid58_created(void) {return bootstrap_created;}
int rudroid58_freed(void) {return bootstrap_freed;}
static void bootstrap_reenter(int phase) {if(bootstrap_callback&&phase==bootstrap_callback_phase){void(*fn)(void)=bootstrap_callback;bootstrap_callback=0;fn();}}
static bool bootstrap_rooted(void *p) {for(int i=0;i<64;++i)if(bootstrap_handles[i]==p)return true;return false;}
uintptr_t il2cpp_gchandle_new(void *object,bool pinned) {
 assert(!pinned);
 if(bootstrap_error==5)return 0;
 for(int i=0;i<15;++i)if(object==&bootstrap_objects[i]&&bootstrap_error==8)return 0;
 for(int i=0;i<64;++i)if(!bootstrap_handles[i]){bootstrap_handles[i]=object;++bootstrap_created;return (uintptr_t)(i+1)<<32;}
 assert(0);return 0;
}
void *il2cpp_gchandle_get_target(uintptr_t handle) {size_t i=(handle>>32)-1;assert(i<64&&bootstrap_handles[i]);return bootstrap_handles[i];}
void il2cpp_gchandle_free(uintptr_t handle) {size_t i=(handle>>32)-1;assert(i<64&&bootstrap_handles[i]);bootstrap_handles[i]=0;++bootstrap_freed;}
void *il2cpp_object_get_class(void *object) {assert(bootstrap_rooted(object));return (void*)((struct BootstrapObject*)object)->klass;}
void *il2cpp_class_get_method_from_name(void *klass,const char *name,int argc) {
 if(rudroid54_streq(name,"get_unityVersion")&&argc==0)return (void*)&version_method;
 return legacy_class_method(klass,name,argc);
}
void *il2cpp_string_new(const char *text) {
 if(strncmp(text,"Scenes/",7)==0)return legacy_string_new(text);
 ++bootstrap_string_calls;
 if(bootstrap_error==7||strlen(text)>=64)return 0;
 struct BootstrapString *s=&bootstrap_strings[(bootstrap_string_calls-1)%32];s->klass=&extra_classes[15];strcpy(s->text,text);
 bootstrap_wrong=(struct BootstrapObject){&extra_classes[13],0};bootstrap_reenter(1);
 return bootstrap_error==4?(void*)&bootstrap_wrong:s;
}
static uint16_t bootstrap_version_units[]={54,48,48,48,46,48,46,53,56,102,49};
int32_t il2cpp_string_length(void *p) {return p==&bootstrap_version?11:legacy_string_length(p);}
const uint16_t *il2cpp_string_chars(void *p) {if(p!=&bootstrap_version)return legacy_string_chars(p);bootstrap_version_units[8]=bootstrap_bad==4?55:56;return bootstrap_version_units;}

void* il2cpp_runtime_invoke(void* raw,void* self,void** args,void** exc) {
 if(!new_method(raw))return legacy_invoke(raw,self,args,exc);
 if(exc)*exc=0;
 if(raw==&version_method)return &bootstrap_version;
 if(raw==&object_methods[1]){assert(0);return 0;}
 if(raw==&object_methods[2]) {
  assert(!self&&bootstrap_rooted(args[0]));
  const char *name=((struct BootstrapString*)args[0])->text;
  if(rudroid54_streq(name,"/Launcher/MsgBox/Panel")) {
   ++prompt_queries;
   if(prompt_mode==2)return 0;
   if(prompt_mode==3)return &bootstrap_wrong;
   if(prompt_mode==4){*exc=&field_object;return 0;}
   prompt_object=(struct BootstrapObject){&extra_classes[4],-1};return &prompt_object;
  }
  ++bootstrap_find_calls;bootstrap_reenter(2);
  if(bootstrap_error==2){*exc=&field_object;return 0;}
  if(bootstrap_error==3)return &bootstrap_wrong;
  for(int i=0;i<15;++i)if(rudroid54_streq(name,bootstrap_names[i])) {
   if(bootstrap_error==1&&i==13)return 0;
   bootstrap_objects[i]=(struct BootstrapObject){&extra_classes[4],i};return &bootstrap_objects[i];
  }
  return 0;
 }
 if(raw==&cinematic_methods[1]){assert(self==&cinematic_receiver);++cinematic_skips;if(launcher_callback){void(*cb)(void)=launcher_callback;launcher_callback=0;cb();}if(launcher_error==4&&exc)*exc=&field_object;return 0;}
 if(raw==&button_methods[0]){assert(self==&button_object);return &click_object;}
 if(raw==&event_methods[0]){
  assert(self==&click_object);++clicks;prompt_click_order=++prompt_sequence;
  if(launcher_callback){void(*cb)(void)=launcher_callback;launcher_callback=0;cb();}
  if(launcher_error==1&&exc)*exc=&field_object;
  return 0;
 }
 if(raw==&address_methods[0]){
  assert(!self&&rudroid54_streq(args[0],"Scenes/Pre_Menu_Intro"));
  assert(*(int32_t*)args[1]==0&&*(bool*)args[2]&&*(int32_t*)args[3]==100&&*(int32_t*)args[4]==0);
  ++loads;if(launcher_error==2){if(exc)*exc=&field_object;return 0;}return &handle_object;
 }
 if(raw==&component_methods[0]){assert(self==&receiver);return &game_object;}
 if(raw==&object_methods[0]) {
  assert(!*(bool*)args[0]);
  if(self==&game_object){++deactivations;return 0;}
  assert(bootstrap_rooted(self));if(bootstrap_error==6){*exc=&field_object;return 0;}
  if(self==&prompt_object){
   if(prompt_mode==5){*exc=&field_object;return 0;}
   ++prompt_hidden;prompt_visible=0;prompt_hide_order=++prompt_sequence;bootstrap_reenter(3);return 0;
  }
  ++bootstrap_disabled[((struct BootstrapObject*)self)->index];bootstrap_reenter(3);return 0;
 }
 return 0;
}
