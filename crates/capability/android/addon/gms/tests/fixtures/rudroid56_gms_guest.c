// INPUT:  (none) — freestanding C, no libc dependency (elf_load rejects SHN_UNDEF imports)
// OUTPUT:  JNI_OnLoad, rudroid56_gms_report
// POS:    Ticket 56 GMS visibility probe: the guest side re-verifies the GMS registration surface through the shared JNI session
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// ticket 56 fixture (synthetic guest, not the real libmain): deployed as libmain.so, mapped by the
// loader through the android_elf_load exec backend, which calls JNI_OnLoad per the ticket-52 contract. The probe
// runs after the register-jni phase of sdk_google_gms / sdk_google_play_core and, from the guest
// side, walks GetEnv → FindClass(GMS class surface) → GetStaticMethodID → CallStatic*MethodA,
// verifying that the minimal GMS/Play Core registration is visible to any session holder via shared JNI queries (the 52
// session/registry path), rather than only checking the addon crate's static return values. Returns
// JNI_VERSION_1_6 when all steps pass; on failure returns (0x5600 | failing_step) to locate the broken link.
//
// JNI slots share their source with rudroid52_probe.c (spec + libjnivm jni.h oracle): GetEnv=6,
// GetVersion=4, FindClass=6, NewGlobalRef=21, IsSameObject=24,
// GetStaticMethodID=113, CallStaticBooleanMethodA=119, ExceptionCheck=228.

#define JNI_OK            0
#define JNI_VERSION_1_6  0x00010006
#define JNI_TRUE         1
#define JNI_FALSE        0

typedef int jint;
typedef unsigned char jboolean;
typedef void* jobject;
typedef void* jclass;
typedef void* jmethodID;

union jvalue {
    jboolean z;
    jint i;
};

struct Rudroid56GmsReport {
    unsigned magic;      /* 0xB05601 once JNI_OnLoad is entered */
    jint fail_step;      /* 0 when every step passed */
    void* env;
    jclass task;
    jclass listeners[3];
    jclass auth_result;
    void* global_task;
    jboolean is_successful;        /* oracle google_gms.cpp Task::isSuccessful */
    jboolean authenticated;        /* oracle AuthenticationResult (games) */
    unsigned flags;
};

static void report_set(volatile struct Rudroid56GmsReport* r, int step) {
    r->flags |= (1u << step);
}

static int streq(const char* a, const char* b) {
    unsigned i;
    for (i = 0; i < 80; i++) {
        if (a[i] != b[i]) return 0;
        if (a[i] == 0) return 1;
    }
    return 0;
}

volatile struct Rudroid56GmsReport rudroid56_gms_report = { 0, 0, 0, 0, {0, 0, 0}, 0, 0, 0, 0, 0 };

