// INPUT:  (none) — self-contained C, compiled with gcc -shared -fPIC -O0, loaded via dlopen
// OUTPUT:  il2cpp_* fake API, same-named type counterexamples and rudroid54_hk_* game/readback exports
// POS:    Ticket 54 Hollow Knight fake IL2CPP object-model fixture: real callable
//         functions and real memory layouts play Camera/tk2dCamera/ForceCameraAspect and other patched
//         methods, so the ungated gamepatch can resolve them, install real detours and assert viewport-fit behavior.
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Compiled by the hollow_knight integration tests with
// `gcc -shared -fPIC -O0` and loaded via dlopen (the test's HostApiV0
// so_symbol resolves the il2cpp_* exports on this handle). MethodInfo
// layout matches the il2cpp ABI the patch reads: the FIRST word of a
// MethodInfo is the native entry. FIELD offsets are modeled per class so
// the patch's resolve-field path (class_get_field_from_name +
// field_get_offset) drives the real tk2d layout, mirroring the reference
// resolve_field_offset flow.
//
// Bodies are compiled -O0 so every hook-target entry keeps a relocatable
// frame prologue (only the entry word is ever patched). Bodies append one
// event each; float observations are recorded as bit patterns. The fake
// screen is 960x720 (aspect 4:3 < 16:9), so the reference width-safe scale
// is (16/9)/(4/3) = 4/3 — the value the tests assert on every scaled
// surface.

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

static int rudroid54_streq(const char* a, const char* b) {
    if (!a || !b) return 0;
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}

/* ---- event buffer (module-local, rudroid53 spy convention) ---- */

#define EV_RECT_BODY            1 /* a=tag(1 main / 2 hud) b=rect.width bits */
#define EV_ORTHO_BODY           2 /* a=tag b=ortho bits (value the body saw) */
#define EV_TK2D_MATRIX_BODY     3 /* a=ortho bits AT BODY ENTRY */
#define EV_OVERSCANVP_BODY      4 /* a=adjustment bits */
#define EV_SET_OVERSCAN_BODY    5 /* a=value bits */
#define EV_AUTO_BODY            6 /* a=return bits */

struct Rudroid54HkEvent {
    int kind;
    long long a;
    long long b;
};

#define RUDROID54_HK_EVENT_MAX 128
static struct Rudroid54HkEvent hk_events[RUDROID54_HK_EVENT_MAX];
static int hk_event_count_v = 0;

static void hk_note(int kind, long long a, long long b) {
    if (hk_event_count_v >= RUDROID54_HK_EVENT_MAX) return;
    hk_events[hk_event_count_v].kind = kind;
    hk_events[hk_event_count_v].a = a;
    hk_events[hk_event_count_v].b = b;
    ++hk_event_count_v;
}

int rudroid54_hk_event_count(void) { return hk_event_count_v; }

int rudroid54_hk_event_at(int index, struct Rudroid54HkEvent* out) {
    if (index < 0 || index >= hk_event_count_v || out == 0) return 0;
    *out = hk_events[index];
    return 1;
}

void rudroid54_hk_reset_events(void) { hk_event_count_v = 0; }

static long long fbits(float v) {
    union { float f; int32_t i; } bits;
    bits.f = v;
    return bits.i;
}

/* ---- fake game state (the surfaces the reference patch mutates) ---- */

#define FAKE_SCREEN_WIDTH 960
#define FAKE_SCREEN_HEIGHT 720

struct Rudroid54Rect { float x, y, width, height; };

/* tk2dCameraSettings — the layout the patch resolves through FIELD offsets.
 * The initial values are the native overscan state the width-safe pass
 * rescales (ortho 3.0 @ ppm 100, zoom 1.0). */
struct Rudroid54Tk2dSettings {
    float orthographicSize;             /* 0x00 */
    float orthographicPixelsPerMeter;   /* 0x04 */
    int32_t orthographicType;           /* 0x08 */
    struct Rudroid54Rect rect;             /* 0x0c */
};

/* tk2dCamera — field offsets resolve through the model (resolve_field_offset
 * flow); the struct layout matches the field table below exactly. */
