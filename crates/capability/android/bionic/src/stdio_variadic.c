// INPUT:  Linux ARM64 stdio/stdarg ABI and Rust's guest FILE conversion
// OUTPUT:  rudroid_bionic_stdio_format_symbol/_symbol_name table of variadic addresses and native standard stream lookup
// POS:    uses compiler-managed AAPCS64 va_list; no guessed Rust variadic layout
#define _GNU_SOURCE
#include <stdio.h>
#include <stdarg.h>
#include <stdint.h>
#include <string.h>
#include <stdlib.h>
#include "symbol_table.h"

// Both supported ARM64 targets use the AAPCS64 three-pointer/two-int va_list.
_Static_assert(sizeof(va_list) == 32, "requires AAPCS64 LP64 va_list");
_Static_assert(_IOFBF == 0 && _IOLBF == 1 && _IONBF == 2, "stdio buffer modes");
extern FILE *rudroid_bionic_stdio_native(void *guest);
extern void *rudroid_bionic_stdio_standard(unsigned index);

FILE *rudroid_bionic_host_standard(unsigned index) {
    return index == 0 ? stdin : index == 1 ? stdout : stderr;
}

static int bridge_vfprintf(void *guest, const char *format, va_list args) {
    FILE *host = rudroid_bionic_stdio_native(guest);
    return host ? vfprintf(host, format, args) : -1;
}
static int bridge_fprintf(void *guest, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = bridge_vfprintf(guest, format, args);
    va_end(args);
    return result;
}
static int bridge_vprintf(const char *format, va_list args) {
    return bridge_vfprintf(rudroid_bionic_stdio_standard(1), format, args);
}
static int bridge_printf(const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = bridge_vprintf(format, args);
    va_end(args);
    return result;
}
static int bridge_vfscanf(void *guest, const char *format, va_list args) {
    FILE *host = rudroid_bionic_stdio_native(guest);
    return host ? vfscanf(host, format, args) : EOF;
}
static int bridge_fscanf(void *guest, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = bridge_vfscanf(guest, format, args);
    va_end(args);
    return result;
}

// Android FORTIFY calls preserve their compiler-provided object bounds.
// AAPCS64 va_list stays entirely in this compiler-managed C boundary.
static size_t bridge_strlen_chk(const char *s, size_t size) {
    size_t length = strnlen(s, size);
    if (length == size) abort();
    return length;
}
static int bridge_vsnprintf_chk(char *out, size_t size, int flags,
        size_t capacity, const char *format, va_list args) {
    (void)flags;
    if (size > capacity) abort();
    return vsnprintf(out, size, format, args);
}
static int bridge_snprintf_chk(char *out, size_t size, int flags,
        size_t capacity, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = bridge_vsnprintf_chk(out, size, flags, capacity, format, args);
    va_end(args);
    return result;
}

static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("fprintf", bridge_fprintf)
    RUDROID_SYMBOL("vfprintf", bridge_vfprintf)
    RUDROID_SYMBOL("printf", bridge_printf)
    RUDROID_SYMBOL("vprintf", bridge_vprintf)
    RUDROID_SYMBOL("fscanf", bridge_fscanf)
    RUDROID_SYMBOL("vfscanf", bridge_vfscanf)
    // No FILE crosses these native interfaces. Allocated asprintf strings
    // use the same C heap as the allocation-family provider.
    RUDROID_SYMBOL("snprintf", snprintf)
    RUDROID_SYMBOL("vsnprintf", vsnprintf)
    RUDROID_SYMBOL("sscanf", sscanf)
    RUDROID_SYMBOL("vsscanf", vsscanf)
    RUDROID_SYMBOL("asprintf", asprintf)
    RUDROID_SYMBOL("vasprintf", vasprintf)
    RUDROID_SYMBOL("__strlen_chk", bridge_strlen_chk)
    RUDROID_SYMBOL("__vsnprintf_chk", bridge_vsnprintf_chk)
    RUDROID_SYMBOL("__snprintf_chk", bridge_snprintf_chk)
};
RUDROID_SYMBOL_PROVIDER(stdio_format, symbols)
