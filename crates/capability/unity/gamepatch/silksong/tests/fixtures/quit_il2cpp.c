// INPUT:  base Silksong metadata model and delayed menu/iterator callbacks
// OUTPUT:  original asynchronous menu quit plus nested-type/ABI rejection controls
// POS:    models Application.Quit occurring after the UI callback has returned
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
#define il2cpp_domain_get_assemblies legacy_domain_assemblies
#define il2cpp_image_get_name legacy_image_name
#define il2cpp_method_get_return_type legacy_return_type
#define il2cpp_method_get_flags legacy_flags
#define rudroid54_call_menu_quit legacy_call_menu_quit
#include "rudroid54_silksong_il2cpp.c"
#undef il2cpp_domain_get_assemblies
#undef il2cpp_image_get_name
#undef il2cpp_method_get_return_type
#undef il2cpp_method_get_flags
#undef rudroid54_call_menu_quit

static int quit_pending, quit_bad;
static int quit_receiver;
void rudroid54_set_bad_deferred_quit(int value) { quit_bad = value; }
static void* quit_factory(void* self, void* method) {
    rudroid54_note(12, (intptr_t)self, (intptr_t)method);
    quit_pending = 1;
    return &quit_receiver;
}
static void deferred_menu(void* self, void* method) {
    rudroid54_note(EV_MENU_QUIT_BODY, (intptr_t)self, (intptr_t)method);
    quit_factory(self, method);
}
static bool quit_step(void* self, void* method) {
    rudroid54_note(11, (intptr_t)self, (intptr_t)method);
    if (quit_pending) {
        quit_pending = 0;
        rudroid54_quit_noargs_body(0);
        rudroid54_quit_code_body(7, 0);
    }
    return false;
}
static const struct Rudroid54Method menu_methods[] = {{ (void*)deferred_menu, "QuitGame", 0 }};
static const struct Rudroid54Method factory_methods[] = {{ (void*)quit_factory, "QuitGame", 0 }};
static const struct Rudroid54Method step_methods[] = {{ (void*)quit_step, "MoveNext", 0 }};
static const struct Rudroid54Class quit_classes[] = {
    { "", "UIManager", menu_methods, 1 },
    { "", "GameManager", factory_methods, 1 },
    { "", "<QuitGame>d__481", step_methods, 1 },
    { "System.Collections", "IEnumerator", 0, 0 },
    { "", "<QuitGame>d__999", step_methods, 1 },
    { "", "<SaveGame>d__481", step_methods, 1 },
};
static const struct Rudroid54Image quit_image = { "Silksong.dll", quit_classes, 6 };
static const struct Rudroid54Assembly quit_assembly = { &quit_image };
static const struct Rudroid54Assembly* quit_domain[] = {
    &rudroid54_assemblies[0], &rudroid54_assemblies[1], &rudroid54_assemblies[2], &quit_assembly,
};
void** il2cpp_domain_get_assemblies(void* domain, size_t* count) {
    if (domain != (void*)rudroid54_domain) return 0;
    *count = 4;
    return (void**)quit_domain;
}
const char* il2cpp_image_get_name(void* image) {
    return image == &game_image ? "OtherTitleSupport.dll" : legacy_image_name(image);
}
void* il2cpp_method_get_return_type(void* method) {
    if (method == &factory_methods[0]) return (void*)(uintptr_t)(quit_bad == 1 ? 1 : 0x812);
    if (method == &step_methods[0]) return (void*)(uintptr_t)(quit_bad == 5 ? 1 : quit_bad == 7 ? 0x102 : 2);
    return legacy_return_type(method);
}
uint32_t il2cpp_method_get_flags(void* method, uint32_t* implementation) {
    if (method == &step_methods[0] && quit_bad == 6) {
        if (implementation) *implementation = 0;
        return 0x10;
    }
    return legacy_flags(method, implementation);
}
void* il2cpp_class_from_type(void* type) {
    return type == (void*)0x812 ? (void*)&quit_classes[quit_bad == 2 ? 0 : 3] : 0;
}
#ifndef RUDROID54_MISSING_NESTED_TYPES
void* il2cpp_class_get_nested_types(void* klass, void** iterator) {
    if (klass != &quit_classes[1]) return 0;
    size_t index = (uintptr_t)*iterator;
    if (quit_bad == 8) return (void*)&quit_classes[2];
    if (index >= (size_t)(quit_bad == 4 ? 2 : 1)) return 0;
    *iterator = (void*)(index + 1);
    if (quit_bad == 3) return (void*)&quit_classes[5];
    return (void*)&quit_classes[index == 1 || quit_bad == 9 ? 4 : 2];
}
#endif
void rudroid54_call_menu_quit(void) { deferred_menu(&quit_receiver, (void*)&menu_methods[0]); }
bool rudroid54_advance_quit(void) { return quit_step(&quit_receiver, (void*)&step_methods[0]); }
