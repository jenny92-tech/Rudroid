// INPUT:  (none) — self-contained C, compiled with gcc -shared -fPIC -O0, loaded via dlopen
// OUTPUT:  il2cpp_* fake API, rudroid54_* fake game model and event readback exports
// POS:    Ticket 54 Silksong fake IL2CPP object-model fixture: real callable functions
//         play libil2cpp's metadata API and the patched host/game methods, so gamepatch
//         can bind via so_symbol, resolve, install real detours and then assert observable behavior changes.
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Compiled by the gamepatch integration tests (and the ticket-54 red probe)
// with `gcc -shared -fPIC -O0` and loaded via dlopen; the test's HostApiV0
// so_symbol implementation resolves the il2cpp_* exports on this handle, so
// the Rust patch binds the SAME C surface the reference resolves inside the
// real guest. Method-info layout matches the il2cpp ABI the patch reads:
// the FIRST word of a MethodInfo is the native entry (`*(uintptr_t*)mi`).
//
// Bodies are compiled -O0 so every hook-target entry keeps a relocatable
// frame prologue (stp pair; only the entry word is ever patched). Every body
// appends one event to the module-local buffer; the tests read the buffer
// through rudroid54_event_count/_at to assert exact call sequences (ticket 53
// spy convention). Event kinds are mirrored by the Rust tests.

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

static int rudroid54_streq(const char* a, const char* b) {
    if (!a || !b) return 0;
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}

/* ---- event buffer (module-local, rudroid53 spy convention) ---- */

#define EV_QUIT_NOARGS_BODY   1
#define EV_QUIT_CODE_BODY     2
#define EV_PARTICLES_ENABLE   3
#define EV_PARTICLES_DISABLE  4
#define EV_PRELOAD_BODY       5
#define EV_MOSS_UPDATE_BODY   6
#define EV_CLAMP_BODY         7
#define EV_AUTO_BODY          8
#define EV_UNLOAD_ALL         9
#define EV_MENU_QUIT_BODY     10

struct Rudroid54Event {
    int kind;
    long long a;
    long long b;
};

#define RUDROID54_EVENT_MAX 128
static struct Rudroid54Event rudroid54_events[RUDROID54_EVENT_MAX];
static int rudroid54_event_count_v = 0;

static void rudroid54_note(int kind, long long a, long long b) {
    if (rudroid54_event_count_v >= RUDROID54_EVENT_MAX) return;
    rudroid54_events[rudroid54_event_count_v].kind = kind;
    rudroid54_events[rudroid54_event_count_v].a = a;
    rudroid54_events[rudroid54_event_count_v].b = b;
    ++rudroid54_event_count_v;
}

int rudroid54_event_count(void) { return rudroid54_event_count_v; }

int rudroid54_event_at(int index, struct Rudroid54Event* out) {
    if (index < 0 || index >= rudroid54_event_count_v || out == 0) return 0;
    *out = rudroid54_events[index];
    return 1;
}

void rudroid54_reset_events(void) { rudroid54_event_count_v = 0; }

/* ---- fake game state ---- */

#define FAKE_SCREEN_WIDTH 960
#define FAKE_SCREEN_HEIGHT 720
static int bad_menu_abi;
void rudroid54_set_bad_menu_abi(int bad) { bad_menu_abi = bad; }

/* The native "supported aspect range" lower bound of the fake
 * AutoScaleViewport pipeline (TeamCherry's MinMaxFloat clamp). Any aspect
 * below this is RAISED by the native code — exactly the lower bound the
 * reference clamp_hook removes. */
#define FAKE_NATIVE_MIN_ASPECT 1.4f

/* ---- hook-target bodies (exported so tests can assert that the fake
 * object model resolves exactly these entries) ---- */

/* UnityEngine.Application.Quit() — static, argc 0. The reference suppress
 * hook swallows the call: after install the body must NOT run. */