jint JNI_OnLoad(void* vm, void* reserved) {
    volatile struct Rudroid56GmsReport* r = &rudroid56_gms_report;
    void** vm_tab;
    void** env_tab;
    void* env = 0;
    jint rc;

    r->magic = 0xB05601u;
    (void)reserved;

    if (vm == 0) { r->fail_step = 1; return (0x5600 | 1); }
    vm_tab = *(void***)vm;
    {
        typedef jint (*GetEnv_t)(void*, void**, jint);
        GetEnv_t get_env = (GetEnv_t)vm_tab[6];
        if (get_env == 0) { r->fail_step = 1; return (0x5600 | 1); }
        rc = get_env(vm, &env, JNI_VERSION_1_6);
        r->env = env;
        if (rc != JNI_OK || env == 0) { r->fail_step = 1; return (0x5600 | 1); }
    }
    report_set(r, 1);

    env_tab = *(void***)env;
    {
        typedef void* (*FindClass_t)(void*, const char*);
        typedef void* (*NewGlobalRef_t)(void*, jobject);
        typedef jboolean (*IsSameObject_t)(void*, jobject, jobject);
        typedef void* (*GetStaticMethodID_t)(void*, jclass, const char*, const char*);
        typedef jboolean (*CallStaticBooleanA_t)(void*, jclass, jmethodID, union jvalue*);
        typedef jboolean (*ExceptionCheck_t)(void*);
        typedef void (*ExceptionClear_t)(void*);
        FindClass_t find_class = (FindClass_t)env_tab[6];
        NewGlobalRef_t new_global = (NewGlobalRef_t)env_tab[21];
        IsSameObject_t is_same = (IsSameObject_t)env_tab[24];
        GetStaticMethodID_t get_static = (GetStaticMethodID_t)env_tab[113];
        CallStaticBooleanA_t call_bool = (CallStaticBooleanA_t)env_tab[119];
        ExceptionCheck_t exception_check = (ExceptionCheck_t)env_tab[228];
        ExceptionClear_t exception_clear = (ExceptionClear_t)env_tab[17];
        jclass task = 0;
        jclass auth = 0;
        jmethodID is_successful = 0;
        jmethodID is_authenticated = 0;
        union jvalue arg;
        int i;

        if (find_class == 0 || new_global == 0 || is_same == 0 || get_static == 0 ||
            call_bool == 0 || exception_check == 0) {
            r->fail_step = 2;
            return (0x5600 | 2);
        }
        arg.z = 0;

        /* Step 3: the GMS Task class + the three owned task-listener
         * interfaces resolve through the SHARED session (the session maps
         * every library's handles onto one class table — the addon's
         * register-jni phase anchored the same names). */
        task = find_class(env, "com/google/android/gms/tasks/Task");
        r->task = task;
        if (task == 0) { r->fail_step = 3; return (0x5600 | 3); }
        {
            const char* names[3] = {
                "com/google/android/gms/tasks/OnCompleteListener",
                "com/google/android/gms/tasks/OnSuccessListener",
                "com/google/android/gms/tasks/OnFailureListener",
            };
            for (i = 0; i < 3; i++) {
                jclass c = find_class(env, names[i]);
                r->listeners[i] = c;
                if (c == 0) { r->fail_step = 3; return (0x5600 | 3); }
            }
        }
        report_set(r, 3);

        /* Step 4: the registered method surface answers through the shared
         * session: Task.isSuccessful ()Z == JNI_TRUE (oracle google_gms.cpp:
         * "bool tasks::Task::isSuccessful() { return true; }"). */
        is_successful = get_static(env, task, "isSuccessful", "()Z");
        if (is_successful == 0) {
            if (exception_check(env)) exception_clear(env);
            r->fail_step = 4;
            return (0x5600 | 4);
        }
        r->is_successful = call_bool(env, task, is_successful, &arg);
        if (r->is_successful != JNI_TRUE) { r->fail_step = 4; return (0x5600 | 4); }
        report_set(r, 4);

        /* Step 5: the games surface answers too: AuthenticationResult
         * .isAuthenticated ()Z == JNI_FALSE (oracle: verbose "-> false"). */
        auth = find_class(env, "com/google/android/gms/games/AuthenticationResult");
        r->auth_result = auth;
        if (auth == 0) { r->fail_step = 5; return (0x5600 | 5); }
        is_authenticated = get_static(env, auth, "isAuthenticated", "()Z");
        if (is_authenticated == 0) {
            if (exception_check(env)) exception_clear(env);
            r->fail_step = 5;
            return (0x5600 | 5);
        }
        r->authenticated = call_bool(env, auth, is_authenticated, &arg);
        if (r->authenticated != JNI_FALSE) { r->fail_step = 5; return (0x5600 | 5); }
        report_set(r, 5);

        /* Step 6: a global anchor on the Task class lives in the shared
         * session-wide reference table and stays resolvable. */
        r->global_task = new_global(env, task);
        if (r->global_task == 0) { r->fail_step = 6; return (0x5600 | 6); }
        if (!is_same(env, r->global_task, task)) { r->fail_step = 6; return (0x5600 | 6); }
        report_set(r, 6);
    }

    return JNI_VERSION_1_6;
}
