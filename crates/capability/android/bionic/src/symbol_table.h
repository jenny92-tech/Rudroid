// INPUT:  stddef.h, stdint.h, string.h
// OUTPUT:  RudroidSymbol entries, RUDROID_SYMBOL table macros and RUDROID_SYMBOL_PROVIDER lookup/enumeration functions
// POS:    one data-table shape for every android_bionic C symbol source, checked by the Rust registry
#pragma once
#include <stddef.h>
#include <stdint.h>
#include <string.h>

// One Android-visible name and the native address that answers it. Data
// symbols (environ) use their object address.
typedef struct {
    const char *name;
    const void *address;
} RudroidSymbol;

// Table entry for a different implementation name (`{"sigaction", disposition}`).
#define RUDROID_SYMBOL(name, function) {name, (const void *)(function)},
// Table entry whose Android name equals the native identifier.
#define RUDROID_SYMBOL_SAME(function) {#function, (const void *)(function)},
#define RUDROID_SYMBOL_COUNT(table) (sizeof(table) / sizeof((table)[0]))

static inline uintptr_t rudroid_symbol_find(const RudroidSymbol *table, size_t count, const char *name) {
    for (size_t index = 0; index < count; ++index)
        if (!strcmp(table[index].name, name)) return (uintptr_t)table[index].address;
    return 0;
}

static inline const char *rudroid_symbol_name(const RudroidSymbol *table, size_t count, size_t index) {
    return index < count ? table[index].name : NULL;
}

// Lookup and enumeration come from the same table, so the registry's
// duplicate checks see exactly the names the lookup answers.
#define RUDROID_SYMBOL_PROVIDER(provider, table) \
    uintptr_t rudroid_bionic_##provider##_symbol(const char *name) { \
        return rudroid_symbol_find(table, RUDROID_SYMBOL_COUNT(table), name); \
    } \
    const char *rudroid_bionic_##provider##_symbol_name(size_t index) { \
        return rudroid_symbol_name(table, RUDROID_SYMBOL_COUNT(table), index); \
    }