struct Rudroid54Tk2d {
    uint8_t pad0[0x20];
    struct Rudroid54Tk2dSettings* cameraSettings; /* 0x20 */
    int32_t nativeResolutionWidth;             /* 0x28 */
    int32_t nativeResolutionHeight;            /* 0x2c */
    void* unityCamera;                         /* 0x30 */
    float targetResolution[2];                 /* 0x38 */
    float zoomFactor;                          /* 0x40 */
    float padZoom;                             /* 0x44 (alignment) */
    struct Rudroid54Rect screenExtents;           /* 0x48 */
    struct Rudroid54Rect nativeScreenExtents;     /* 0x58 */
};

_Static_assert(offsetof(struct Rudroid54Tk2d, cameraSettings) == 0x20, "layout");
_Static_assert(offsetof(struct Rudroid54Tk2d, zoomFactor) == 0x40, "layout");
_Static_assert(offsetof(struct Rudroid54Tk2d, screenExtents) == 0x48, "layout");
_Static_assert(offsetof(struct Rudroid54Tk2d, nativeScreenExtents) == 0x58, "layout");

static struct Rudroid54Tk2dSettings hk_tk2d_settings = {
    3.0f, 100.0f, 1, { 0.0f, 0.0f, 1.0f, 1.0f },
};
static struct Rudroid54Tk2d hk_tk2d = {
    { 0 }, &hk_tk2d_settings, 1024, 768, 0, { 1024.0f, 768.0f }, 1.0f, 0.0f,
    { 0.0f, 0.0f, 1.0f, 1.0f }, { 0.0f, 0.0f, 1.0f, 1.0f },
};

/* Two cameras the rect/ortho hooks mutate: the main camera starts with a
 * non-full overscan rect, the HUD camera with a narrowed ortho. */
struct Rudroid54Camera {
    struct Rudroid54Rect rect;
    float orthographicSize;
};
static struct Rudroid54Camera hk_main_camera = {
    { 0.05f, 0.05f, 0.90f, 0.90f }, 3.0f,
};
static struct Rudroid54Camera hk_hud_camera = {
    { 0.1f, 0.1f, 0.8f, 0.8f }, 2.0f,
};

/* ForceCameraAspect deliberately differs from the reference offsets;
 * metadata must select these fields rather than read self+0x18/self+0x20. */
struct Rudroid54ForceAspect {
    uint8_t pad0[0x28];
    void* tk2dCam; /* 0x28 */
    void* hudCam;  /* 0x30 */
};
static struct Rudroid54ForceAspect hk_force_aspect = {
    { 0 }, &hk_tk2d, &hk_hud_camera,
};

/* Deliberately different offsets from the game's native layout. */
struct Rudroid54GameCameras {
    uint8_t pad0[0x38];
    void* hudCamera;
    void* mainCamera;
    void* tk2dCam;
};
static struct Rudroid54GameCameras hk_game_cameras = {
    { 0 }, &hk_hud_camera, &hk_main_camera, &hk_tk2d,
};

static int camera_tag(const void* self) {
    return self == (const void*)&hk_main_camera ? 1 : 2;
}

/* ---- hook-target bodies ---- */

/* UnityEngine.Camera.set_rect_Injected(Rect) — instance, argc 1. The
 * reference force_full_viewport hook rewrites the rect to (0,0,1,1) BEFORE
 * the original runs, so the body observes the forced rect. */
void rudroid54_hk_set_rect_body(void* self, struct Rudroid54Rect* rect, void* method) {
    (void)method;
    struct Rudroid54Camera* cam = (struct Rudroid54Camera*)self;
    cam->rect = *rect;
    hk_note(EV_RECT_BODY, camera_tag(self), fbits(rect->width));
}

/* UnityEngine.Camera.set_orthographicSize(float) — instance, argc 1. The
 * ui_orthographic_width_safe hook scales the VALUE before the original. */
void rudroid54_hk_set_ortho_body(void* self, float value, void* method) {
    (void)method;
    struct Rudroid54Camera* cam = (struct Rudroid54Camera*)self;
    cam->orthographicSize = value;
    hk_note(EV_ORTHO_BODY, camera_tag(self), fbits(value));
}

float rudroid54_hk_get_ortho_body(void* self, void* method) {
    (void)method;
    return ((struct Rudroid54Camera*)self)->orthographicSize;
}

