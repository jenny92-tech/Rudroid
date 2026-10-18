// INPUT:  shared JNI function table — freestanding C without libc dependencies
// OUTPUT:  JNI_OnLoad, rudroid_spy_NativeLoader_load, rudroid_spy_initJni_ctx/_ctx_i,
//         rudroid_spy_native* lifecycle probes, rudroid_spy_signal_vsync, rudroid_spy_event_count/_at
// POS:    Ticket 53 hand-written guest spy fixture: a real ARM64 .so plays the libmain/libunity
//         native method bodies, mapped via ticket-49 exec and dispatched by the profile through shared JNI RegisterNatives / Call*MethodA,
//         and records every NativeLoader/JNI/bootstrap/frame-loop call with its arguments into an event buffer.
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Compiled by the Docker verification with `gcc -shared -fPIC -nostdlib`
// and passed to the profile-lifecycle integration tests via RUDROID53_SPY
// (plus the -DRUDROID_SPY_OMIT_RENDER / -DRUDROID_SPY_LOAD_FAILS failure
// variants). The loader maps this file as BOTH libmain.so and libil2cpp.so;
// JNI_OnLoad runs through the ticket-49 exec mapping (loader-driven), the
// lifecycle probes are dispatched by the unity profile capabilities through
// the shared JNI registered-method table on the same mapping.
//
// Every probe appends one RudroidSpyEvent to the module-local buffer; the
// profile reads the buffer back through rudroid_spy_event_count/_at after the
// N-frame run and echoes it into the host log, so the tests can assert the
// exact call sequence AND arguments (ticket 53 acceptance).
//
// JNI slot indices follow the fixed JNINativeInterface order (spec + the
// read-only libjnivm/include/jni.h oracle, same transcription as the
// rudroid52 probe): GetVersion=4, ExceptionCheck=228, NewStringUTF=167,
// GetStringUTFChars=169, ReleaseStringUTFChars=170. JNIInvokeInterface:
// GetEnv=6.

#define JNI_OK           0
#define JNI_ERR          (-1)
#define JNI_VERSION_1_6  0x00010006
#define JNI_TRUE         1
#define JNI_FALSE        0

typedef int jint;
typedef unsigned char jboolean;
typedef void* jobject;
typedef void* jclass;
typedef void* jstring;
/* JNI jvalue storage for the one reference argument used below. */
typedef union { jobject l; long long j; double d; } SpyValue;
_Static_assert(sizeof(SpyValue) == 8 && _Alignof(SpyValue) == 8, "ARM64 JNI value ABI");

/* Event kinds — mirrored by the profile capabilities' Rust echo code. */
#define SPY_JNI_ONLOAD       1
#define SPY_NATIVE_LOADER_LOAD 2
#define SPY_INIT_JNI_CTX     3
#define SPY_INIT_JNI_CTX_INT 4
#define SPY_RECREATE_GFX     5
#define SPY_RESTART_INDICATOR 6
#define SPY_SURFACE_CHANGED  7
#define SPY_FOCUS_CHANGED    8
#define SPY_RESUME           9
#define SPY_RENDER           10
#define SPY_SIGNAL_VSYNC     11
#define SPY_PAUSE            12
#define SPY_DONE             13

struct RudroidSpyEvent {
    int kind;
    long long a;
    long long b;
    char text[48];
};

#define SPY_EVENT_MAX 128

/* Module-local recording buffer (each mapped copy of this file owns one). */
static struct RudroidSpyEvent spy_events[SPY_EVENT_MAX];
static int spy_event_count = 0;
static int spy_render_frames = 0;
static int spy_vsync_signals = 0;

static void spy_record(int kind, long long a, long long b, const char* text) {
    struct RudroidSpyEvent* event;
    int i;
    if (spy_event_count >= SPY_EVENT_MAX) return;
    event = &spy_events[spy_event_count++];
    event->kind = kind;
    event->a = a;
    event->b = b;
    for (i = 0; i < 47 && text[i] != 0; ++i) event->text[i] = text[i];
    event->text[i] = 0;
}

/* Hand-rolled C-string copy (no libc; elf_load binds no undefined imports,
 * ticket 62). */
static void spy_copy(char* dst, const char* src, int cap) {
    int i;
    for (i = 0; i + 1 < cap && src[i] != 0; ++i) dst[i] = src[i];
    dst[i] = 0;
}