void rudroid54_quit_noargs_body(void* method) {
    (void)method;
    rudroid54_note(EV_QUIT_NOARGS_BODY, 0, 0);
}

/* UnityEngine.Application.Quit(int) — static, argc 1. */
void rudroid54_quit_code_body(int32_t code, void* method) {
    (void)method;
    rudroid54_note(EV_QUIT_CODE_BODY, code, 0);
}

/* Confirmed UI action preserves its body before both Application overloads. */
void rudroid54_menu_quit_body(void* self, void* method) {
    rudroid54_note(EV_MENU_QUIT_BODY, (intptr_t)self, (intptr_t)method);
    rudroid54_quit_noargs_body(0);
    rudroid54_quit_code_body(7, 0);
}

/* SceneParticlesController.EnableParticles(bool) — instance, argc 1. The
 * reference hook calls the ORIGINAL with `true` (its no-scene-particles
 * path) and then calls DisableParticles itself. */
void rudroid54_particles_enable_body(void* self, bool enable, void* method) {
    (void)self; (void)method;
    rudroid54_note(EV_PARTICLES_ENABLE, enable, 0);
}

/* SceneParticlesController.DisableParticles() — instance, argc 0. */
void rudroid54_particles_disable_body(void* self, void* method) {
    (void)self; (void)method;
    rudroid54_note(EV_PARTICLES_DISABLE, 0, 0);
}

/* AudioPreloader.PreloadClip(...) — instance, argc 1. The reference
 * replaces this with a no-op. */
void rudroid54_preload_body(void* self, void* clip, void* method) {
    (void)self; (void)clip; (void)method;
    static long long calls = 0;
    ++calls;
    rudroid54_note(EV_PRELOAD_BODY, calls, 0);
}

/* MossClump.Update() — instance, argc 0. Reference freeze hook: no-op. */
void rudroid54_moss_update_body(void* self, void* method) {
    (void)self; (void)method;
    static long long calls = 0;
    ++calls;
    rudroid54_note(EV_MOSS_UPDATE_BODY, calls, 0);
}

/* TeamCherry.SharedUtils.MinMaxFloat.GetClampedBetween(float) — instance,
 * argc 1, returns Single. The native lower bound: values below
 * FAKE_NATIVE_MIN_ASPECT come back raised. */
float rudroid54_clamp_body(void* self, float value, void* method) {
    (void)self; (void)method;
    float clamped = value < FAKE_NATIVE_MIN_ASPECT ? FAKE_NATIVE_MIN_ASPECT : value;
    union { float f; int32_t i; } bits;
    bits.f = clamped;
    rudroid54_note(EV_CLAMP_BODY, bits.i, 0);
    return clamped;
}

/* ForceCameraAspect.AutoScaleViewport() — instance, argc 0, returns Single.
 * The native body computes the screen aspect and pushes it through the
 * MinMaxFloat clamp (the call chain the reference scope hook observes). */
float rudroid54_auto_body(void* self, void* method) {
    (void)self; (void)method;
    float aspect = (float)FAKE_SCREEN_WIDTH / (float)FAKE_SCREEN_HEIGHT;
    float result = rudroid54_clamp_body(0, aspect, 0);
    union { float f; int32_t i; } bits;
    bits.f = result;
    rudroid54_note(EV_AUTO_BODY, bits.i, 0);
    return result;
}

/* UnityEngine.Screen.get_width/get_height — static, argc 0 (the reference
 * viewport calls them with the resolved MethodInfo pointer). */
int32_t rudroid54_screen_width_body(void* method) {
    (void)method;
    return FAKE_SCREEN_WIDTH;
}

int32_t rudroid54_screen_height_body(void* method) {
    (void)method;
    return FAKE_SCREEN_HEIGHT;
}

/* Convenience callers: the tests assert the model-resolved entries equal
 * these bodies, then call THROUGH the resolved entries (the guest path). */
