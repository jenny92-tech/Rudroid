// INPUT: real JNI tables, native scalar method IDs and the calling object/class
// OUTPUT: A, variadic and compiler-owned va_list width/bit-pattern checks
// POS: separate private archive member referenced only by Rust cfg(test) unit code
#include "jni.h"
#include <stdarg.h>
#include <stdint.h>
#include <string.h>

#define V_CALL(NAME,TYPE) \
static TYPE NAME##_v(JNIEnv *env,jobject object,jmethodID method,...) { \
    va_list args;va_start(args,method);TYPE result=(*env)->Call##NAME##MethodV(env,object,method,args);va_end(args);return result; \
} \
static TYPE NAME##_static_v(JNIEnv *env,jclass clazz,jmethodID method,...) { \
    va_list args;va_start(args,method);TYPE result=(*env)->CallStatic##NAME##MethodV(env,clazz,method,args);va_end(args);return result; \
} \
static TYPE NAME##_nonvirtual_v(JNIEnv *env,jobject object,jclass clazz,jmethodID method,...) { \
    va_list args;va_start(args,method);TYPE result=(*env)->CallNonvirtual##NAME##MethodV(env,object,clazz,method,args);va_end(args);return result; \
}
V_CALL(Byte,jbyte)
V_CALL(Char,jchar)
V_CALL(Short,jshort)
V_CALL(Double,jdouble)

#define CHECK(TYPE,NAME,FIELD,VALUE,METHOD,STEP) do { \
    TYPE value=(VALUE);jvalue args={0};args.FIELD=value; \
    if ((*env)->Call##NAME##MethodA(env,object,METHOD,&args)!=value) return STEP; \
    if ((*env)->CallStatic##NAME##MethodA(env,clazz,METHOD,&args)!=value) return STEP+1; \
    if ((*env)->Call##NAME##Method(env,object,METHOD,value)!=value) return STEP+2; \
    if ((*env)->CallStatic##NAME##Method(env,clazz,METHOD,value)!=value) return STEP+3; \
    if (NAME##_v(env,object,METHOD,value)!=value) return STEP+4; \
    if (NAME##_static_v(env,clazz,METHOD,value)!=value) return STEP+5; \
    if ((*env)->CallNonvirtual##NAME##MethodA(env,object,clazz,METHOD,&args)!=value) return STEP+7; \
    if ((*env)->CallNonvirtual##NAME##Method(env,object,clazz,METHOD,value)!=value) return STEP+8; \
    if (NAME##_nonvirtual_v(env,object,clazz,METHOD,value)!=value) return STEP+9; \
    if ((*env)->ExceptionCheck(env)) return STEP+6; \
} while (0)

int rudroid_test_scalar_calls(JNIEnv *env,jobject object,jclass clazz,jmethodID byte,
    jmethodID character,jmethodID short_method,jmethodID double_method) {
    CHECK(jbyte,Byte,b,(jbyte)-128,byte,10);
    CHECK(jchar,Char,c,(jchar)65535,character,20);
    CHECK(jshort,Short,s,(jshort)-32768,short_method,30);
    const uint64_t patterns[]={UINT64_C(0x8000000000000000),UINT64_C(0x7ff8123456789abc),UINT64_C(0x4132d68720000000)};
    for (size_t i=0;i<sizeof(patterns)/sizeof(patterns[0]);++i) {
        jdouble value;memcpy(&value,&patterns[i],sizeof(value));jvalue args={0};args.d=value;
        jdouble results[]={(*env)->CallDoubleMethodA(env,object,double_method,&args),
            (*env)->CallStaticDoubleMethodA(env,clazz,double_method,&args),
            (*env)->CallDoubleMethod(env,object,double_method,value),
            (*env)->CallStaticDoubleMethod(env,clazz,double_method,value),
            Double_v(env,object,double_method,value),Double_static_v(env,clazz,double_method,value),
            (*env)->CallNonvirtualDoubleMethodA(env,object,clazz,double_method,&args),
            (*env)->CallNonvirtualDoubleMethod(env,object,clazz,double_method,value),
            Double_nonvirtual_v(env,object,clazz,double_method,value)};
        for (size_t j=0;j<sizeof(results)/sizeof(results[0]);++j) {
            uint64_t bits;memcpy(&bits,&results[j],sizeof(bits));if(bits!=patterns[i])return 40+(int)(i*6+j);
        }
        if ((*env)->ExceptionCheck(env))return 80;
    }
    return 0;
}

#define NOARG_V(NAME,TYPE) \
static TYPE nonvirtual_##NAME##_v(JNIEnv *env,jobject object,jclass clazz,jmethodID method,...) { \
    va_list args;va_start(args,method);TYPE value=(*env)->CallNonvirtual##NAME##MethodV(env,object,clazz,method,args);va_end(args);return value; \
}
NOARG_V(Object,jobject)
NOARG_V(Boolean,jboolean)
NOARG_V(Byte,jbyte)
NOARG_V(Char,jchar)
NOARG_V(Short,jshort)
NOARG_V(Int,jint)
NOARG_V(Long,jlong)
NOARG_V(Float,jfloat)
NOARG_V(Double,jdouble)

static void nonvirtual_void_v(JNIEnv *env,jobject object,jclass clazz,jmethodID method,...) {
    va_list args;va_start(args,method);(*env)->CallNonvirtualVoidMethodV(env,object,clazz,method,args);va_end(args);
}
#define NOARG_CHECK(NAME,EXPECTED,INDEX) do { \
    if ((*env)->CallNonvirtual##NAME##MethodA(env,object,clazz,methods[INDEX],NULL)!=(EXPECTED))return 10+INDEX; \
    if ((*env)->CallNonvirtual##NAME##Method(env,object,clazz,methods[INDEX])!=(EXPECTED))return 30+INDEX; \
    if (nonvirtual_##NAME##_v(env,object,clazz,methods[INDEX])!=(EXPECTED))return 50+INDEX; \
    if ((*env)->ExceptionCheck(env))return 70+INDEX; \
} while (0)
int rudroid_test_nonvirtual_calls(JNIEnv *env,jobject object,jclass clazz,const jmethodID *methods) {
    jobject a=(*env)->CallNonvirtualObjectMethodA(env,object,clazz,methods[0],NULL);
    jobject regular=(*env)->CallNonvirtualObjectMethod(env,object,clazz,methods[0]);
    jobject v=nonvirtual_Object_v(env,object,clazz,methods[0]);
    if (!(*env)->IsSameObject(env,a,object) || !(*env)->IsSameObject(env,regular,object) || !(*env)->IsSameObject(env,v,object))return 10;
    (*env)->DeleteLocalRef(env,a);(*env)->DeleteLocalRef(env,regular);(*env)->DeleteLocalRef(env,v);
    NOARG_CHECK(Boolean,1,1);
    NOARG_CHECK(Byte,-127,2);
    NOARG_CHECK(Char,65535,3);
    NOARG_CHECK(Short,-32767,4);
    NOARG_CHECK(Int,0x1234567,5);
    NOARG_CHECK(Long,INT64_C(0x1122334455667788),6);
    NOARG_CHECK(Float,1.25f,7);
    NOARG_CHECK(Double,1234567.125,8);
    (*env)->CallNonvirtualVoidMethodA(env,object,clazz,methods[9],NULL);
    (*env)->CallNonvirtualVoidMethod(env,object,clazz,methods[9]);
    nonvirtual_void_v(env,object,clazz,methods[9]);
    return (*env)->ExceptionCheck(env)?90:0;
}