struct NativeMethod { const char *name, *signature; void *fn_ptr; };
jboolean rudroid_spy_NativeLoader_load(void *env,jclass clazz,jstring dir);
void rudroid_spy_initJni_ctx(void *, jobject, jobject);
void rudroid_spy_initJni_ctx_i(void *, jobject, jobject, jint);
void rudroid_spy_nativeRecreateGfxState(void *, jobject, jint, jobject);
void rudroid_spy_nativeRestartActivityIndicator(void *, jobject);
void rudroid_spy_nativeSendSurfaceChangedEvent(void *, jobject);
void rudroid_spy_nativeFocusChanged(void *, jobject, jboolean);
void rudroid_spy_nativeResume(void *, jobject);
jboolean rudroid_spy_nativePause(void *, jobject);
jboolean rudroid_spy_nativeDone(void *, jobject);
#ifndef RUDROID_SPY_OMIT_RENDER
jboolean rudroid_spy_nativeRender(void *, jobject);
#endif

#ifdef RUDROID_SPY_FLUSH_REFUSE
static jboolean refuse_preference_flush(void* env, jclass cls) {
    (void)env; (void)cls;
    return JNI_FALSE;
}
#endif

jint JNI_OnLoad(void* vm, void* reserved) {
    void** vm_tab;
    void* env = 0;
    typedef jint (*GetEnv_t)(void*, void**, jint);
    (void)reserved;
    if (vm == 0) {
        spy_record(SPY_JNI_ONLOAD, 0, 0, "no vm");
        return JNI_ERR;
    }
    vm_tab = *(void***)vm;
    if (((GetEnv_t)vm_tab[6])(vm, &env, JNI_VERSION_1_6) != JNI_OK || env == 0) {
        spy_record(SPY_JNI_ONLOAD, 0, 0, "GetEnv failed");
        return JNI_ERR;
    }
    void **env_tab=*(void ***)env;
    typedef jclass (*FindClass_t)(void *,const char *);
    typedef jint (*Register_t)(void *,jclass,const struct NativeMethod *,jint);
    typedef void (*Delete_t)(void *,jobject);
    jclass loader=((FindClass_t)env_tab[6])(env,"com/unity3d/player/NativeLoader");
    typedef void *(*GetMethod_t)(void *,jclass,const char *,const char *);
    typedef void (*Clear_t)(void *);
    if (!loader) return JNI_ERR;
    /* The fixture is mapped again as IL2CPP: only its first/main image binds load. */
    void *existing=((GetMethod_t)env_tab[113])(env,loader,"load","(Ljava/lang/String;)Z");
    if (!existing) {
        ((Clear_t)env_tab[17])(env);
        struct NativeMethod method={"load","(Ljava/lang/String;)Z",(void *)rudroid_spy_NativeLoader_load};
        if (((Register_t)env_tab[215])(env,loader,&method,1)!=JNI_OK) return JNI_ERR;
        static struct NativeMethod lifecycle[]={
            {"initJni", "(Landroid/content/Context;)V", (void *)rudroid_spy_initJni_ctx},
            {"nativeRecreateGfxState", "(ILandroid/view/Surface;)V", (void *)rudroid_spy_nativeRecreateGfxState},
            {"nativeRestartActivityIndicator", "()V", (void *)rudroid_spy_nativeRestartActivityIndicator},
            {"nativeSendSurfaceChangedEvent", "()V", (void *)rudroid_spy_nativeSendSurfaceChangedEvent},
            {"nativeFocusChanged", "(Z)V", (void *)rudroid_spy_nativeFocusChanged},
            {"nativeResume", "()V", (void *)rudroid_spy_nativeResume},
            {"nativePause", "()Z", (void *)rudroid_spy_nativePause},
            {"nativeDone", "()Z", (void *)rudroid_spy_nativeDone},
#ifndef RUDROID_SPY_OMIT_RENDER
            {"nativeRender", "()Z", (void *)rudroid_spy_nativeRender},
#endif
        };
        const char *classes[]={"com/unity3d/player/UnityPlayer", "com/unity3d/player/UnityPlayerForActivityOrService"};
        for (int i=0;i<2;++i) {
            jclass player=((FindClass_t)env_tab[6])(env,classes[i]);
            if (i==1) {
                lifecycle[0].signature="(Landroid/content/Context;I)V";
                lifecycle[0].fn_ptr=(void *)rudroid_spy_initJni_ctx_i;
            }
            if (((Register_t)env_tab[215])(env,player,lifecycle,sizeof(lifecycle)/sizeof(lifecycle[0]))!=JNI_OK) return JNI_ERR;
            ((Delete_t)env_tab[23])(env,player);
        }
    }
#ifdef RUDROID_SPY_FLUSH_REFUSE
    {
        jclass cls=((FindClass_t)env_tab[6])(env,"org/rudroid/runtime/Lifecycle");
        struct NativeMethod method={"flushPreferences", "()Z", (void *)refuse_preference_flush};
        if (((Register_t)env_tab[215])(env,cls,&method,1)!=JNI_OK) return JNI_ERR;
        ((Delete_t)env_tab[23])(env,cls);
    }
#endif
    ((Delete_t)env_tab[23])(env,loader);
    spy_record(SPY_JNI_ONLOAD, JNI_VERSION_1_6, 0, "getenv ok");
    return JNI_VERSION_1_6;
}

