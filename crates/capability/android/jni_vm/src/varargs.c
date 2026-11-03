// INPUT: JNI method descriptor kinds, native va_list and the shared JNIEnv table
// OUTPUT: correctly promoted jvalue arguments forwarded to the same A dispatcher
// POS: private ABI-only adapter; no Java objects, catalog, allocation or services
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#if !defined(__aarch64__) || !defined(__GLIBC__)
#error JNI variadic adapter requires the audited ARM64 GNU ABI
#endif
typedef union {
    uint8_t z; int8_t b; uint16_t c; int16_t s; int32_t i;
    int64_t j; float f; double d; void *l;
} jvalue;
_Static_assert(sizeof(va_list)==32, "ARM64 va_list layout");
_Static_assert(sizeof(jvalue)==8 && _Alignof(jvalue)==8, "JNI jvalue layout");

/* Rust validates the live method and writes at most capacity descriptor kinds.
 * JNI permits at most 255 parameter slots; no heap or descriptor copy is needed. */
extern int rudroid_jni_argument_kinds(void *method, char *out, size_t capacity);
static int collect(void *method, va_list input, jvalue *out) {
    char kinds[255];
    int count=rudroid_jni_argument_kinds(method,kinds,sizeof(kinds));
    if (count<0 || (size_t)count>sizeof(kinds)) return 0;
    va_list args;
    va_copy(args,input);
    for (int i=0;i<count;++i) {
        switch (kinds[i]) {
            case 'Z': out[i].z=(uint8_t)va_arg(args,int); break;
            case 'B': out[i].b=(int8_t)va_arg(args,int); break;
            case 'C': out[i].c=(uint16_t)va_arg(args,int); break;
            case 'S': out[i].s=(int16_t)va_arg(args,int); break;
            case 'I': out[i].i=va_arg(args,int); break;
            case 'J': out[i].j=va_arg(args,int64_t); break;
            case 'F': out[i].f=(float)va_arg(args,double); break;
            case 'D': out[i].d=va_arg(args,double); break;
            case 'L': out[i].l=va_arg(args,void *); break;
            default: va_end(args); return 0;
        }
    }
    va_end(args);
    return 1;
}

#define CALL(NAME, TYPE, SLOT) \
TYPE rudroid_jni_##NAME##_v(void *env,void *object,void *method,va_list args) { \
    jvalue values[255]; \
    if (!collect(method,args,values)) return (TYPE)0; \
    void **table=*(void ***)env; \
    return ((TYPE (*)(void *,void *,void *,const jvalue *))table[SLOT])(env,object,method,values); \
} \
TYPE rudroid_jni_##NAME(void *env,void *object,void *method,...) { \
    va_list args; va_start(args,method); \
    TYPE value=rudroid_jni_##NAME##_v(env,object,method,args); \
    va_end(args); return value; \
}
CALL(object,void *,36)
CALL(boolean,uint8_t,39)
CALL(byte,int8_t,42)
CALL(char,uint16_t,45)
CALL(short,int16_t,48)
CALL(int,int32_t,51)
CALL(long,int64_t,54)
CALL(float,float,57)
CALL(double,double,60)
CALL(static_object,void *,116)
CALL(static_boolean,uint8_t,119)
CALL(static_byte,int8_t,122)
CALL(static_char,uint16_t,125)
CALL(static_short,int16_t,128)
CALL(static_int,int32_t,131)
CALL(static_long,int64_t,134)
CALL(static_float,float,137)
CALL(static_double,double,140)
CALL(new_object,void *,30)

#define VOID_CALL(NAME,SLOT) \
void rudroid_jni_##NAME##_v(void *env,void *object,void *method,va_list args) { \
    jvalue values[255]; if (!collect(method,args,values)) return; \
    void **table=*(void ***)env; \
    ((void (*)(void *,void *,void *,const jvalue *))table[SLOT])(env,object,method,values); \
} \
void rudroid_jni_##NAME(void *env,void *object,void *method,...) { \
    va_list args; va_start(args,method); rudroid_jni_##NAME##_v(env,object,method,args); va_end(args); \
}
VOID_CALL(void,63)
VOID_CALL(static_void,143)

/* The nonvirtual class is an ABI parameter, not a Java method argument.
 * Rust validates its method provenance in the same A dispatcher. */
#define NONVIRTUAL_CALL(NAME,TYPE,SLOT) \
TYPE rudroid_jni_nonvirtual_##NAME##_v(void *env,void *object,void *clazz,void *method,va_list args) { \
    jvalue values[255]; if (!collect(method,args,values)) return (TYPE)0; \
    void **table=*(void ***)env; \
    return ((TYPE (*)(void *,void *,void *,void *,const jvalue *))table[SLOT])(env,object,clazz,method,values); \
} \
TYPE rudroid_jni_nonvirtual_##NAME(void *env,void *object,void *clazz,void *method,...) { \
    va_list args; va_start(args,method); \
    TYPE result=rudroid_jni_nonvirtual_##NAME##_v(env,object,clazz,method,args); \
    va_end(args); return result; \
}
NONVIRTUAL_CALL(object,void *,66)
NONVIRTUAL_CALL(boolean,uint8_t,69)
NONVIRTUAL_CALL(byte,int8_t,72)
NONVIRTUAL_CALL(char,uint16_t,75)
NONVIRTUAL_CALL(short,int16_t,78)
NONVIRTUAL_CALL(int,int32_t,81)
NONVIRTUAL_CALL(long,int64_t,84)
NONVIRTUAL_CALL(float,float,87)
NONVIRTUAL_CALL(double,double,90)

void rudroid_jni_nonvirtual_void_v(void *env,void *object,void *clazz,void *method,va_list args) {
    jvalue values[255]; if (!collect(method,args,values)) return;
    void **table=*(void ***)env;
    ((void (*)(void *,void *,void *,void *,const jvalue *))table[93])(env,object,clazz,method,values);
}
void rudroid_jni_nonvirtual_void(void *env,void *object,void *clazz,void *method,...) {
    va_list args;va_start(args,method);rudroid_jni_nonvirtual_void_v(env,object,clazz,method,args);va_end(args);
}
