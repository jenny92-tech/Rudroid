// INPUT:  jni.h type layout (ticket 52 session fixed slot table), ticket-56 loader integration tests
// OUTPUT:  JNI_OnLoad, rudroid56_xlua_probe_jni_calls, rudroid56_xlua_probe_reset
// POS:    Synthetic libxlua.so stand-in that records whether guest code was really mapped and then executed
//
// ticket 56 fixture (synthetic guest, not a real Unity/Burst library): after the loader maps this file
// through the ELF provider, it calls JNI_OnLoad per the ticket-52 contract; this probe counts statically in the module, and integration
// tests read the execution evidence via rudroid56_xlua_probe_jni_calls. JNI types are used with a fixed layout
// (JavaVM* as arg 1, reserved slot as arg 2); the return value follows ticket-52 version-negotiation semantics.
//
// Variant: -DRUDROID56_PROBE_NO_JNI_ONLOAD drops the JNI_OnLoad export (missing-symbol path).

#include <stdint.h>

static volatile int32_t g_jni_calls = 0;
static volatile int32_t g_last_jni_result = 0;

int32_t rudroid56_xlua_probe_jni_calls(void) { return g_jni_calls; }
int32_t rudroid56_xlua_probe_last_result(void) { return g_last_jni_result; }
void rudroid56_xlua_probe_reset(void) {
    g_jni_calls = 0;
    g_last_jni_result = 0;
}

#ifndef RUDROID56_PROBE_NO_JNI_ONLOAD
__attribute__((visibility("default")))
int32_t JNI_OnLoad(void* vm, void* reserved) {
    (void)vm;
    (void)reserved;
    g_jni_calls += 1;
    /* JNI_VERSION_1_6 — the ticket-52 session negotiates this version. */
    g_last_jni_result = 0x00010006;
    return g_last_jni_result;
}
#endif