/* Native body behind the static `NativeLoader.load(String)Z` the C++
 * oracle's bootstrap invokes (registered by libmain there; dispatched
 * through RegisterNatives and the shared method table here). Consumes the SHARED SESSION's env
 * for the directory string — a real JNI round trip, not a mock. */
jboolean rudroid_spy_NativeLoader_load(void* env, jclass clazz, jstring dir) {
    void** env_tab;
    jboolean result = JNI_TRUE;
    typedef jstring (*NewStringUTF_t)(void*, const char*);
    typedef const char* (*GetStringUTFChars_t)(void*, jstring, jboolean*);
    typedef void (*ReleaseStringUTFChars_t)(void*, jstring, const char*);
    typedef jboolean (*ExceptionCheck_t)(void*);
    const char* utf;
    char text[48];
    (void)clazz;
    if (env == 0) return JNI_FALSE;
    env_tab = *(void***)env;
    if (dir == 0) {
        spy_record(SPY_NATIVE_LOADER_LOAD, 0, 0, "null dir");
        return JNI_FALSE;
    }
    {
        GetStringUTFChars_t get = (GetStringUTFChars_t)env_tab[169];
        ReleaseStringUTFChars_t release = (ReleaseStringUTFChars_t)env_tab[170];
        ExceptionCheck_t check = (ExceptionCheck_t)env_tab[228];
        utf = get(env, dir, 0);
        if (utf == 0 || check(env) != JNI_FALSE) {
            spy_record(SPY_NATIVE_LOADER_LOAD, 0, 0, "GetStringUTFChars failed");
            return JNI_FALSE;
        }
        spy_copy(text, utf, 48);
        release(env, dir, utf);
    }
#ifdef RUDROID_SPY_LOAD_FAILS
    result = JNI_FALSE;
#endif
    spy_record(SPY_NATIVE_LOADER_LOAD, result, 0, text);
    return result;
}

static int spy_valid_object(void *env,jobject object,const char *name) {
    if (!env || !object) return 0;
    void **table=*(void ***)env;
    jclass expected=((jclass (*)(void *,const char *))table[6])(env,name);
    if (!expected) return 0;
    /* IsInstanceOf accepts UnityPlayerActivity as an Activity/Context, while
       rejecting a null or unrelated object. Concrete class identity is not
       the native initJni parameter contract. */
    int valid=((jboolean (*)(void *,jobject,jclass))table[32])(env,object,expected);
    ((void (*)(void *,jobject))table[23])(env,expected);
    return valid;
}

/* `UnityPlayer.initJni` native bodies — one export per reference
 * signature. The profile resolves exactly the one its Spec names, so the
 * recorded kind is the per-version signature evidence. */
void rudroid_spy_initJni_ctx(void* env, jobject thiz, jobject ctx) {
    (void)env; (void)thiz; (void)ctx;
    spy_record(SPY_INIT_JNI_CTX, 0, spy_valid_object(env,thiz,"com/unity3d/player/UnityPlayer") && spy_valid_object(env,ctx,"android/app/Activity"), "(Landroid/content/Context;)V");
}