/* tk2dCamera.UpdateCameraMatrix() — instance, argc 0. The tk2d_width_safe
 * hook rewrites the settings/zoom fields BEFORE the original runs, so the
 * body entry records the ALREADY patched ortho. */
void rudroid54_hk_tk2d_matrix_body(void* self, void* method) {
    (void)method;
    struct Rudroid54Tk2d* tk2d = (struct Rudroid54Tk2d*)self;
    hk_note(EV_TK2D_MATRIX_BODY, fbits(tk2d->cameraSettings->orthographicSize), 0);
}

/* ForceCameraAspect.SetOverscanViewport(float) — instance, argc 1. Always-on
 * logging pass-through in the reference; the body records the value. */
void rudroid54_hk_overscan_viewport_body(void* self, float adjustment, void* method) {
    (void)self; (void)method;
    hk_note(EV_OVERSCANVP_BODY, fbits(adjustment), 0);
}

/* ForceCameraAspect.AutoScaleViewport() — instance, argc 0, returns Single.
 * Native return: the aspect fill factor (16/9)/aspect = 4/3 at the fake 4:3
 * screen — NOT 1.0, so the force_full_viewport override to 1.0 is
 * observable. */
float rudroid54_hk_auto_body(void* self, void* method) {
    (void)self; (void)method;
    float aspect = (float)FAKE_SCREEN_WIDTH / (float)FAKE_SCREEN_HEIGHT;
    float ret = (16.0f / 9.0f) / aspect;
    hk_note(EV_AUTO_BODY, fbits(ret), 0);
    return ret;
}

/* GameCameras.SetOverscan(float) — instance, argc 1. */
void rudroid54_hk_set_overscan_body(void* self, float value, void* method) {
    (void)self; (void)method;
    hk_note(EV_SET_OVERSCAN_BODY, fbits(value), 0);
}

/* UnityEngine.Screen.get_width/get_height — static, argc 0. */
int32_t rudroid54_hk_screen_width_body(void* method) {
    (void)method;
    return FAKE_SCREEN_WIDTH;
}

int32_t rudroid54_hk_screen_height_body(void* method) {
    (void)method;
    return FAKE_SCREEN_HEIGHT;
}

/* ---- state getters (the test's observation surface) ---- */

void rudroid54_hk_main_rect(struct Rudroid54Rect* out) { if (out) *out = hk_main_camera.rect; }
void rudroid54_hk_hud_rect(struct Rudroid54Rect* out) { if (out) *out = hk_hud_camera.rect; }
float rudroid54_hk_main_ortho(void) { return hk_main_camera.orthographicSize; }
float rudroid54_hk_hud_ortho(void) { return hk_hud_camera.orthographicSize; }
float rudroid54_hk_tk2d_ortho(void) { return hk_tk2d_settings.orthographicSize; }
float rudroid54_hk_tk2d_ppm(void) { return hk_tk2d_settings.orthographicPixelsPerMeter; }
float rudroid54_hk_tk2d_zoom(void) { return hk_tk2d.zoomFactor; }

/* Convenience callers (tests assert model-resolved entries equal these
 * bodies, then call THROUGH the resolved entries — the guest path). */
void rudroid54_hk_call_set_rect(int tag, float x, float y, float w, float h) {
    struct Rudroid54Rect rect = { x, y, w, h };
    void* self = tag == 1 ? (void*)&hk_main_camera : (void*)&hk_hud_camera;
    rudroid54_hk_set_rect_body(self, &rect, 0);
}
void rudroid54_hk_call_set_ortho(int tag, float value) {
    void* self = tag == 1 ? (void*)&hk_main_camera : (void*)&hk_hud_camera;
    rudroid54_hk_set_ortho_body(self, value, 0);
}
void rudroid54_hk_call_update_matrix(void) { rudroid54_hk_tk2d_matrix_body(&hk_tk2d, 0); }
float rudroid54_hk_call_auto_scale(void) { return rudroid54_hk_auto_body(&hk_force_aspect, 0); }
void rudroid54_hk_call_set_overscan_viewport(float v) {
    rudroid54_hk_overscan_viewport_body(&hk_force_aspect, v, 0);
}
void rudroid54_hk_call_set_overscan(float v) { rudroid54_hk_set_overscan_body(&hk_game_cameras, v, 0); }
int32_t rudroid54_hk_screen_width(void) { return FAKE_SCREEN_WIDTH; }
int32_t rudroid54_hk_screen_height(void) { return FAKE_SCREEN_HEIGHT; }

