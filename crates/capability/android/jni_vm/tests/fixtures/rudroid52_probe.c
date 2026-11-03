// INPUT:  vendored AOSP jni.h; freestanding C, no libc dependency
// OUTPUT:  JNI_OnLoad, rudroid52_report
// POS:    Ticket 52 hand-written JNI probe fixture: the guest side walks the real JNI function table through
//         GetEnv→FindClass→method lookup/call→catalog known values, and re-verifies from the guest side
//         exception/type semantics, URI/orientation and real system CA export through full JNI dispatch.
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Compiled automatically by the Linux ARM64 jni-on-load integration test
// with `cc -shared -fPIC -nostdlib` and the real JNI header. The loader
// maps this probe through the android_elf_load exec backend, finds
// JNI_OnLoad and calls it with the shared session's JavaVM*. Every step
// records its outcome in rudroid52_report; JNI_OnLoad returns
// JNI_VERSION_1_6 (0x00010006) only when ALL steps passed, otherwise
// (0x5200 | failing_step), or TLS checkpoints 0x53xx/0x54xx. The
// harness sees which link broke and includes captured JNI diagnostics.
//
// JNI slot indices follow the fixed JNINativeInterface order (spec + the
// read-only libjnivm/include/jni.h oracle): GetVersion=4, FindClass=6,
// ThrowNew=14, ExceptionOccurred=15, ExceptionClear=17, NewGlobalRef=21,
// DeleteGlobalRef=22, DeleteLocalRef=23, IsSameObject=24, NewLocalRef=25,
// GetStaticMethodID=113, CallStaticBooleanMethodA=119,
// CallStaticIntMethodA=131, NewStringUTF=167, GetStringUTFLength=168,
// GetStringUTFChars=169, ReleaseStringUTFChars=170, GetJavaVM=219,
// ExceptionCheck=228. JNIInvokeInterface: GetEnv=6.

#include "jni.h"

struct Rudroid52Report {
    unsigned magic;      /* 0xB05252 once JNI_OnLoad is entered */
    jint fail_step;      /* 0 when every step passed */
    jint getenv_rc;
    jint version;
    void* env;
    void* clazz;
    void* method_int;
    jint volume;         /* catalog fixed reply for getStreamVolume */
    jboolean bt;         /* catalog fixed reply for isBluetoothA2dpOn */
    jboolean pending_seen;
    unsigned flags;      /* one bit per passed step */
};

static void report_set(volatile struct Rudroid52Report* r, int step) {
    r->flags |= (1u << step);
}

/* Hand-rolled C-string compare: no libc (elf_load binds no undefined
 * imports, ticket 62). */
static int streq(const char* a, const char* b) {
    unsigned i;
    for (i = 0; i < 64; i++) {
        if (a[i] != b[i]) return 0;
        if (a[i] == 0) return 1;
    }
    return 0;
}

volatile struct Rudroid52Report rudroid52_report = { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 };