void rudroid_spy_initJni_ctx_i(void* env, jobject thiz, jobject ctx, jint type) {
    (void)env; (void)thiz; (void)ctx;
    void **table = *(void ***)env;
    typedef jclass (*Class_t)(void *, jobject);
    typedef void *(*Method_t)(void *, jclass, const char *, const char *);
    typedef jobject (*String_t)(void *, const char *);
    typedef jobject (*Call_t)(void *, jobject, void *, const SpyValue *);
    typedef jboolean (*Pending_t)(void *);
    typedef void (*Delete_t)(void *, jobject);
    jclass klass = ((Class_t)table[31])(env, thiz);
    void *query = ((Method_t)table[33])(env, klass, "getNetworkProxySettings", "(Ljava/lang/String;)Ljava/lang/String;");
    jobject url = ((String_t)table[167])(env, "https://example.invalid");
    SpyValue args[1] = {{0}}; args[0].l = url;
    jobject proxy = query && url ? ((Call_t)table[36])(env, thiz, query, args) : (jobject)0;
    int no_proxy = query && url && !proxy && !((Pending_t)table[228])(env);
    if (proxy) ((Delete_t)table[23])(env, proxy);
    if (url) ((Delete_t)table[23])(env, url);
    if (klass) ((Delete_t)table[23])(env, klass);
    spy_record(SPY_INIT_JNI_CTX_INT, type, no_proxy && spy_valid_object(env,thiz,"com/unity3d/player/UnityPlayerForActivityOrService") && spy_valid_object(env,ctx,"android/app/Activity"), "(Landroid/content/Context;I)V");
}

void rudroid_spy_nativeRecreateGfxState(void* env, jobject thiz, jint type,
                                     jobject surface) {
    (void)env; (void)thiz;
    spy_record(SPY_RECREATE_GFX, type, !spy_valid_object(env,surface,"android/view/Surface"), "");
}

void rudroid_spy_nativeRestartActivityIndicator(void* env, jobject thiz) {
    (void)env; (void)thiz;
    spy_record(SPY_RESTART_INDICATOR, 0, 0, "");
}

void rudroid_spy_nativeSendSurfaceChangedEvent(void* env, jobject thiz) {
    (void)env; (void)thiz;
    spy_record(SPY_SURFACE_CHANGED, 0, 0, "");
}

void rudroid_spy_nativeFocusChanged(void* env, jobject thiz, jboolean focused) {
    (void)env; (void)thiz;
    spy_record(SPY_FOCUS_CHANGED, focused, 0, "");
}

void rudroid_spy_nativeResume(void* env, jobject thiz) {
    (void)env; (void)thiz;
    spy_record(SPY_RESUME, 0, 0, "");
}

jboolean rudroid_spy_nativePause(void* env,jobject thiz) {(void)env;(void)thiz;spy_record(SPY_PAUSE,0,0,"");return JNI_FALSE;}
jboolean rudroid_spy_nativeDone(void* env,jobject thiz) {(void)env;(void)thiz;spy_record(SPY_DONE,0,0,"");return JNI_FALSE;}

/* Guest `nativeRender()Z`: returns true so the loop continues; the
 * OMIT_RENDER variant drops the export entirely (missing-required-function
 * path). */
#ifdef RUDROID_SPY_OMIT_RENDER
#else
jboolean rudroid_spy_nativeRender(void* env, jobject thiz) {
    (void)env; (void)thiz;
    ++spy_render_frames;
    spy_record(SPY_RENDER, spy_render_frames, 0, "");
#ifdef RUDROID_SPY_RENDER_FALSE_BEFORE
    if (spy_render_frames <= RUDROID_SPY_RENDER_FALSE_BEFORE) return JNI_FALSE;
#endif
#ifdef RUDROID_SPY_RENDER_STOP_AT
    if (spy_render_frames >= RUDROID_SPY_RENDER_STOP_AT) return JNI_FALSE;
#endif
    return JNI_TRUE;
}
#endif

/* Guest-side Choreographer stand-in: the profile signals vsync INTO the
 * guest before every rendered frame (reference
 * `Choreographer::signalAllVSync` position in the loop). */
void rudroid_spy_signal_vsync(void) {
    ++spy_vsync_signals;
    spy_record(SPY_SIGNAL_VSYNC, spy_vsync_signals, 0, "");
}

int rudroid_spy_event_count(void) {
    return spy_event_count;
}

int rudroid_spy_event_at(int index, struct RudroidSpyEvent* out) {
    if (index < 0 || index >= spy_event_count || out == 0) return 0;
    *out = spy_events[index];
    return 1;
}