/* ---- fake metadata model ---- */

struct Rudroid54HkMethod {
    void* fn;            /* FIRST word: the native entry the patch reads */
    const char* name;
    int argc;
};

struct Rudroid54HkField {
    const char* name;
    size_t offset;
};

struct Rudroid54HkClass {
    const char* ns;
    const char* name;
    const struct Rudroid54HkMethod* methods;
    int method_count;
    const struct Rudroid54HkField* fields;
    int field_count;
};

struct Rudroid54HkImage {
    const char* name;
    const struct Rudroid54HkClass* classes;
    int class_count;
};

struct Rudroid54HkAssembly {
    const struct Rudroid54HkImage* image;
};

static const struct Rudroid54HkMethod core_camera_methods[] = {
    { (void*)rudroid54_hk_set_rect_body, "set_rect_Injected", 1 },
    { (void*)rudroid54_hk_set_ortho_body, "set_orthographicSize", 1 },
    { (void*)rudroid54_hk_get_ortho_body, "get_orthographicSize", 0 },
};
static const struct Rudroid54HkMethod core_screen_methods[] = {
    { (void*)rudroid54_hk_screen_width_body, "get_width", 0 },
    { (void*)rudroid54_hk_screen_height_body, "get_height", 0 },
};
static const struct Rudroid54HkClass core_classes[] = {
    { "UnityEngine", "Camera", core_camera_methods, 3, 0, 0 },
    { "UnityEngine", "Screen", core_screen_methods, 2, 0, 0 },
    { "UnityEngine", "Rect", 0, 0, 0, 0 },
};
static const struct Rudroid54HkImage core_image = {
    "UnityEngine.CoreModule", core_classes, 3,
};

static const struct Rudroid54HkMethod asm_force_aspect_methods[] = {
    { (void*)rudroid54_hk_overscan_viewport_body, "SetOverscanViewport", 1 },
    { (void*)rudroid54_hk_auto_body, "AutoScaleViewport", 0 },
};
static const struct Rudroid54HkMethod asm_tk2d_methods[] = {
    { (void*)rudroid54_hk_tk2d_matrix_body, "UpdateCameraMatrix", 0 },
};
static const struct Rudroid54HkMethod asm_gamecameras_methods[] = {
    { (void*)rudroid54_hk_set_overscan_body, "SetOverscan", 1 },
};
static const struct Rudroid54HkField tk2d_fields[] = {
    { "cameraSettings", 0x20 },
    { "nativeResolutionWidth", 0x28 },
    { "nativeResolutionHeight", 0x2c },
    { "_unityCamera", 0x30 },
    { "_targetResolution", 0x38 },
    { "zoomFactor", 0x40 },
    { "_screenExtents", 0x48 },
    { "_nativeScreenExtents", 0x58 },
};
static const struct Rudroid54HkField tk2d_settings_fields[] = {
    { "orthographicSize", 0x00 },
    { "orthographicPixelsPerMeter", 0x04 },
    { "orthographicType", 0x08 },
    { "rect", 0x0c },
};
static const struct Rudroid54HkField force_aspect_fields[] = {
    { "tk2dCam", offsetof(struct Rudroid54ForceAspect, tk2dCam) },
    { "hudCam", offsetof(struct Rudroid54ForceAspect, hudCam) },
};
static const struct Rudroid54HkField game_camera_fields[] = {
    { "hudCamera", offsetof(struct Rudroid54GameCameras, hudCamera) },
    { "mainCamera", offsetof(struct Rudroid54GameCameras, mainCamera) },
    { "tk2dCam", offsetof(struct Rudroid54GameCameras, tk2dCam) },
};
static const struct Rudroid54HkClass asm_classes[] = {
    { "", "ForceCameraAspect", asm_force_aspect_methods, 2, force_aspect_fields, 2 },
    { "", "tk2dCamera", asm_tk2d_methods, 1, tk2d_fields, 8 },
    { "", "tk2dCameraSettings", 0, 0, tk2d_settings_fields, 4 },
    { "", "GameCameras", asm_gamecameras_methods, 1, game_camera_fields, 3 },
};
static const struct Rudroid54HkImage asm_image = {
    "Assembly-CSharp", asm_classes, 4,
};