jint JNI_OnLoad(JavaVM* vm, void* reserved) {
    volatile struct Rudroid52Report* r = &rudroid52_report;
    JNIEnv* env = 0;
    JavaVM* vm2 = 0;
    jclass clazz = 0;
    jclass global = 0;
    jmethodID method_int = 0;
    jmethodID method_bool = 0;
    jstring str = 0;
    jint rc;

    r->magic = 0xB05252u;
    (void)reserved;

    if (vm == 0) { r->fail_step = 1; return (0x5200 | 1); }
    /* Step 1: GetEnv on the session's JavaVM. */
    {
        rc = (*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6);
        r->getenv_rc = rc;
        r->env = env;
        if (rc != JNI_OK || env == 0) { r->fail_step = 1; return (0x5200 | 1); }
    }
    report_set(r, 1);

    /* Step 2: GetVersion reports JNI_VERSION_1_6. */
    {
        jint version = (*env)->GetVersion(env);
        r->version = version;
        if (version != (jint)JNI_VERSION_1_6) { r->fail_step = 2; return (0x5200 | 2); }
    }
    report_set(r, 2);

    {
        union jvalue args[1];

        /* Step 3: FindClass on the framework surface class. */
        clazz = (jclass)(*env)->FindClass(env, "android/media/AudioManager");
        r->clazz = clazz;
        if (clazz == 0) { r->fail_step = 3; return (0x5200 | 3); }
        report_set(r, 3);

        /* Step 4: string round trip (New/Get/Release/Length). */
        str = (jstring)(*env)->NewStringUTF(env, "rudroid52");
        if (str == 0) { r->fail_step = 4; return (0x5200 | 4); }
        {
            const char* utf = (*env)->GetStringUTFChars(env, str, 0);
            jint utf_len = (*env)->GetStringUTFLength(env, str);
            if (utf == 0 || !streq(utf, "rudroid52") || utf_len != 9) {
                r->fail_step = 4;
                return (0x5200 | 4);
            }
            (*env)->ReleaseStringUTFChars(env, str, utf);
        }
        report_set(r, 4);

        /* Step 5: GetStaticMethodID for the int-valued catalog method. */
        method_int = (jmethodID)(*env)->GetStaticMethodID(env, clazz, "getStreamVolume", "(I)I");
        r->method_int = method_int;
        if (method_int == 0) { r->fail_step = 5; return (0x5200 | 5); }
        report_set(r, 5);

        /* Step 6: CallStaticIntMethodA must return the catalog's known
         * fixed reply 100 — resolved through the host registry's catalog
         * provider (reply_catalog .so), proving the session and the
         * registry/catalog ecosystem interoperate across libraries. */
        args[0].i = 3;
        r->volume = (*env)->CallStaticIntMethodA(env, clazz, method_int, args);
        if (r->volume != 100) { r->fail_step = 6; return (0x5200 | 6); }
        report_set(r, 6);

        /* Step 7/8: the bool-valued catalog method resolves and answers
         * JNI_FALSE through the boolean call channel. */
        method_bool = (jmethodID)(*env)->GetStaticMethodID(env, clazz, "isBluetoothA2dpOn", "()Z");
        if (method_bool == 0) { r->fail_step = 7; return (0x5200 | 7); }
        report_set(r, 7);
        args[0].i = 0;
        r->bt = (*env)->CallStaticBooleanMethodA(env, clazz, method_bool, args);
        if (r->bt != JNI_FALSE || (*env)->ExceptionCheck(env) != JNI_FALSE) {
            r->fail_step = 8;
            return (0x5200 | 8);
        }
        report_set(r, 8);

        /* Step 9: TYPE MISMATCH — an int call on a bool-valued method must
         * fail clearly: zero result AND a pending exception, never a fake
         * success. */
        args[0].i = 0;
        r->pending_seen = 0;
        if ((*env)->CallStaticIntMethodA(env, clazz, method_bool, args) != 0) {
            r->fail_step = 9;
            return (0x5200 | 9);
        }
        if ((*env)->ExceptionCheck(env) != JNI_TRUE) { r->fail_step = 9; return (0x5200 | 9); }
        r->pending_seen = JNI_TRUE;
        report_set(r, 9);

        /* Step 10: pending exception cleared — calls recover. */
        (*env)->ExceptionClear(env);
        if ((*env)->ExceptionCheck(env) != JNI_FALSE || (*env)->ExceptionOccurred(env) != 0) {
            r->fail_step = 10;
            return (0x5200 | 10);
        }
        args[0].i = 0;
        if ((*env)->CallStaticBooleanMethodA(env, clazz, method_bool, args) != JNI_FALSE) {
            r->fail_step = 10;
            return (0x5200 | 10);
        }
        report_set(r, 10);

        /* Step 11: UNKNOWN METHOD — GetStaticMethodID returns NULL and
         * leaves an exception pending. */
        if ((*env)->GetStaticMethodID(env, clazz, "rudroid52_absent_method", "(I)I") != 0) {
            r->fail_step = 11;
            return (0x5200 | 11);
        }
        if ((*env)->ExceptionCheck(env) != JNI_TRUE) { r->fail_step = 11; return (0x5200 | 11); }
        (*env)->ExceptionClear(env);
        report_set(r, 11);

        /* Step 12: WRONG SIGNATURE — known method name, mismatching
         * descriptor must fail the same explicit way. */
        if ((*env)->GetStaticMethodID(env, clazz, "getStreamVolume", "(I)V") != 0) {
            r->fail_step = 12;
            return (0x5200 | 12);
        }
        if ((*env)->ExceptionCheck(env) != JNI_TRUE) { r->fail_step = 12; return (0x5200 | 12); }
        (*env)->ExceptionClear(env);
        report_set(r, 12);

        /* Step 13: ThrowNew raises a pending exception the guest can
         * observe. */
        if ((*env)->ThrowNew(env, clazz, "rudroid52 probe exception") != JNI_OK) {
            r->fail_step = 13;
            return (0x5200 | 13);
        }
        jthrowable exception = (*env)->ExceptionOccurred(env);
        if ((*env)->ExceptionCheck(env) != JNI_TRUE || exception == 0) {
            r->fail_step = 13;
            return (0x5200 | 13);
        }
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, exception);
        if ((*env)->ExceptionCheck(env)) { r->fail_step = 13; return (0x5200 | 13); }
        report_set(r, 13);

        /* Step 14: reference lifecycle — a global ref survives deleting
         * the local it came from and stays usable for method lookup;
         * IsSameObject aliases through local/global views; deletes clear. */
        global = (jclass)(*env)->NewGlobalRef(env, clazz);
        if (global == 0) { r->fail_step = 14; return (0x5200 | 14); }
        {
            jobject local_alias = (*env)->NewLocalRef(env, global);
            jmethodID via_global;
            if (local_alias == 0 || (*env)->IsSameObject(env, local_alias, global) != JNI_TRUE) {
                r->fail_step = 14;
                return (0x5200 | 14);
            }
            (*env)->DeleteLocalRef(env, local_alias);
            /* Delete the FindClass local, then keep using the class ONLY
             * through the global ref — the shared local/global reference
             * semantics across scopes. */
            (*env)->DeleteLocalRef(env, clazz);
            via_global = (*env)->GetStaticMethodID(env, (jclass)global, "getStreamVolume", "(I)I");
            if (via_global == 0) { r->fail_step = 14; return (0x5200 | 14); }
            /* A fresh local view of the global for the remaining steps. */
            clazz = (jclass)(*env)->NewLocalRef(env, global);
            if (clazz == 0) { r->fail_step = 14; return (0x5200 | 14); }
            /* Deleting the global ref must not disturb the live local. */
            (*env)->DeleteGlobalRef(env, global);
            global = 0;
            if ((*env)->IsSameObject(env, clazz, clazz) != JNI_TRUE) {
                r->fail_step = 14;
                return (0x5200 | 14);
            }
        }
        report_set(r, 14);

        /* Step 15: GetJavaVM hands back the SAME session VM. */
        if ((*env)->GetJavaVM(env, &vm2) != JNI_OK || vm2 != vm) {
            r->fail_step = 15;
            return (0x5200 | 15);
        }
        report_set(r, 15);

        /* Step 16: a pending exception BLOCKS calls (no execution, zero
         * result) until cleared — explicit, never a silent default. */
        if ((*env)->ThrowNew(env, clazz, "rudroid52 blocked call") != JNI_OK) {
            r->fail_step = 16;
            return (0x5200 | 16);
        }
        args[0].i = 3;
        if ((*env)->CallStaticIntMethodA(env, clazz, method_int, args) != 0) {
            r->fail_step = 16;
            return (0x5200 | 16);
        }
        if ((*env)->ExceptionCheck(env) != JNI_TRUE) { r->fail_step = 16; return (0x5200 | 16); }
        (*env)->ExceptionClear(env);
        args[0].i = 3;
        if ((*env)->CallStaticIntMethodA(env, clazz, method_int, args) != 100) {
            r->fail_step = 16;
            return (0x5200 | 16);
        }
        report_set(r, 16);
    }

    /* Step 17: real Android Uri.decode lookup and UTF-16 result, including
     * percent UTF-8, literal plus, embedded NUL, null and malformed input. */
    {
        jclass uri = (*env)->FindClass(env, "android/net/Uri");
        jmethodID decode;
        const char* inputs[] = { "%E4%B8%AD+%2B%00", "", "abc%", "%80" };
        const jchar expected[][5] = { {0x4e2d, '+', '+', 0}, {0}, {'a','b','c',0xfffd}, {0xfffd} };
        const jsize lengths[] = { 4, 0, 4, 1 };
        unsigned i;
        if (!uri) { r->fail_step = 17; return (0x5200 | 17); }
        decode = (*env)->GetStaticMethodID(env, uri, "decode", "(Ljava/lang/String;)Ljava/lang/String;");
        if (!decode) { r->fail_step = 17; return (0x5200 | 17); }
        for (i = 0; i < 4; i++) {
            jvalue argument;
            jstring result;
            const jchar* chars;
            jsize j;
            argument.l = (*env)->NewStringUTF(env, inputs[i]);
            result = (*env)->CallStaticObjectMethodA(env, uri, decode, &argument);
            if (!result || (*env)->ExceptionCheck(env) || (*env)->GetStringLength(env, result) != lengths[i]) {
                r->fail_step = 17; return (0x5200 | 17);
            }
            chars = (*env)->GetStringChars(env, result, 0);
            if (!chars) { r->fail_step = 17; return (0x5200 | 17); }
            for (j = 0; j < lengths[i]; j++) {
                if (chars[j] != expected[i][j]) { r->fail_step = 17; return (0x5200 | 17); }
            }
            (*env)->ReleaseStringChars(env, result, chars);
            (*env)->DeleteLocalRef(env, result);
            (*env)->DeleteLocalRef(env, argument.l);
        }
        {
            jvalue argument;
            argument.l = 0;
            if ((*env)->CallStaticObjectMethodA(env, uri, decode, &argument) || (*env)->ExceptionCheck(env)) {
                r->fail_step = 17; return (0x5200 | 17);
            }
        }
        (*env)->DeleteLocalRef(env, uri);
        report_set(r, 17);
    }

    /* Step 18: fixed-device Activity orientation, matching the C runtime.
     * setRequestedOrientation cannot rotate the host-owned SDL display. */
    {
        jclass activity = (*env)->FindClass(env, "android/app/Activity");
        jmethodID get, set;
        jobject object;
        jvalue orientation;
        if (!activity) { r->fail_step = 18; return (0x5200 | 18); }
        get = (*env)->GetMethodID(env, activity, "getRequestedOrientation", "()I");
        set = (*env)->GetMethodID(env, activity, "setRequestedOrientation", "(I)V");
        object = (*env)->AllocObject(env, activity);
        if (!get || !set || !object || (*env)->ExceptionCheck(env)) { r->fail_step = 18; return (0x5200 | 18); }
        if ((*env)->CallIntMethodA(env, object, get, 0) != 0) { r->fail_step = 18; return (0x5200 | 18); }
        orientation.i = 10;
        (*env)->CallVoidMethodA(env, object, set, &orientation);
        if ((*env)->ExceptionCheck(env) || (*env)->CallIntMethodA(env, object, get, 0) != 0) { r->fail_step = 18; return (0x5200 | 18); }
        (*env)->DeleteLocalRef(env, object);
        (*env)->DeleteLocalRef(env, activity);
        report_set(r, 18);
    }

    /* Step 19: Unity's complete trust-material query, with real system roots.
     * Java peer checks fail closed; exported byte arrays are independent. */
    {
        jclass factory_class = (*env)->FindClass(env, "javax/net/ssl/TrustManagerFactory");
        jclass manager_class = (*env)->FindClass(env, "javax/net/ssl/X509TrustManager");
        jclass cert_class = (*env)->FindClass(env, "java/security/cert/Certificate");
        jmethodID default_algorithm, get_instance, init, get_managers, get_issuers, get_encoded, check;
        jobject factory, manager, cert;
        jstring algorithm;
        jobjectArray managers, issuers;
        jbyteArray bytes, bytes_again;
        jvalue argument, check_args[2];
        jbyte prefix[4], mutated = 0;
        if (!factory_class || !manager_class || !cert_class) { r->fail_step = 191; return (0x5300 | 1); }
        default_algorithm = (*env)->GetStaticMethodID(env, factory_class, "getDefaultAlgorithm", "()Ljava/lang/String;");
        get_instance = (*env)->GetStaticMethodID(env, factory_class, "getInstance", "(Ljava/lang/String;)Ljavax/net/ssl/TrustManagerFactory;");
        init = (*env)->GetMethodID(env, factory_class, "init", "(Ljava/security/KeyStore;)V");
        get_managers = (*env)->GetMethodID(env, factory_class, "getTrustManagers", "()[Ljavax/net/ssl/TrustManager;");
        get_issuers = (*env)->GetMethodID(env, manager_class, "getAcceptedIssuers", "()[Ljava/security/cert/X509Certificate;");
        get_encoded = (*env)->GetMethodID(env, cert_class, "getEncoded", "()[B");
        check = (*env)->GetMethodID(env, manager_class, "checkServerTrusted", "([Ljava/security/cert/X509Certificate;Ljava/lang/String;)V");
        if (!default_algorithm || !get_instance || !init || !get_managers || !get_issuers || !get_encoded || !check) { r->fail_step = 192; return (0x5300 | 2); }
        argument.l = (*env)->NewStringUTF(env, "unavailable-algorithm");
        factory = (*env)->CallStaticObjectMethodA(env, factory_class, get_instance, &argument);
        if (factory || !(*env)->ExceptionCheck(env)) { r->fail_step = 193; return (0x5300 | 3); }
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, argument.l);
        algorithm = (*env)->CallStaticObjectMethodA(env, factory_class, default_algorithm, 0);
        argument.l = algorithm;
        factory = (*env)->CallStaticObjectMethodA(env, factory_class, get_instance, &argument);
        if (!factory || (*env)->ExceptionCheck(env)) { r->fail_step = 194; return (0x5300 | 4); }
        managers = (*env)->CallObjectMethodA(env, factory, get_managers, 0);
        if (managers || !(*env)->ExceptionCheck(env)) { r->fail_step = 195; return (0x5300 | 5); }
        (*env)->ExceptionClear(env);
        argument.l = 0;
        (*env)->CallVoidMethodA(env, factory, init, &argument);
        if ((*env)->ExceptionCheck(env)) { r->fail_step = 196; return (0x5300 | 6); }
        managers = (*env)->CallObjectMethodA(env, factory, get_managers, 0);
        if (!managers || (*env)->GetArrayLength(env, managers) != 1) { r->fail_step = 197; return (0x5300 | 7); }
        manager = (*env)->GetObjectArrayElement(env, managers, 0);
        if (!manager || !(*env)->IsInstanceOf(env, manager, manager_class)) { r->fail_step = 198; return (0x5300 | 8); }
        issuers = (*env)->CallObjectMethodA(env, manager, get_issuers, 0);
        if (!issuers || (*env)->GetArrayLength(env, issuers) < 1) { r->fail_step = 199; return (0x5300 | 9); }
        cert = (*env)->GetObjectArrayElement(env, issuers, 0);
        bytes = (*env)->CallObjectMethodA(env, cert, get_encoded, 0);
        if (!bytes) { r->fail_step = 200; return (0x5300 | 10); }
        if ((*env)->GetArrayLength(env, bytes) < 128) { r->fail_step = 200; return (0x5400 | ((*env)->GetArrayLength(env, bytes) & 255)); }
        (*env)->GetByteArrayRegion(env, bytes, 0, 4, prefix);
        if ((*env)->ExceptionCheck(env) || prefix[0] != 0x30) { r->fail_step = 201; return (0x5300 | 11); }
        (*env)->SetByteArrayRegion(env, bytes, 0, 1, &mutated);
        bytes_again = (*env)->CallObjectMethodA(env, cert, get_encoded, 0);
        if (!bytes_again) { r->fail_step = 202; return (0x5300 | 12); }
        (*env)->GetByteArrayRegion(env, bytes_again, 0, 4, prefix);
        if ((*env)->ExceptionCheck(env) || prefix[0] != 0x30) { r->fail_step = 203; return (0x5300 | 13); }
        check_args[0].l = issuers;
        check_args[1].l = (*env)->NewStringUTF(env, "RSA");
        (*env)->CallVoidMethodA(env, manager, check, check_args);
        if (!(*env)->ExceptionCheck(env)) { r->fail_step = 204; return (0x5300 | 14); }
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, check_args[1].l);
        argument.l = factory;
        (*env)->CallVoidMethodA(env, factory, init, &argument);
        if (!(*env)->ExceptionCheck(env)) { r->fail_step = 205; return (0x5300 | 15); }
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, bytes_again);
        (*env)->DeleteLocalRef(env, bytes);
        (*env)->DeleteLocalRef(env, cert);
        (*env)->DeleteLocalRef(env, issuers);
        (*env)->DeleteLocalRef(env, manager);
        (*env)->DeleteLocalRef(env, managers);
        (*env)->DeleteLocalRef(env, factory);
        (*env)->DeleteLocalRef(env, algorithm);
        (*env)->DeleteLocalRef(env, cert_class);
        (*env)->DeleteLocalRef(env, manager_class);
        (*env)->DeleteLocalRef(env, factory_class);
        report_set(r, 19);
    }

    /* Step 20: a physical gamepad is neither virtual nor alphabetic.
     * The synthetic keyboard/touch device remains virtual and alphabetic. */
    {
        jclass input_class = (*env)->FindClass(env, "android/view/InputDevice");
        jobject device;
        jfieldID id, sources;
        jmethodID is_virtual, keyboard;
        if (!input_class) { r->fail_step = 20; return (0x5200 | 20); }
        id = (*env)->GetFieldID(env, input_class, "id", "I");
        sources = (*env)->GetFieldID(env, input_class, "sources", "I");
        is_virtual = (*env)->GetMethodID(env, input_class, "isVirtual", "()Z");
        keyboard = (*env)->GetMethodID(env, input_class, "getKeyboardType", "()I");
        device = (*env)->AllocObject(env, input_class);
        if (!device || !id || !sources || !is_virtual || !keyboard) { r->fail_step = 20; return (0x5200 | 20); }
        (*env)->SetIntField(env, device, id, 1);
        (*env)->SetIntField(env, device, sources, 0x401);
        if ((*env)->CallBooleanMethodA(env, device, is_virtual, 0) || (*env)->CallIntMethodA(env, device, keyboard, 0) != 0 || (*env)->ExceptionCheck(env)) { r->fail_step = 20; return (0x5200 | 20); }
        (*env)->SetIntField(env, device, id, 0);
        (*env)->SetIntField(env, device, sources, 0x1103);
        if (!(*env)->CallBooleanMethodA(env, device, is_virtual, 0) || (*env)->CallIntMethodA(env, device, keyboard, 0) != 2 || (*env)->ExceptionCheck(env)) { r->fail_step = 20; return (0x5200 | 20); }
        (*env)->DeleteLocalRef(env, device);
        (*env)->DeleteLocalRef(env, input_class);
        report_set(r, 20);
    }

    /* Step 21: mixed object/float arguments and float return use the real
     * ARM JNI ABI, including va_arg's promoted double input. */
    {
        jclass context_class=(*env)->FindClass(env,"android/content/Context");
        jclass prefs_class=(*env)->FindClass(env,"android/content/SharedPreferences");
        jobject context=(*env)->AllocObject(env,context_class);
        jmethodID open=(*env)->GetMethodID(env,context_class,"getSharedPreferences","(Ljava/lang/String;I)Landroid/content/SharedPreferences;");
        jmethodID get=(*env)->GetMethodID(env,prefs_class,"getFloat","(Ljava/lang/String;F)F");
        jstring name=(*env)->NewStringUTF(env,"rudroid52-float-read");
        jstring key=(*env)->NewStringUTF(env,"missing-float");
        jvalue args[2]; jobject prefs; jfloat value;
        if(!context||!open||!get) {r->fail_step=21;return (0x5200|21);}
        args[0].l=name;args[1].i=0;
        prefs=(*env)->CallObjectMethodA(env,context,open,args);
        args[0].l=key;args[1].f=1.25f;
        value=(*env)->CallFloatMethodA(env,prefs,get,args);
        if(!prefs||value!=1.25f||(*env)->ExceptionCheck(env)){r->fail_step=21;return (0x5200|21);}
        value=(*env)->CallFloatMethod(env,prefs,get,key,(double)-0.75);
        if(value!=-0.75f||(*env)->ExceptionCheck(env)){r->fail_step=21;return (0x5200|21);}
        (*env)->DeleteLocalRef(env,prefs);(*env)->DeleteLocalRef(env,context);
        (*env)->DeleteLocalRef(env,name);(*env)->DeleteLocalRef(env,key);
        (*env)->DeleteLocalRef(env,context_class);(*env)->DeleteLocalRef(env,prefs_class);
        report_set(r,21);
    }
    r->fail_step = 0;
    return (jint)JNI_VERSION_1_6;
}