void rudroid54_call_quit(void) { rudroid54_quit_noargs_body(0); }
void rudroid54_call_quit_code(int32_t code) { rudroid54_quit_code_body(code, 0); }
void rudroid54_call_enable_particles(bool enable) {
    rudroid54_particles_enable_body(0, enable, 0);
}
void rudroid54_call_preload(void) { rudroid54_preload_body(0, 0, 0); }
void rudroid54_call_moss_update(void) { rudroid54_moss_update_body(0, 0); }
float rudroid54_call_auto_scale(void) { return rudroid54_auto_body(0, 0); }
float rudroid54_call_clamp(float value) { return rudroid54_clamp_body(0, value, 0); }
int32_t rudroid54_screen_width(void) { return FAKE_SCREEN_WIDTH; }
int32_t rudroid54_screen_height(void) { return FAKE_SCREEN_HEIGHT; }

/* Managed String fixture exposed only through IL2CPP string APIs. */
static struct { int32_t length; uint16_t chars[64]; } title_scene;
static int title_error, bad_title_abi, title_queries;
static void (*title_callback)(void);
void rudroid54_set_scene(const char* name) {
    int i = 0;
    while (name[i] && i < 63) { title_scene.chars[i] = (uint8_t)name[i]; ++i; }
    title_scene.length = i;
}
void rudroid54_set_title_error(int error) { title_error = error; }
void rudroid54_set_bad_title_abi(int bad) { bad_title_abi = bad; }
void rudroid54_set_title_callback(void (*callback)(void)) { title_callback = callback; }
int rudroid54_title_queries(void) { return title_queries; }
void* il2cpp_thread_current(void) { return &title_scene; }
int32_t il2cpp_string_length(void* string) {
    return string == &title_scene ? title_scene.length : -1;
}
#ifndef RUDROID54_MISSING_TITLE_API
const uint16_t* il2cpp_string_chars(void* string) {
    return string == &title_scene ? title_scene.chars : 0;
}
#endif
void* rudroid54_scene_body(void* method) { (void)method; return &title_scene; }
void rudroid54_unload_body(bool destroy, void* method) {
    (void)method; rudroid54_note(EV_UNLOAD_ALL, destroy, 0);
}

/* ---- fake metadata model ---- */

struct Rudroid54Method {
    void* fn;            /* FIRST word: the native entry the patch reads */
    const char* name;
    int argc;
};

struct Rudroid54Class {
    const char* ns;
    const char* name;
    const struct Rudroid54Method* methods;
    int method_count;
};

struct Rudroid54Image {
    const char* name;
    const struct Rudroid54Class* classes;
    int class_count;
};

struct Rudroid54Assembly {
    const struct Rudroid54Image* image;
};

static const struct Rudroid54Method core_application_methods[] = {
    { (void*)rudroid54_quit_noargs_body, "Quit", 0 },
    { (void*)rudroid54_quit_code_body, "Quit", 1 },
    { (void*)rudroid54_scene_body, "get_loadedLevelName", 0 },
};
static const struct Rudroid54Method core_screen_methods[] = {
    { (void*)rudroid54_screen_width_body, "get_width", 0 },
    { (void*)rudroid54_screen_height_body, "get_height", 0 },
};
static const struct Rudroid54Method bundle_methods[] = {
    { (void*)rudroid54_unload_body, "UnloadAllAssetBundles", 1 },
};
static const struct Rudroid54Class core_classes[] = {
    { "UnityEngine", "Application", core_application_methods, 3 },
    { "UnityEngine", "Screen", core_screen_methods, 2 },
    { "UnityEngine", "AssetBundle", bundle_methods, 1 },
};
static const struct Rudroid54Image core_image = {
    "UnityEngine.CoreModule", core_classes, 3,
};