static const struct Rudroid54HkAssembly hk_assemblies[2] = {
    { &core_image },
    { &asm_image },
};
/* il2cpp semantics: the domain hands out an array of ASSEMBLY POINTERS. */
static const struct Rudroid54HkAssembly* const hk_domain[2] = {
    &hk_assemblies[0],
    &hk_assemblies[1],
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
    return (void*)hk_domain; /* opaque domain handle */
}

void** il2cpp_domain_get_assemblies(void* domain, size_t* size) {
    if (domain != (void*)hk_domain) return 0;
    if (size) *size = 2;
    return (void**)hk_domain;
}

void* il2cpp_assembly_get_image(void* assembly) {
    const struct Rudroid54HkAssembly* assembly_ = (const struct Rudroid54HkAssembly*)assembly;
    return assembly_ ? (void*)assembly_->image : 0;
}

const char* il2cpp_image_get_name(void* image) {
    const struct Rudroid54HkImage* img = (const struct Rudroid54HkImage*)image;
    return img ? img->name : 0;
}

size_t il2cpp_image_get_class_count(void* image) {
    const struct Rudroid54HkImage* img = (const struct Rudroid54HkImage*)image;
    return img ? (size_t)img->class_count : 0;
}

void* il2cpp_image_get_class(void* image, size_t index) {
    const struct Rudroid54HkImage* img = (const struct Rudroid54HkImage*)image;
    if (!img || index >= (size_t)img->class_count) return 0;
    return (void*)&img->classes[index];
}

void* il2cpp_class_from_name(void* image, const char* ns, const char* name) {
    const struct Rudroid54HkImage* img = (const struct Rudroid54HkImage*)image;
    int i;
    if (!img || !ns || !name) return 0;
    for (i = 0; i < img->class_count; ++i) {
        const struct Rudroid54HkClass* klass = &img->classes[i];
        if (rudroid54_streq(klass->ns, ns) && rudroid54_streq(klass->name, name))
            return (void*)klass;
    }
    return 0;
}

void* il2cpp_class_get_method_from_name(void* klass, const char* name, int argc) {
    const struct Rudroid54HkClass* k = (const struct Rudroid54HkClass*)klass;
    int i;
    if (!k || !name) return 0;
    for (i = 0; i < k->method_count; ++i) {
        if (k->methods[i].argc == argc && rudroid54_streq(k->methods[i].name, name))
            return (void*)&k->methods[i];
    }
    return 0;
}

void* il2cpp_class_get_field_from_name(void* klass, const char* name) {
    const struct Rudroid54HkClass* k = (const struct Rudroid54HkClass*)klass;
    int i;
    if (!k || !name) return 0;
    for (i = 0; i < k->field_count; ++i) {
        if (rudroid54_streq(k->fields[i].name, name))
            return (void*)&k->fields[i];
    }
    return 0;
}

size_t il2cpp_field_get_offset(void* field) {
    const struct Rudroid54HkField* f = (const struct Rudroid54HkField*)field;
    return f ? f->offset : 0;
}

const char* il2cpp_class_get_name(void* klass) {
    const struct Rudroid54HkClass* k = (const struct Rudroid54HkClass*)klass;
    return k ? k->name : 0;
}

const char* il2cpp_class_get_namespace(void* klass) {
    const struct Rudroid54HkClass* k = (const struct Rudroid54HkClass*)klass;
    return k ? k->ns : 0;
}

/* The remaining exports are part of the reference init's REQUIRED set; the
 * fixture models them minimally so a full-set bind succeeds and a stripped
 * fixture (test variant) can drop them to prove the explicit failure. */

void* il2cpp_class_get_type(void* klass) { return klass; }
void* il2cpp_type_get_object(const void* type) { return (void*)type; }
void* il2cpp_object_get_class(void* object) { return object; }
void il2cpp_field_get_value(void* obj, void* field, void* out) {
    (void)obj; (void)field; (void)out;
}
void* il2cpp_runtime_invoke(void* method, void* instance, void** params, void** exc) {
    const struct Rudroid54HkMethod* mi = (const struct Rudroid54HkMethod*)method;
    (void)instance; (void)params;
    if (exc) *exc = 0;
    return mi ? mi->fn : 0;
}
void* il2cpp_string_new(const char* value) { return (void*)value; }
int64_t il2cpp_gc_get_heap_size(void) { return 0; }
int64_t il2cpp_gc_get_used_size(void) { return 0; }

