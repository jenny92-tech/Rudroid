// INPUT:  ticket-52 session JNI layout, symbol surface of the sdk_fmod.cpp read-only oracle
// OUTPUT:  JNI_OnLoad, FMOD5_System_SetOutput, rudroid56_fmod_probe_* evidence exports
// POS:    Synthetic libfmod.so stand-in that verifies the FMOD addon's org/fmod registration and SetOutput hook
//
// ticket 56 fixture (synthetic guest, not the real FMOD library): models the libfmod symbol
// surface consumed by the read-only oracle `projects/unityloader/plugins/sdk_fmod`:
//   - JNI_OnLoad: called by the loader through the shared JavaVM per the ticket-52 contract, returns JNI_VERSION_1_6;
//   - FMOD5_System_SetOutput: the oracle's preferred symbol (the try-order original),
//     records (output_type, result) per candidate, following the behavior observed by the oracle's set_output_hook.
//
// original behavior model (matching oracle semantics): AUDIOTRACK(15)/NOSOUND(2)/OPENSL(16)/
// AUTODETECT(0) are outputs this probe's "driver" supports and return 0 (success); other types return
// the FMOD_ERR_PLUGIN_MISSING-style 18. Injectable variants (gcc -D, rudroid53 spy convention):
//   -DRUDROID56_FMOD_NO_SETOUTPUT     drop the SetOutput export (missing-symbol path)
//   -DRUDROID56_FMOD_AUDIOTRACK_FAILS preferred AUDIOTRACK fails, forcing try-order to continue
//   -DRUDROID56_FMOD_ALL_FAIL         all candidates fail (oracle forces the NOSOUND fallback)
//   -DRUDROID56_FMOD_NO_JNI_ONLOAD    drop JNI_OnLoad (module without a VM entry)
// Evidence exports read by the integration tests: call sequence, latest result, JNI call count.

#include <stdint.h>

enum {
    OUT_AUTODETECT = 0,
    OUT_NOSOUND = 2,
    OUT_AUDIOTRACK = 15,
    OUT_OPENSL = 16,
};

#define RUDROID56_MAX_TRIES 16

static volatile int32_t g_jni_calls = 0;
static volatile int32_t g_jni_result = 0;
/* Record of original SetOutput calls: interleaved (type, result) pairs. */
static volatile int32_t g_try_types[RUDROID56_MAX_TRIES];
static volatile int32_t g_try_results[RUDROID56_MAX_TRIES];
static volatile int32_t g_try_count = 0;
/* Currently "selected" output inside the fake FMOD system. */
static volatile int32_t g_selected_output = -1;

int32_t rudroid56_fmod_probe_jni_calls(void) { return g_jni_calls; }
int32_t rudroid56_fmod_probe_jni_result(void) { return g_jni_result; }
int32_t rudroid56_fmod_probe_try_count(void) { return g_try_count; }
int32_t rudroid56_fmod_probe_try_type(int32_t index) {
    if (index < 0 || index >= RUDROID56_MAX_TRIES) return -1;
    return g_try_types[index];
}
int32_t rudroid56_fmod_probe_try_result(int32_t index) {
    if (index < 0 || index >= RUDROID56_MAX_TRIES) return -1;
    return g_try_results[index];
}
int32_t rudroid56_fmod_probe_selected_output(void) { return g_selected_output; }

static int original_set_output(void* system, int output_type) {
    if (g_try_count < RUDROID56_MAX_TRIES) {
        g_try_types[g_try_count] = output_type;
    }
    g_try_count += 1;
    int result;
#if defined(RUDROID56_FMOD_ALL_FAIL)
    (void)output_type;
    result = 18;
#elif defined(RUDROID56_FMOD_AUDIOTRACK_FAILS)
    if (output_type == OUT_AUDIOTRACK) {
        result = 18;
    } else {
        result = 0;
        g_selected_output = output_type;
    }
#else
    switch (output_type) {
        case OUT_AUTODETECT:
        case OUT_NOSOUND:
        case OUT_AUDIOTRACK:
        case OUT_OPENSL:
            result = 0;
            g_selected_output = output_type;
            break;
        default:
            result = 18;
            break;
    }
#endif
    if (g_try_count <= RUDROID56_MAX_TRIES) {
        g_try_results[g_try_count - 1] = result;
    }
    return result;
}

#ifndef RUDROID56_FMOD_NO_SETOUTPUT
/* The oracle's preferred symbol: FMOD5_System_SetOutput(void* system, int type). */
__attribute__((visibility("default")))
int FMOD5_System_SetOutput(void* system, int output_type) {
    return original_set_output(system, output_type);
}
#endif

#ifndef RUDROID56_FMOD_NO_JNI_ONLOAD
__attribute__((visibility("default")))
int32_t JNI_OnLoad(void* vm, void* reserved) {
    (void)vm;
    (void)reserved;
    g_jni_calls += 1;
    g_jni_result = 0x00010006; /* JNI_VERSION_1_6 */
    return g_jni_result;
}
#endif
