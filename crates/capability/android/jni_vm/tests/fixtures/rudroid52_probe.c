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

        /* Step 13: ThrowNew preserves the requested exception class. */
        jclass throwable_class = (*env)->FindClass(env, "java/lang/RuntimeException");
        if (throwable_class == 0 || (*env)->ThrowNew(env, throwable_class, "rudroid52 probe exception") != JNI_OK) {
            r->fail_step = 13;
            return (0x5200 | 13);
        }
        jthrowable exception = (*env)->ExceptionOccurred(env);
        if ((*env)->ExceptionCheck(env) != JNI_TRUE || exception == 0) {
            r->fail_step = 13;
            return (0x5200 | 13);
        }
        (*env)->ExceptionClear(env);
        jclass exception_class = (*env)->GetObjectClass(env, exception);
        if (exception_class == 0 || !(*env)->IsSameObject(env, exception_class, throwable_class)) {
            r->fail_step = 13;
            return (0x5200 | 13);
        }
        (*env)->DeleteLocalRef(env, exception_class);
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
        if ((*env)->ThrowNew(env, throwable_class, "rudroid52 blocked call") != JNI_OK) {
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
        (*env)->DeleteLocalRef(env, throwable_class);
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

    /* Step 22: the framework's bounded ActivityThread facade is the SAME
     * process identity the shared VM already advertises, verified from the
     * guest side across libraries (android_base registers it, this probe only
     * walks the shared JNI table). currentActivityThread is a process
     * singleton, its class is the requested one, and both Application
     * accessors alias Context.getApplicationContext — one rooted object, three
     * fresh caller-local views, no second Application copy. Only the three
     * advertised accessors exist: no other ActivityThread member is implied. */
    {
        jclass context_class=(*env)->FindClass(env,"android/content/Context");
        jclass thread_class=(*env)->FindClass(env,"android/app/ActivityThread");
        jclass application_class=(*env)->FindClass(env,"android/app/Application");
        jobject context=(*env)->AllocObject(env,context_class);
        jmethodID get_context, get_thread, get_current_app, get_app;
        jobject application, thread, thread_again, class_of, application_local;
        if(!context_class||!thread_class||!application_class||!context) {r->fail_step=22;return (0x5200|22);}
        get_context=(*env)->GetMethodID(env,context_class,"getApplicationContext","()Landroid/content/Context;");
        get_thread=(*env)->GetStaticMethodID(env,thread_class,"currentActivityThread","()Landroid/app/ActivityThread;");
        get_current_app=(*env)->GetStaticMethodID(env,thread_class,"currentApplication","()Landroid/app/Application;");
        get_app=(*env)->GetMethodID(env,thread_class,"getApplication","()Landroid/app/Application;");
        if(!get_context||!get_thread||!get_current_app||!get_app||(*env)->ExceptionCheck(env)) {r->fail_step=22;return (0x5200|22);}
        application=(*env)->CallObjectMethodA(env,context,get_context,0);
        thread=(*env)->CallStaticObjectMethodA(env,thread_class,get_thread,0);
        thread_again=(*env)->CallStaticObjectMethodA(env,thread_class,get_thread,0);
        if(!application||!thread||!thread_again||(*env)->ExceptionCheck(env)) {r->fail_step=22;return (0x5200|22);}
        if((*env)->IsSameObject(env,thread,thread_again)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        /* The instance's runtime class is the requested ActivityThread class. */
        class_of=(*env)->GetObjectClass(env,thread);
        if(!class_of||(*env)->IsSameObject(env,class_of,thread_class)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        (*env)->DeleteLocalRef(env,class_of);
        /* The Context accessor really yields an android/app/Application, and
         * both ActivityThread accessors hand back THAT object: one Application
         * for the process, no separate owner or copy. */
        class_of=(*env)->GetObjectClass(env,application);
        if(!class_of||(*env)->IsSameObject(env,class_of,application_class)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        (*env)->DeleteLocalRef(env,class_of);
        application_local=(*env)->CallStaticObjectMethodA(env,thread_class,get_current_app,0);
        if(!application_local||(*env)->IsSameObject(env,application_local,application)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        (*env)->DeleteLocalRef(env,application_local);
        application_local=(*env)->CallObjectMethodA(env,thread,get_app,0);
        if(!application_local||(*env)->IsSameObject(env,application_local,application)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        if((*env)->ExceptionCheck(env)) {r->fail_step=22;return (0x5200|22);}
        /* The advertised Application really is an android/app/Application. */
        class_of=(*env)->GetObjectClass(env,application_local);
        if(!class_of||(*env)->IsSameObject(env,class_of,application_class)!=JNI_TRUE) {r->fail_step=22;return (0x5200|22);}
        (*env)->DeleteLocalRef(env,class_of);
        (*env)->DeleteLocalRef(env,application_local);
        (*env)->DeleteLocalRef(env,thread_again);
        (*env)->DeleteLocalRef(env,thread);
        (*env)->DeleteLocalRef(env,application);
        (*env)->DeleteLocalRef(env,context);
        (*env)->DeleteLocalRef(env,context_class);
        (*env)->DeleteLocalRef(env,thread_class);
        (*env)->DeleteLocalRef(env,application_class);
        if((*env)->ExceptionCheck(env)) {r->fail_step=22;return (0x5200|22);}
        report_set(r,22);
    }

    /* Step 23: the framework's zero-argument android/content/Intent
     * constructor. Android's empty Intent() does nothing beyond ordinary
     * fresh-object initialization, so the shared VM's fresh field defaults
     * must already satisfy it: two NewObjectA views are distinct Intents with
     * null action/package/categories and zero flags, mutating ONE through the
     * already-registered setters leaves the other empty, and a third default
     * object built after that mutation is still empty (no cross-object reset
     * or shared launcher-Intent state). */
    {
        jclass intent_class=(*env)->FindClass(env,"android/content/Intent");
        jmethodID ctor, ctor_named, get_action, get_package, get_categories, get_flags, set_flags, set_package;
        jobject first, second, third, class_of, mutated, package_string, action, package, categories;
        jvalue argument;
        jint flags;
        if(!intent_class) {r->fail_step=23;return (0x5200|23);}
        ctor=(*env)->GetMethodID(env,intent_class,"<init>","()V");
        ctor_named=(*env)->GetMethodID(env,intent_class,"<init>","(Ljava/lang/String;)V");
        get_action=(*env)->GetMethodID(env,intent_class,"getAction","()Ljava/lang/String;");
        get_package=(*env)->GetMethodID(env,intent_class,"getPackage","()Ljava/lang/String;");
        get_categories=(*env)->GetMethodID(env,intent_class,"getCategories","()Ljava/util/Set;");
        get_flags=(*env)->GetMethodID(env,intent_class,"getFlags","()I");
        set_flags=(*env)->GetMethodID(env,intent_class,"setFlags","(I)Landroid/content/Intent;");
        set_package=(*env)->GetMethodID(env,intent_class,"setPackage","(Ljava/lang/String;)Landroid/content/Intent;");
        if(!ctor||!ctor_named||!get_action||!get_package||!get_categories||!get_flags||!set_flags||!set_package
           ||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        first=(*env)->NewObjectA(env,intent_class,ctor,0);
        second=(*env)->NewObjectA(env,intent_class,ctor,0);
        if(!first||!second||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        /* Each default object really is an android/content/Intent... */
        class_of=(*env)->GetObjectClass(env,first);
        if(!class_of||(*env)->IsSameObject(env,class_of,intent_class)!=JNI_TRUE) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,class_of);
        /* ...and the two are distinct objects, never a shared singleton. */
        if((*env)->IsSameObject(env,first,second)==JNI_TRUE) {r->fail_step=23;return (0x5200|23);}
        /* A fresh default Intent carries only field defaults: no action, no
         * package, no categories and zero flags, without a pending exception. */
        action=(*env)->CallObjectMethodA(env,first,get_action,0);
        package=(*env)->CallObjectMethodA(env,first,get_package,0);
        categories=(*env)->CallObjectMethodA(env,first,get_categories,0);
        flags=(*env)->CallIntMethodA(env,first,get_flags,0);
        if(action||package||categories||flags!=0||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,action);
        (*env)->DeleteLocalRef(env,package);
        (*env)->DeleteLocalRef(env,categories);
        /* Mutate ONLY the first object through the already-registered setters
         * (the named constructor is also proven live by the same lookup). */
        package_string=(*env)->NewStringUTF(env,"com.example.notused");
        argument.l=package_string;
        mutated=(*env)->CallObjectMethodA(env,first,set_package,&argument);
        if(!mutated||(*env)->IsSameObject(env,mutated,first)!=JNI_TRUE||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,mutated);
        argument.i=0x20;
        mutated=(*env)->CallObjectMethodA(env,first,set_flags,&argument);
        if(!mutated||(*env)->IsSameObject(env,mutated,first)!=JNI_TRUE||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,mutated);
        if((*env)->CallIntMethodA(env,first,get_flags,0)!=0x20||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        package=(*env)->CallObjectMethodA(env,first,get_package,0);
        if(!package||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,package);
        /* The other default Intent is untouched... */
        action=(*env)->CallObjectMethodA(env,second,get_action,0);
        package=(*env)->CallObjectMethodA(env,second,get_package,0);
        categories=(*env)->CallObjectMethodA(env,second,get_categories,0);
        flags=(*env)->CallIntMethodA(env,second,get_flags,0);
        if(action||package||categories||flags!=0||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,action);
        (*env)->DeleteLocalRef(env,package);
        (*env)->DeleteLocalRef(env,categories);
        /* ...and a default Intent built AFTER that mutation is still empty. */
        third=(*env)->NewObjectA(env,intent_class,ctor,0);
        if(!third||(*env)->IsSameObject(env,third,first)==JNI_TRUE||(*env)->IsSameObject(env,third,second)==JNI_TRUE
           ||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        action=(*env)->CallObjectMethodA(env,third,get_action,0);
        package=(*env)->CallObjectMethodA(env,third,get_package,0);
        categories=(*env)->CallObjectMethodA(env,third,get_categories,0);
        flags=(*env)->CallIntMethodA(env,third,get_flags,0);
        if(action||package||categories||flags!=0||(*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        (*env)->DeleteLocalRef(env,action);
        (*env)->DeleteLocalRef(env,package);
        (*env)->DeleteLocalRef(env,categories);
        (*env)->DeleteLocalRef(env,third);
        (*env)->DeleteLocalRef(env,second);
        (*env)->DeleteLocalRef(env,first);
        (*env)->DeleteLocalRef(env,package_string);
        (*env)->DeleteLocalRef(env,intent_class);
        if((*env)->ExceptionCheck(env)) {r->fail_step=23;return (0x5200|23);}
        report_set(r,23);
    }

    /* Step 24: standard Intent.setAction(String)Intent. It must be fluent
     * (returns the SAME object, not a copy), assign the same action field the
     * (String) constructor uses, compare equal to the supplied String, accept a
     * null argument as an action reset, and leave every other Intent alone -
     * including the distinct launcher Intent Activity.getIntent advertises. */
    {
        jclass intent_class=(*env)->FindClass(env,"android/content/Intent");
        jclass activity_class=(*env)->FindClass(env,"android/app/Activity");
        jmethodID ctor, set_action, get_action;
        jobject first, other, returned, launcher, activity, action_string, action_seen;
        const char* action_chars;
        jvalue argument;
        if(!intent_class||!activity_class) {r->fail_step=24;return (0x5200|24);}
        ctor=(*env)->GetMethodID(env,intent_class,"<init>","()V");
        set_action=(*env)->GetMethodID(env,intent_class,"setAction","(Ljava/lang/String;)Landroid/content/Intent;");
        get_action=(*env)->GetMethodID(env,intent_class,"getAction","()Ljava/lang/String;");
        if(!ctor||!set_action||!get_action||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        first=(*env)->NewObjectA(env,intent_class,ctor,0);
        other=(*env)->NewObjectA(env,intent_class,ctor,0);
        activity=(*env)->AllocObject(env,activity_class);
        if(!first||!other||!activity||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        action_string=(*env)->NewStringUTF(env,"android.intent.action.MAIN");
        if(!action_string) {r->fail_step=24;return (0x5200|24);}
        argument.l=action_string;
        returned=(*env)->CallObjectMethodA(env,first,set_action,&argument);
        /* Fluent identity: the receiver comes back, never a second owner. */
        if(!returned||(*env)->IsSameObject(env,returned,first)!=JNI_TRUE||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        (*env)->DeleteLocalRef(env,returned);
        /* The stored action compares equal to the supplied String. */
        action_seen=(*env)->CallObjectMethodA(env,first,get_action,0);
        if(!action_seen||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        action_chars=(*env)->GetStringUTFChars(env,action_seen,0);
        if(!action_chars||!streq(action_chars,"android.intent.action.MAIN")) {r->fail_step=24;return (0x5200|24);}
        (*env)->ReleaseStringUTFChars(env,action_seen,action_chars);
        (*env)->DeleteLocalRef(env,action_seen);
        /* A null argument resets the action on the SAME object. */
        argument.l=0;
        returned=(*env)->CallObjectMethodA(env,first,set_action,&argument);
        if(!returned||(*env)->IsSameObject(env,returned,first)!=JNI_TRUE||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        (*env)->DeleteLocalRef(env,returned);
        action_seen=(*env)->CallObjectMethodA(env,first,get_action,0);
        if(action_seen||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        (*env)->DeleteLocalRef(env,action_seen);
        /* Independence: the other default Intent never received an action... */
        action_seen=(*env)->CallObjectMethodA(env,other,get_action,0);
        if(action_seen||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        (*env)->DeleteLocalRef(env,action_seen);
        /* ...and the launcher Intent stays a distinct object with its own
         * configured action, untouched by this Intent's set/reset. */
        {
            jmethodID get_intent=(*env)->GetMethodID(env,activity_class,"getIntent","()Landroid/content/Intent;");
            if(!get_intent||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
            launcher=(*env)->CallObjectMethodA(env,activity,get_intent,0);
            if(!launcher||(*env)->IsSameObject(env,launcher,first)==JNI_TRUE
               ||(*env)->IsSameObject(env,launcher,other)==JNI_TRUE||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
            action_seen=(*env)->CallObjectMethodA(env,launcher,get_action,0);
            if(!action_seen||(*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
            action_chars=(*env)->GetStringUTFChars(env,action_seen,0);
            if(!action_chars||!streq(action_chars,"android.intent.action.MAIN")) {r->fail_step=24;return (0x5200|24);}
            (*env)->ReleaseStringUTFChars(env,action_seen,action_chars);
            (*env)->DeleteLocalRef(env,action_seen);
            (*env)->DeleteLocalRef(env,launcher);
        }
        (*env)->DeleteLocalRef(env,action_string);
        (*env)->DeleteLocalRef(env,activity);
        (*env)->DeleteLocalRef(env,other);
        (*env)->DeleteLocalRef(env,first);
        (*env)->DeleteLocalRef(env,intent_class);
        (*env)->DeleteLocalRef(env,activity_class);
        if((*env)->ExceptionCheck(env)) {r->fail_step=24;return (0x5200|24);}
        report_set(r,24);
    }
    /* Step 25: real string extras, separate Intent owners, and detached Bundle
     * snapshots. Null mappings must shadow launcher defaults without vanishing;
     * keys compare by UTF-16 content, including embedded NUL/surrogate units. */
    {
        jclass ic=(*env)->FindClass(env,"android/content/Intent");
        jclass bc=(*env)->FindClass(env,"android/os/Bundle");
        jclass ac=(*env)->FindClass(env,"android/app/Activity");
        jmethodID ctor=(*env)->GetMethodID(env,ic,"<init>","()V");
        jmethodID put=(*env)->GetMethodID(env,ic,"putExtra","(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;");
        jmethodID get=(*env)->GetMethodID(env,ic,"getStringExtra","(Ljava/lang/String;)Ljava/lang/String;");
        jmethodID all=(*env)->GetMethodID(env,ic,"getExtras","()Landroid/os/Bundle;");
        jmethodID bp=(*env)->GetMethodID(env,bc,"putString","(Ljava/lang/String;Ljava/lang/String;)V");
        jmethodID bg=(*env)->GetMethodID(env,bc,"getString","(Ljava/lang/String;)Ljava/lang/String;");
        jmethodID bd=(*env)->GetMethodID(env,bc,"getString","(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;");
        jmethodID has=(*env)->GetMethodID(env,bc,"containsKey","(Ljava/lang/String;)Z");
        jmethodID launch=(*env)->GetMethodID(env,ac,"getIntent","()Landroid/content/Intent;");
        jobject first,second,key,same_key,one,two,fallback,a,b,activity,launcher,la,lb,lc,config,configured;
        jobject unicode_key,unicode_again;
        jvalue args[2];
        jchar units[]={0x41,0,0xd800};
#define EXTRA_REQUIRE(condition) do { if(!(condition)||(*env)->ExceptionCheck(env)) {r->fail_step=25;return (0x5200|25);} } while(0)
#define EXTRA_REF(expression,expected) do { jobject result=(expression); EXTRA_REQUIRE((*env)->IsSameObject(env,result,(expected))==JNI_TRUE); (*env)->DeleteLocalRef(env,result); } while(0)
        EXTRA_REQUIRE(ic&&bc&&ac&&ctor&&put&&get&&all&&bp&&bg&&bd&&has&&launch);
        first=(*env)->NewObjectA(env,ic,ctor,0);second=(*env)->NewObjectA(env,ic,ctor,0);
        key=(*env)->NewStringUTF(env,"probe.extra");same_key=(*env)->NewStringUTF(env,"probe.extra");
        one=(*env)->NewStringUTF(env,"one");two=(*env)->NewStringUTF(env,"two");fallback=(*env)->NewStringUTF(env,"fallback");
        EXTRA_REQUIRE(first&&second&&key&&same_key&&one&&two&&fallback);
        EXTRA_REF((*env)->CallObjectMethodA(env,first,all,0),0);
        args[0].l=key;args[1].l=one;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,put,args),first);
        args[0].l=same_key;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,get,args),one);
        EXTRA_REF((*env)->CallObjectMethodA(env,second,get,args),0);
        args[1].l=two;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,put,args),first);
        a=(*env)->CallObjectMethodA(env,first,all,0);b=(*env)->CallObjectMethodA(env,first,all,0);
        EXTRA_REQUIRE(a&&b&&(*env)->IsSameObject(env,a,b)==JNI_FALSE);
        args[1].l=one;(*env)->CallVoidMethodA(env,a,bp,args);
        EXTRA_REF((*env)->CallObjectMethodA(env,first,get,args),two);
        EXTRA_REF((*env)->CallObjectMethodA(env,b,bg,args),two);
        EXTRA_REF((*env)->CallObjectMethodA(env,a,bg,args),one);
        args[0].l=0;args[1].l=one;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,put,args),first);
        EXTRA_REF((*env)->CallObjectMethodA(env,first,get,args),one);
        unicode_key=(*env)->NewString(env,units,3);unicode_again=(*env)->NewString(env,units,3);
        EXTRA_REQUIRE(unicode_key&&unicode_again);
        args[0].l=unicode_key;args[1].l=two;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,put,args),first);
        (*env)->DeleteLocalRef(env,unicode_key);
        args[0].l=unicode_again;
        EXTRA_REF((*env)->CallObjectMethodA(env,first,get,args),two);

        activity=(*env)->AllocObject(env,ac);launcher=(*env)->CallObjectMethodA(env,activity,launch,0);
        config=(*env)->NewStringUTF(env,"rudroid_probe_config");configured=(*env)->NewStringUTF(env,"configured");
        EXTRA_REQUIRE(activity&&launcher&&config&&configured);
        args[0].l=config;
        { jobject value=(*env)->CallObjectMethodA(env,launcher,get,args);const char* text;
          EXTRA_REQUIRE(value);text=(*env)->GetStringUTFChars(env,value,0);
          EXTRA_REQUIRE(text&&streq(text,"configured"));(*env)->ReleaseStringUTFChars(env,value,text);(*env)->DeleteLocalRef(env,value); }
        EXTRA_REF((*env)->CallObjectMethodA(env,second,get,args),0);
        la=(*env)->CallObjectMethodA(env,launcher,all,0);lb=(*env)->CallObjectMethodA(env,launcher,all,0);
        EXTRA_REQUIRE(la&&lb);
        args[1].l=0;(*env)->CallVoidMethodA(env,la,bp,args);
        EXTRA_REQUIRE((*env)->CallBooleanMethodA(env,la,has,args)==JNI_TRUE);
        args[1].l=fallback;
        EXTRA_REF((*env)->CallObjectMethodA(env,la,bd,args),fallback);
        { jobject value=(*env)->CallObjectMethodA(env,lb,bg,args);const char* text;
          EXTRA_REQUIRE(value);text=(*env)->GetStringUTFChars(env,value,0);
          EXTRA_REQUIRE(text&&streq(text,"configured"));(*env)->ReleaseStringUTFChars(env,value,text);(*env)->DeleteLocalRef(env,value); }
        /* Null override on the launcher must not fall back to configuration. */
        args[1].l=0;EXTRA_REF((*env)->CallObjectMethodA(env,launcher,put,args),launcher);
        EXTRA_REF((*env)->CallObjectMethodA(env,launcher,get,args),0);
        lc=(*env)->CallObjectMethodA(env,launcher,all,0);EXTRA_REQUIRE(lc);
        EXTRA_REQUIRE((*env)->CallBooleanMethodA(env,lc,has,args)==JNI_TRUE);
        args[1].l=fallback;EXTRA_REF((*env)->CallObjectMethodA(env,lc,bd,args),fallback);
        args[1].l=configured;EXTRA_REF((*env)->CallObjectMethodA(env,launcher,put,args),launcher);
        { jobject locals[]={first,second,key,same_key,one,two,fallback,a,b,activity,launcher,la,lb,lc,config,configured,unicode_again,ic,bc,ac};
          unsigned int i;for(i=0;i<sizeof(locals)/sizeof(locals[0]);++i)(*env)->DeleteLocalRef(env,locals[i]); }
        EXTRA_REQUIRE(1);report_set(r,25);
#undef EXTRA_REF
#undef EXTRA_REQUIRE
    }
    /* Step 26: Java Class.getSuperclass uses the same ordinary-class
     * authority as JNI, with independently known expected parents. */
    {
        jclass meta=(*env)->FindClass(env,"java/lang/Class");
        jclass app=(*env)->FindClass(env,"android/app/Application");
        jclass context=(*env)->FindClass(env,"android/content/Context");
        jclass object=(*env)->FindClass(env,"java/lang/Object");
        jclass array=(*env)->FindClass(env,"[Ljava/lang/String;");
        jmethodID parent=(*env)->GetMethodID(env,meta,"getSuperclass","()Ljava/lang/Class;");
        jobject from_java,from_jni;
#define SUPER_REQUIRE(x) do{if(!(x)||(*env)->ExceptionCheck(env)){r->fail_step=26;return (0x5200|26);}}while(0)
        SUPER_REQUIRE(meta&&app&&context&&object&&array&&parent);
        from_java=(*env)->CallObjectMethodA(env,app,parent,0);
        from_jni=(*env)->GetSuperclass(env,app);
        SUPER_REQUIRE((*env)->IsSameObject(env,from_java,context)&&(*env)->IsSameObject(env,from_jni,context));
        (*env)->DeleteLocalRef(env,from_java);(*env)->DeleteLocalRef(env,from_jni);
        from_java=(*env)->CallObjectMethodA(env,object,parent,0);
        SUPER_REQUIRE(from_java==0&&(*env)->GetSuperclass(env,object)==0);
        from_java=(*env)->CallObjectMethodA(env,array,parent,0);
        from_jni=(*env)->GetSuperclass(env,array);
        SUPER_REQUIRE((*env)->IsSameObject(env,from_java,object)&&(*env)->IsSameObject(env,from_jni,object));
        {jobject locals[]={from_java,from_jni,meta,app,context,object,array};unsigned int i;
         for(i=0;i<sizeof(locals)/sizeof(locals[0]);++i)(*env)->DeleteLocalRef(env,locals[i]);}
        SUPER_REQUIRE(1);report_set(r,26);
#undef SUPER_REQUIRE
    }
    /* Step 27: declared APK SDK levels come from package metadata and do
     * not replace the compatibility runtime's independent API level. */
    {
        jclass activity=(*env)->FindClass(env,"android/app/Activity");
        jclass info_class=(*env)->FindClass(env,"android/content/pm/ApplicationInfo");
        jclass version=(*env)->FindClass(env,"android/os/Build$VERSION");
        jobject context=(*env)->AllocObject(env,activity),info;
        jmethodID get=(*env)->GetMethodID(env,activity,"getApplicationInfo","()Landroid/content/pm/ApplicationInfo;");
        jfieldID min=(*env)->GetFieldID(env,info_class,"minSdkVersion","I");
        jfieldID target=(*env)->GetFieldID(env,info_class,"targetSdkVersion","I");
        jfieldID runtime=(*env)->GetStaticFieldID(env,version,"SDK_INT","I");
#define SDK_REQUIRE(x) do{if(!(x)||(*env)->ExceptionCheck(env)){r->fail_step=27;return (0x5200|27);}}while(0)
        SDK_REQUIRE(activity&&info_class&&version&&context&&get&&min&&target&&runtime);
        info=(*env)->CallObjectMethodA(env,context,get,0);SDK_REQUIRE(info);
        SDK_REQUIRE((*env)->GetIntField(env,info,min)==24);
        SDK_REQUIRE((*env)->GetIntField(env,info,target)==35);
        SDK_REQUIRE((*env)->GetStaticIntField(env,version,runtime)==26);
        {
            jmethodID get_manager=(*env)->GetMethodID(env,activity,"getPackageManager","()Landroid/content/pm/PackageManager;");
            jobject manager=(*env)->CallObjectMethodA(env,context,get_manager,0),package_info,from_package;
            jclass manager_class=(*env)->FindClass(env,"android/content/pm/PackageManager");
            jclass package_class=(*env)->FindClass(env,"android/content/pm/PackageInfo");
            jmethodID get_package=(*env)->GetMethodID(env,manager_class,"getPackageInfo","(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;");
            jfieldID application=(*env)->GetFieldID(env,package_class,"applicationInfo","Landroid/content/pm/ApplicationInfo;");
            jstring name=(*env)->NewStringUTF(env,"org.rudroid.fixture");jvalue args[2];
            SDK_REQUIRE(get_manager&&manager&&manager_class&&package_class&&get_package&&application&&name);
            args[0].l=name;args[1].i=0;package_info=(*env)->CallObjectMethodA(env,manager,get_package,args);SDK_REQUIRE(package_info);
            from_package=(*env)->GetObjectField(env,package_info,application);
            SDK_REQUIRE((*env)->IsSameObject(env,info,from_package));
            {jobject locals[]={from_package,package_info,name,manager,manager_class,package_class};unsigned int i;
             for(i=0;i<sizeof(locals)/sizeof(locals[0]);++i)(*env)->DeleteLocalRef(env,locals[i]);}
        }
        {jobject locals[]={info,context,activity,info_class,version};unsigned int i;
         for(i=0;i<sizeof(locals)/sizeof(locals[0]);++i)(*env)->DeleteLocalRef(env,locals[i]);}
        SDK_REQUIRE(1);report_set(r,27);
#undef SDK_REQUIRE
    }
    /* Step 28: filters keep independent ordered action sets, including
     * duplicates, UTF-16 strings, strong ownership and real failure cases. */
    {
        jclass filter=(*env)->FindClass(env,"android/content/IntentFilter");
        jmethodID ctor=(*env)->GetMethodID(env,filter,"<init>","()V");
        jmethodID named=(*env)->GetMethodID(env,filter,"<init>","(Ljava/lang/String;)V");
        jmethodID add=(*env)->GetMethodID(env,filter,"addAction","(Ljava/lang/String;)V");
        jmethodID count=(*env)->GetMethodID(env,filter,"countActions","()I");
        jmethodID get=(*env)->GetMethodID(env,filter,"getAction","(I)Ljava/lang/String;");
        jmethodID has=(*env)->GetMethodID(env,filter,"hasAction","(Ljava/lang/String;)Z");
        jmethodID match=(*env)->GetMethodID(env,filter,"matchAction","(Ljava/lang/String;)Z");
        jstring a=(*env)->NewStringUTF(env,"fixture.action.A"),same=(*env)->NewStringUTF(env,"fixture.action.A");
        jstring b=(*env)->NewStringUTF(env,"fixture.action.B"),unicode,again,value;
        jobject first,second; jvalue args[1];jchar units[]={0x78,0,0xd800};
#define FILTER_REQUIRE(x) do{if(!(x)||(*env)->ExceptionCheck(env)){r->fail_step=28;return (0x5200|28);}}while(0)
#define FILTER_ERROR(name) do{jthrowable ex=(*env)->ExceptionOccurred(env);jclass cls; \
        if(!ex){r->fail_step=28;return (0x5200|28);}(*env)->ExceptionClear(env);cls=(*env)->FindClass(env,name); \
        FILTER_REQUIRE(cls&&(*env)->IsInstanceOf(env,ex,cls));(*env)->DeleteLocalRef(env,ex);(*env)->DeleteLocalRef(env,cls);}while(0)
        FILTER_REQUIRE(filter&&ctor&&named&&add&&count&&get&&has&&match&&a&&same&&b);
        first=(*env)->NewObjectA(env,filter,ctor,0);args[0].l=a;second=(*env)->NewObjectA(env,filter,named,args);
        FILTER_REQUIRE(first&&second);
        FILTER_REQUIRE((*env)->CallIntMethodA(env,first,count,0)==0&&(*env)->CallIntMethodA(env,second,count,0)==1);
        FILTER_REQUIRE((*env)->CallBooleanMethodA(env,first,has,args)==JNI_FALSE);
        FILTER_REQUIRE((*env)->CallBooleanMethodA(env,second,match,args)==JNI_TRUE);
        (*env)->CallVoidMethodA(env,first,add,args);args[0].l=same;(*env)->CallVoidMethodA(env,first,add,args);
        FILTER_REQUIRE((*env)->CallIntMethodA(env,first,count,0)==1);
        args[0].l=b;(*env)->CallVoidMethodA(env,first,add,args);
        (*env)->DeleteLocalRef(env,a);(*env)->DeleteLocalRef(env,same);(*env)->DeleteLocalRef(env,b);
        args[0].i=0;value=(*env)->CallObjectMethodA(env,first,get,args);
        {const char* text=(*env)->GetStringUTFChars(env,value,0);FILTER_REQUIRE(text&&streq(text,"fixture.action.A"));
         (*env)->ReleaseStringUTFChars(env,value,text);(*env)->DeleteLocalRef(env,value);}
        args[0].i=1;value=(*env)->CallObjectMethodA(env,first,get,args);
        {const char* text=(*env)->GetStringUTFChars(env,value,0);FILTER_REQUIRE(text&&streq(text,"fixture.action.B"));
         (*env)->ReleaseStringUTFChars(env,value,text);(*env)->DeleteLocalRef(env,value);}
        unicode=(*env)->NewString(env,units,3);again=(*env)->NewString(env,units,3);FILTER_REQUIRE(unicode&&again);
        args[0].l=unicode;(*env)->CallVoidMethodA(env,first,add,args);(*env)->DeleteLocalRef(env,unicode);
        args[0].l=again;FILTER_REQUIRE((*env)->CallBooleanMethodA(env,first,has,args)==JNI_TRUE);
        FILTER_REQUIRE((*env)->CallIntMethodA(env,first,count,0)==3&&(*env)->CallIntMethodA(env,second,count,0)==1);
        args[0].i=2;value=(*env)->CallObjectMethodA(env,first,get,args);
        {const jchar* text=(*env)->GetStringChars(env,value,0);FILTER_REQUIRE(text&&(*env)->GetStringLength(env,value)==3&&text[0]==0x78&&text[1]==0&&text[2]==0xd800);
         (*env)->ReleaseStringChars(env,value,text);(*env)->DeleteLocalRef(env,value);}
        args[0].l=0;FILTER_REQUIRE((*env)->CallBooleanMethodA(env,first,has,args)==JNI_FALSE&&(*env)->CallBooleanMethodA(env,first,match,args)==JNI_FALSE);
        (*env)->CallVoidMethodA(env,first,add,args);FILTER_ERROR("java/lang/NullPointerException");
        FILTER_REQUIRE((*env)->CallIntMethodA(env,first,count,0)==3);
        args[0].i=-1;value=(*env)->CallObjectMethodA(env,first,get,args);
        if(value){r->fail_step=28;return (0x5200|28);}FILTER_ERROR("java/lang/IndexOutOfBoundsException");
        args[0].i=3;value=(*env)->CallObjectMethodA(env,first,get,args);
        if(value){r->fail_step=28;return (0x5200|28);}FILTER_ERROR("java/lang/IndexOutOfBoundsException");
        args[0].l=0;value=(*env)->NewObjectA(env,filter,named,args);
        if(value){r->fail_step=28;return (0x5200|28);}FILTER_ERROR("java/lang/NullPointerException");
        FILTER_REQUIRE((*env)->CallIntMethodA(env,second,count,0)==1);
        {jobject locals[]={first,second,again,filter};unsigned int i;for(i=0;i<sizeof(locals)/sizeof(locals[0]);++i)(*env)->DeleteLocalRef(env,locals[i]);}
        FILTER_REQUIRE(1);report_set(r,28);
#undef FILTER_ERROR
#undef FILTER_REQUIRE
    }
    r->fail_step = 0;
    return (jint)JNI_VERSION_1_6;
}