/* Typed metadata: byref is bit 8, low byte is Il2CppTypeEnum. */
static int bad_method_abi;
void rudroid54_set_bad_method_abi(int bad) { bad_method_abi = bad; }
static int bad_reference_types;
void rudroid54_set_bad_reference_types(int bad) { bad_reference_types = bad; }
/* Distinct immutable types deliberately sharing the expected names. */
static const struct Rudroid54HkClass shadow_reference_classes[] = {
    { "", "tk2dCameraSettings", 0, 0, 0, 0 },
    { "", "tk2dCamera", 0, 0, 0, 0 },
    { "UnityEngine", "Camera", 0, 0, 0, 0 },
    { "UnityEngine", "Rect", 0, 0, 0, 0 },
};
uint32_t il2cpp_method_get_param_count(void *raw) { return ((const struct Rudroid54HkMethod *)raw)->argc; }

uint32_t il2cpp_method_get_flags(void *raw, uint32_t *implementation) {
    const struct Rudroid54HkMethod *method = raw;
    if (implementation) *implementation = 0;
    return rudroid54_streq(method->name, "Quit") || rudroid54_streq(method->name, "get_width") || rudroid54_streq(method->name, "get_height") ? 0x10 : 0;
}
void *il2cpp_method_get_return_type(void *raw) {
    const struct Rudroid54HkMethod *method = raw;
    uintptr_t kind = bad_method_abi ? 2 :
        rudroid54_streq(method->name, "get_width") || rudroid54_streq(method->name, "get_height") ? 8 :
        rudroid54_streq(method->name, "AutoScaleViewport") || rudroid54_streq(method->name, "get_orthographicSize") || rudroid54_streq(method->name, "GetClampedBetween") ? 0x0c : 1;
    return (void *)kind;
}
void *il2cpp_method_get_param(void *raw, uint32_t index) {
    const struct Rudroid54HkMethod *method = raw;
    if (index >= (uint32_t)method->argc) return 0;
    uintptr_t kind = rudroid54_streq(method->name, "set_rect_Injected") ? 0x40111 :
        rudroid54_streq(method->name, "EnableParticles") ? 2 :
        rudroid54_streq(method->name, "Quit") ? 8 :
        rudroid54_streq(method->name, "PreloadClip") ? 0x12 : 0x0c;
    return (void *)kind;
}
int32_t il2cpp_type_get_type(void *type) { return (uintptr_t)type & 0xff; }
bool il2cpp_type_is_byref(void *type) { return ((uintptr_t)type & 0x100) != 0; }

void *il2cpp_field_get_type(void *raw) {
    const struct Rudroid54HkField *field = raw;
    uintptr_t kind = rudroid54_streq(field->name, "zoomFactor") || rudroid54_streq(field->name, "orthographicSize") || rudroid54_streq(field->name, "orthographicPixelsPerMeter") ? 0x0c :
        rudroid54_streq(field->name, "cameraSettings") ? 0x10012 :
        rudroid54_streq(field->name, "tk2dCam") ? 0x20012 : 0x30012;
    return (void *)kind;
}
int32_t il2cpp_field_get_flags(void *raw) { (void)raw; return 0; }
void *il2cpp_class_from_type(void *type) {
    if (bad_reference_types == 1) return (void *)&asm_classes[3]; /* wrong name/layout */
    uintptr_t identity = (uintptr_t)type >> 16;
    if (bad_reference_types == 2 && identity >= 1 && identity <= 4)
        return (void *)&shadow_reference_classes[identity - 1]; /* matching names, wrong identity */
    switch ((uintptr_t)type >> 16) {
        case 1: return (void *)&asm_classes[2]; /* tk2dCameraSettings */
        case 2: return (void *)&asm_classes[1]; /* tk2dCamera */
        case 3: return (void *)&core_classes[0]; /* Camera */
        case 4: return (void *)&core_classes[2]; /* Rect */
        default: return 0;
    }
}
