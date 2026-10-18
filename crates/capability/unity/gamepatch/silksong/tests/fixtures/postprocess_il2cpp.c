// INPUT:  base Silksong fixture, checked array/Behaviour calls and GC-root probes
// OUTPUT:  scene-start, postprocess, cancellation, exception and root-lifetime controls
// POS:    native ABI oracle for the Rust production postprocess path without game data
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
#define il2cpp_domain_get_assemblies legacy_domain_assemblies
#define il2cpp_class_get_method_from_name legacy_method
#define il2cpp_runtime_invoke legacy_invoke
#define il2cpp_method_get_flags legacy_flags
#define il2cpp_method_get_return_type legacy_return
#define il2cpp_method_get_param legacy_param
#define il2cpp_class_get_type legacy_class_type
#define il2cpp_object_get_class legacy_object_class
#define il2cpp_string_length legacy_string_length
#define il2cpp_string_chars legacy_string_chars
#define il2cpp_thread_current legacy_thread_current
#include "rudroid54_silksong_il2cpp.c"
#undef il2cpp_domain_get_assemblies
#undef il2cpp_class_get_method_from_name
#undef il2cpp_runtime_invoke
#undef il2cpp_method_get_flags
#undef il2cpp_method_get_return_type
#undef il2cpp_method_get_param
#undef il2cpp_class_get_type
#undef il2cpp_object_get_class
#undef il2cpp_string_length
#undef il2cpp_string_chars
#undef il2cpp_thread_current
#include <assert.h>
#include <stdio.h>
#include <string.h>