static const struct Rudroid54Method game_particles_methods[] = {
    { (void*)rudroid54_particles_enable_body, "EnableParticles", 1 },
    { (void*)rudroid54_particles_disable_body, "DisableParticles", 0 },
};
static const struct Rudroid54Method game_preloader_methods[] = {
    { (void*)rudroid54_preload_body, "PreloadClip", 1 },
};
static const struct Rudroid54Method game_moss_methods[] = {
    { (void*)rudroid54_moss_update_body, "Update", 0 },
};
static const struct Rudroid54Method game_force_aspect_methods[] = {
    { (void*)rudroid54_auto_body, "AutoScaleViewport", 0 },
};
static const struct Rudroid54Method game_menu_methods[] = {
    { (void*)rudroid54_menu_quit_body, "QuitGame", 0 },
};
static const struct Rudroid54Class game_classes[] = {
    { "", "SceneParticlesController", game_particles_methods, 2 },
    { "", "AudioPreloader", game_preloader_methods, 1 },
    { "", "MossClump", game_moss_methods, 1 },
    { "", "ForceCameraAspect", game_force_aspect_methods, 1 },
    { "", "UIManager", game_menu_methods, 1 },
};
static const struct Rudroid54Image game_image = {
    "Silksong.dll", game_classes, 5,
};
void rudroid54_call_menu_quit(void) {
    rudroid54_menu_quit_body((void*)&game_classes[4], (void*)&game_menu_methods[0]);
}

static const struct Rudroid54Method shared_minmax_methods[] = {
    { (void*)rudroid54_clamp_body, "GetClampedBetween", 1 },
};
static const struct Rudroid54Class shared_classes[] = {
    { "TeamCherry.SharedUtils", "MinMaxFloat", shared_minmax_methods, 1 },
};
static const struct Rudroid54Image shared_image = {
    "TeamCherry.SharedUtils", shared_classes, 1,
};

/* The domain: three assemblies across three images, so the patch's
 * find_class_in_domain / find_class_named walks are exercised for real.
 * il2cpp semantics: the domain hands out an array of ASSEMBLY POINTERS. */
static const struct Rudroid54Assembly rudroid54_assemblies[3] = {
    { &core_image },
    { &game_image },
    { &shared_image },
};
static const struct Rudroid54Assembly* const rudroid54_domain[3] = {
    &rudroid54_assemblies[0],
    &rudroid54_assemblies[1],
    &rudroid54_assemblies[2],
};

/* ---- il2cpp C API (the surface the reference patch binds) ---- */

/* Ticket 78: metadata is unusable until il2cpp_init returns — the domain
 * handle stays NULL before that, mirroring the real guest. */
static int rudroid54_il2cpp_ready = 0;

int il2cpp_init(const char* name) {
    (void)name;
    rudroid54_il2cpp_ready = 1;
    return 1;
}

void* il2cpp_domain_get(void) {
    if (!rudroid54_il2cpp_ready) return 0;
    return (void*)rudroid54_domain; /* opaque domain handle */
}

void** il2cpp_domain_get_assemblies(void* domain, size_t* size) {
    if (domain != (void*)rudroid54_domain) return 0;
    if (size) *size = 3;
    return (void**)rudroid54_domain;
}

void* il2cpp_assembly_get_image(void* assembly) {
    const struct Rudroid54Assembly* assembly_ = (const struct Rudroid54Assembly*)assembly;
    return assembly_ ? (void*)assembly_->image : 0;
}

const char* il2cpp_image_get_name(void* image) {
    const struct Rudroid54Image* img = (const struct Rudroid54Image*)image;
    if (bad_menu_abi == 8 && img == &game_image) return "UnrelatedAssembly";
    return img ? img->name : 0;
}

size_t il2cpp_image_get_class_count(void* image) {
    const struct Rudroid54Image* img = (const struct Rudroid54Image*)image;
    return img ? (size_t)img->class_count : 0;
}

void* il2cpp_image_get_class(void* image, size_t index) {
    const struct Rudroid54Image* img = (const struct Rudroid54Image*)image;
    if (!img || index >= (size_t)img->class_count) return 0;
    return (void*)&img->classes[index];
}