static int bad, failure, callback_phase, original_starts, disabled[5], created, freed;
static void (*callback)(void);
void rudroid56_set_bad(int value) { bad=value; }
void rudroid56_set_failure(int value) { failure=value; }
void rudroid56_set_callback(int phase,void (*value)(void)) { callback_phase=phase;callback=value; }
void rudroid56_start_body(void *self,void *method) { (void)self;(void)method;++original_starts; }
void rudroid56_placeholder(void) {}
static int scene;
void rudroid56_start(void) { rudroid56_start_body(&scene,0); }
int rudroid56_original_starts(void) { return original_starts; }
int rudroid56_disabled(int index) { return disabled[index]; }
int rudroid56_created(void) { return created; }
int rudroid56_freed(void) { return freed; }
static const struct Rudroid54Method scene_methods[]={{rudroid56_start_body,"Start",0}};
static const struct Rudroid54Method resource_methods[]={{rudroid56_placeholder,"FindObjectsOfTypeAll",1}};
static const struct Rudroid54Method behaviour_methods[]={{rudroid56_placeholder,"set_enabled",1}};
/* The wrong one-argument overload is deliberately first. */
static const struct Rudroid54Method array_methods[]={{rudroid56_placeholder,"GetValue",1},{rudroid56_placeholder,"GetValue",1},{rudroid56_placeholder,"get_Length",0}};
static const struct Rudroid54Method version_method={rudroid56_placeholder,"get_unityVersion",0};
static const struct Rudroid54Method object_methods[]={{rudroid56_placeholder,"FindObjectsOfType",1},{rudroid56_placeholder,"get_name",0}};
static const struct Rudroid54Method component_methods[]={{rudroid56_placeholder,"get_gameObject",0}};
static const struct Rudroid54Method gameobject_methods[]={{rudroid56_placeholder,"GetComponents",1}};
static const struct Rudroid54Method renderer_methods[]={{rudroid56_placeholder,"set_enabled",1}};
static const struct Rudroid54Class classes[]={
 {"","CustomSceneManager",scene_methods,1},
 {"UnityEngine","Resources",resource_methods,1},
 {"UnityEngine","Behaviour",behaviour_methods,1},
 {"System","Array",array_methods,3},
 {"System","Int32",0,0},{"System","Type",0,0},
 {"UnityStandardAssets.ImageEffects","BloomOptimized",0,0},
 {"","CameraBlurPlane",0,0},{"","LightBlur",0,0},
 {"","LightBlurredBackground",0,0},{"","UberPostprocess",0,0},
 {"System","Object[]",0,0},
 {"UnityEngine","Object",object_methods,2},
 {"UnityEngine","Component",component_methods,1},
 {"UnityEngine","GameObject",gameobject_methods,1},
 {"UnityEngine","Renderer",renderer_methods,1},
 {"UnityEngine","SpriteRenderer",0,0},{"UnityEngine","Transform",0,0},
 {"UnityEngine","Collider",0,0},{"UnityEngine","Animator",0,0},
 {"UnityEngine","MonoBehaviour",0,0},{"System","String",0,0},
};
static const struct Rudroid54Image image={"Postprocess",classes,22};
static const struct Rudroid54Assembly assembly={&image};
static const struct Rudroid54Assembly *const domain[]={&rudroid54_assemblies[0],&rudroid54_assemblies[1],&rudroid54_assemblies[2],&assembly};
void **il2cpp_domain_get_assemblies(void *d,size_t *count) { if(d!=(void*)rudroid54_domain)return 0;*count=4;return (void**)domain; }
void *il2cpp_class_get_method_from_name(void *k,const char *name,int argc) {
 if(rudroid54_streq(name,"get_unityVersion"))return (void*)&version_method;
 return legacy_method(k,name,argc);
}
void *il2cpp_class_get_parent(void *klass) {
 for(int i=6;i<=10;++i)if(klass==&classes[i])return bad==4?0:(void*)&classes[2];
 if(klass==&classes[11])return (void*)&classes[3];
 if(klass==&classes[16])return (void*)&classes[15];
 if(klass==&classes[15]||klass==&classes[17]||klass==&classes[18]||klass==&classes[19]||klass==&classes[20])return (void*)&classes[13];
 if(klass==&classes[13]||klass==&classes[14])return (void*)&classes[12];
 return 0;
}
void *il2cpp_class_get_type(void *klass) { return klass; }
void *il2cpp_class_from_type(void *type) {
 if(type==(void*)0x12)return (void*)&classes[5];
 if(type==(void*)0x212)return (void*)&classes[14];
 return type;
}
uint32_t il2cpp_method_get_flags(void *m,uint32_t *impl) {
 if(m==&version_method||m==&resource_methods[0]||m==&object_methods[0])return 0x10;
 if(m==&object_methods[1]||m==&component_methods[0]||m==&gameobject_methods[0]||m==&renderer_methods[0])return 0;
 if(m==&scene_methods[0]||m==&behaviour_methods[0]||m==&array_methods[0]||m==&array_methods[1]||m==&array_methods[2])return 0;
 return legacy_flags(m,impl);
}
void *il2cpp_method_get_return_type(void *m) {
 if(m==&version_method)return (void*)0x0e;
 if(m==&object_methods[0]||m==&gameobject_methods[0])return (void*)0x1d;
 if(m==&object_methods[1])return (void*)0x0e;
 if(m==&component_methods[0])return (void*)(uintptr_t)(bad==9?0x112:0x212);
 if(m==&renderer_methods[0])return (void*)1;
 if(m==&resource_methods[0])return (void*)(uintptr_t)(bad==1?0x11:0x1d);
 if(m==&array_methods[0]||m==&array_methods[1])return (void*)0x1c;
 if(m==&array_methods[2])return (void*)(uintptr_t)(bad==2?0x108:8);
 if(m==&behaviour_methods[0])return (void*)(uintptr_t)(bad==3?2:1);
 if(m==&scene_methods[0])return (void*)1;
 return legacy_return(m);
}
void *il2cpp_method_get_param(void *m,uint32_t i) {
 if(m==&resource_methods[0])return i?0:(void*)0x12;
 if(m==&object_methods[0]||m==&gameobject_methods[0])return i?0:(void*)0x12;
 if(m==&renderer_methods[0])return i?0:(void*)2;
 if(m==&array_methods[0])return i?0:(void*)0x0a;
 if(m==&array_methods[1])return i?0:(void*)(uintptr_t)(bad==5?0x108:8);
 if(m==&behaviour_methods[0])return i?0:(void*)2;
 return legacy_param(m,i);
}
struct Object { const struct Rudroid54Class *klass; int group; int value; };
static struct Object array={&classes[11],0,3},length={&classes[4],0,3};
static struct Object objects[6]={{&classes[6],0,0},{&classes[6],0,0},{&classes[7],1,0},{&classes[8],2,0},{&classes[9],3,0},{&classes[10],4,0}};
struct Name {struct Object object;int32_t length;uint16_t units[64];};
static struct Object sprite_array={&classes[11],100,120};
static struct Object sprites[120],gameobjects[120],transforms[120],component_arrays[120],collider={&classes[18],0,0};
static struct Name names[120];
static int sprite_case,sprite_disabled;
void rudroid57_setup(int which) {
 sprite_case=which;sprite_disabled=0;
 for(int i=0;i<120;++i) {
  sprites[i]=(struct Object){&classes[16],i,0};gameobjects[i]=(struct Object){&classes[14],i,0};
  transforms[i]=(struct Object){&classes[17],i,0};component_arrays[i]=(struct Object){&classes[11],200+i,2};
  names[i].object=(struct Object){&classes[21],i,0};char text[64];
  snprintf(text,sizeof(text),"moss_prop_%d",i);
  if(which==1) {
   if(i%6==1)snprintf(text,sizeof(text),"Hero_%d",i);
   if(i%6==2||i%6==3||i%6==4)component_arrays[i].value=3;
  }
  if(which==2)component_arrays[i].value=3;
  if(which==3)snprintf(text,sizeof(text),"MOSS_%d",i);
  if(which==4)snprintf(text,sizeof(text),"Moss_%d",i);
  names[i].length=(int32_t)strlen(text);
  for(int j=0;j<names[i].length;++j)names[i].units[j]=(unsigned char)text[j];
  if(which==6) {
   names[i].length=7;uint16_t unicode[]={109,111,115,115,95,0xd83c,0xdf3f};
   for(int j=0;j<7;++j)names[i].units[j]=unicode[j];
  }
  if(which==8) {names[i].length=6;names[i].units[5]=0xd83c;}
 }
}
int rudroid57_disabled(void) {return sprite_disabled;}
int rudroid57_disabled_index(int i) {return sprites[i].value;}
static void *handles[64];
uintptr_t il2cpp_gchandle_new(void *obj,bool pinned) {
 assert(!pinned);
 if(failure==7)return 0;
 for(int i=0;i<64;++i)if(!handles[i]){handles[i]=obj;++created;return (uintptr_t)(i+1)<<32;}
 assert(0);return 0;
}
void *il2cpp_gchandle_get_target(uintptr_t handle) { size_t i=(handle>>32)-1;assert(i<64&&handles[i]);return handles[i]; }
void il2cpp_gchandle_free(uintptr_t handle) { size_t i=(handle>>32)-1;assert(i<64&&handles[i]);handles[i]=0;++freed; }
static bool rooted(void *obj) { for(int i=0;i<64;++i)if(handles[i]==obj)return true;return false; }
void *il2cpp_object_get_class(void *obj) { assert(rooted(obj));return (void*)((struct Object*)obj)->klass; }
void *il2cpp_object_unbox(void *obj) { assert(rooted(obj)&&obj==&length);return &length.value; }
static uint16_t version_units[]={54,48,48,48,46,48,46,53,56,102,49};
static int version_object;
int32_t il2cpp_string_length(void *obj) {
 if(obj==&version_object)return 11;
 assert(rooted(obj)&&((struct Object*)obj)->klass==&classes[21]);return ((struct Name*)obj)->length;
}
const uint16_t *il2cpp_string_chars(void *obj) {
 if(obj==&version_object)return version_units;
 assert(rooted(obj)&&((struct Object*)obj)->klass==&classes[21]);return ((struct Name*)obj)->units;
}
void *il2cpp_thread_current(void) { return &scene; }
static void reenter(int phase) { if(callback&&callback_phase==phase){void (*fn)(void)=callback;callback=0;fn();} }
void *il2cpp_runtime_invoke(void *m,void *self,void **args,void **exc) {
 *exc=0;
 if(m==&version_method)return &version_object;
 if(m==&object_methods[0]) {assert(rooted(args[0])&&args[0]==&classes[16]);reenter(1);return &sprite_array;}
 if(m==&component_methods[0]) {
  assert(rooted(self));reenter(4);if(failure==9){*exc=&scene;return 0;}
  return &gameobjects[((struct Object*)self)->group];
 }
 if(m==&gameobject_methods[0]) {
  assert(rooted(self)&&rooted(args[0])&&args[0]==&classes[13]);reenter(5);
  if(failure==10){*exc=&scene;return 0;}return &component_arrays[((struct Object*)self)->group];
 }
 if(m==&object_methods[1]) {
  assert(rooted(self));reenter(6);if(failure==11){*exc=&scene;return 0;}
  if(failure==12)return &length;return &names[((struct Object*)self)->group];
 }
 if(m==&renderer_methods[0]) {
  assert(rooted(self)&&!*(bool*)args[0]);reenter(7);if(failure==13){*exc=&scene;return 0;}
  ++sprite_disabled;++((struct Object*)self)->value;return 0;
 }
 if(m==&resource_methods[0]) {
  assert(rooted(args[0]));
  for(int i=6;i<=10;++i)if(args[0]==&classes[i])array.group=i-6;
  reenter(1);
  if(failure==1){*exc=&scene;return 0;}
  if(failure==8)array.klass=&classes[4];
  return &array;
 }
 if(m==&array_methods[2]) { assert(rooted(self));length.value=failure==6?-1:((struct Object*)self)->value;return &length; }
 if(m==&array_methods[1]) {
  assert(rooted(self));int index=*(int32_t*)args[0];struct Object *a=self;assert(index>=0&&index<a->value);reenter(2);
  if(a->group==100) {
   if(failure==14)return &length;
   return &sprites[index];
  }
  if(a->group>=200) {
   int i=a->group-200;
   if(index==1)return &sprites[sprite_case==7?(i+1)%120:i];
   if(sprite_case==5||index==2)return &collider;
   return &transforms[i];
  }
  if(failure==2){*exc=&scene;return 0;}
  if(index==1)return 0;
  if(array.group==0)return &objects[index==0?0:1];
  if(index==2||array.group==4)return 0;
  if(failure==4)return &length;
  return &objects[array.group+1];
 }
 if(m==&behaviour_methods[0]) {
  assert(rooted(self));assert(!*(bool*)args[0]);reenter(3);
  if(failure==3){*exc=&scene;return 0;}
  ++disabled[((struct Object*)self)->group];return 0;
 }
 return legacy_invoke(m,self,args,exc);
}