void* il2cpp_class_from_name(void* image, const char* ns, const char* name) {
    const struct Rudroid54Image* img = (const struct Rudroid54Image*)image;
    int i;
    if (!img || !ns || !name) return 0;
    if (bad_menu_abi == 9 && rudroid54_streq(name, "UIManager")) return 0;
    for (i = 0; i < img->class_count; ++i) {
        const struct Rudroid54Class* klass = &img->classes[i];
        if (rudroid54_streq(klass->ns, ns) && rudroid54_streq(klass->name, name))
            return (void*)klass;
    }
    return 0;
}

void* il2cpp_class_get_methods(void* klass, void** iterator) {
    const struct Rudroid54Class* k = klass;
    if (!k || !iterator) return 0;
    size_t index = (uintptr_t)*iterator;
    int duplicate = bad_menu_abi == 7 && rudroid54_streq(k->name, "UIManager");
    if (index >= (size_t)(k->method_count + duplicate)) return 0;
    *iterator = (void*)(index + 1);
    return (void*)&k->methods[duplicate ? 0 : index];
}
bool il2cpp_method_is_generic(void* raw) {
    const struct Rudroid54Method* m = raw;
    return bad_menu_abi == 5 && rudroid54_streq(m->name, "QuitGame");
}
bool il2cpp_method_is_inflated(void* raw) {
    const struct Rudroid54Method* m = raw;
    return bad_menu_abi == 6 && rudroid54_streq(m->name, "QuitGame");
}

void* il2cpp_class_get_method_from_name(void* klass, const char* name, int argc) {
    const struct Rudroid54Class* k = (const struct Rudroid54Class*)klass;
    int i;
    if (!k || !name) return 0;
    for (i = 0; i < k->method_count; ++i) {
        if (k->methods[i].argc == argc && rudroid54_streq(k->methods[i].name, name))
            return (void*)&k->methods[i];
    }
    return 0;
}

const char* il2cpp_class_get_name(void* klass) {
    const struct Rudroid54Class* k = (const struct Rudroid54Class*)klass;
    return k ? k->name : 0;
}

const char* il2cpp_class_get_namespace(void* klass) {
    const struct Rudroid54Class* k = (const struct Rudroid54Class*)klass;
    return k ? k->ns : 0;
}

/* The remaining exports are part of the reference init's REQUIRED set (the
 * patch refuses to arm without them); the fixture models them minimally. */

struct Rudroid54FakeType {
    const void* klass;
};
static struct Rudroid54FakeType rudroid54_type_slot;

void* il2cpp_class_get_type(void* klass) {
    rudroid54_type_slot.klass = klass;
    return &rudroid54_type_slot;
}

void* il2cpp_type_get_object(const void* type) {
    return (void*)type;
}

void* il2cpp_object_get_class(void* object) {
    return object;
}

void* il2cpp_class_get_field_from_name(void* klass, const char* name) {
    (void)klass;
    (void)name;
    return 0; /* the modeled classes carry no fields the patch reads */
}

size_t il2cpp_field_get_offset(void* field) {
    (void)field;
    return 0;
}

void il2cpp_field_get_value(void* obj, void* field, void* out) {
    (void)obj; (void)field; (void)out;
}

void* il2cpp_runtime_invoke(void* method, void* instance, void** params,
                            void** exc) {
    const struct Rudroid54Method* mi = (const struct Rudroid54Method*)method;
    (void)instance; (void)params;
    if (exc) *exc = 0;
    if (!mi || !mi->fn) return 0;
    if (rudroid54_streq(mi->name, "get_loadedLevelName")) {
        ++title_queries;
        if (title_callback) {
            void (*callback)(void) = title_callback;
            title_callback = 0; /* one reentry, no fixture recursion */
            callback();
        }
        if (title_error == 1) { if (exc) *exc = &title_scene; return 0; }
        if (title_error == 3) return 0;
        return rudroid54_scene_body(method);
    }
    if (rudroid54_streq(mi->name, "UnloadAllAssetBundles")) {
        if (title_error == 2) { if (exc) *exc = &title_scene; return 0; }
        rudroid54_unload_body(*(bool*)params[0], method);
        return 0;
    }
    return mi->fn; /* legacy tests expose the native entry */
}

/* Stripped-variant hook: -DRUDROID54_MISSING_EXPORTS removes one REQUIRED
 * export so the tests can prove the explicit bind failure. */
#ifndef RUDROID54_MISSING_EXPORTS
void* il2cpp_string_new(const char* value) {
    return (void*)value;
}
#endif

int64_t il2cpp_gc_get_heap_size(void) { return 0; }
int64_t il2cpp_gc_get_used_size(void) { return 0; }

const char* il2cpp_method_get_name(void* method) {
    const struct Rudroid54Method* mi = (const struct Rudroid54Method*)method;
    return mi ? mi->name : 0;
}

uint32_t il2cpp_method_get_param_count(void* method) {
    const struct Rudroid54Method* mi = (const struct Rudroid54Method*)method;
    if (mi && bad_menu_abi == 4 && rudroid54_streq(mi->name, "QuitGame")) return 1;
    return mi ? (uint32_t)mi->argc : 0;
}

/* Typed metadata: byref is bit 8, low byte is Il2CppTypeEnum. */
static int bad_method_abi;
void rudroid54_set_bad_method_abi(int bad) { bad_method_abi = bad; }

uint32_t il2cpp_method_get_flags(void *raw, uint32_t *implementation) {
    const struct Rudroid54Method *method = raw;
    if (implementation) *implementation = 0;
    if (bad_menu_abi == 1 && rudroid54_streq(method->name, "QuitGame")) return 0x10;
    if (bad_title_abi == 1 && rudroid54_streq(method->name, "get_loadedLevelName")) return 0;
    return rudroid54_streq(method->name, "get_loadedLevelName") || rudroid54_streq(method->name, "UnloadAllAssetBundles") || rudroid54_streq(method->name, "Quit") || rudroid54_streq(method->name, "get_width") || rudroid54_streq(method->name, "get_height") ? 0x10 : 0;
}
void *il2cpp_method_get_return_type(void *raw) {
    const struct Rudroid54Method *method = raw;
    if (rudroid54_streq(method->name, "QuitGame")) {
        if (bad_menu_abi == 2) return (void*)2;
        if (bad_menu_abi == 3) return (void*)0x101;
    }
    uintptr_t kind = bad_method_abi ? 2 :
        rudroid54_streq(method->name, "get_loadedLevelName") ? (bad_title_abi == 2 ? 8 : 0x0e) :
        rudroid54_streq(method->name, "get_width") || rudroid54_streq(method->name, "get_height") ? 8 :
        rudroid54_streq(method->name, "AutoScaleViewport") || rudroid54_streq(method->name, "GetClampedBetween") ? 0x0c : 1;
    return (void *)kind;
}
void *il2cpp_method_get_param(void *raw, uint32_t index) {
    const struct Rudroid54Method *method = raw;
    if (index >= (uint32_t)method->argc) return 0;
    uintptr_t kind = rudroid54_streq(method->name, "set_rect_Injected") ? 0x111 :
        rudroid54_streq(method->name, "UnloadAllAssetBundles") ? (bad_title_abi == 3 ? 0x102 : 2) :
        rudroid54_streq(method->name, "EnableParticles") ? 2 :
        rudroid54_streq(method->name, "Quit") ? 8 :
        rudroid54_streq(method->name, "PreloadClip") ? 0x12 : 0x0c;
    return (void *)kind;
}
int32_t il2cpp_type_get_type(void *type) { return (uintptr_t)type & 0xff; }
bool il2cpp_type_is_byref(void *type) { return ((uintptr_t)type & 0x100) != 0; }
