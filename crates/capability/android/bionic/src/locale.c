// INPUT:  Android C/C.UTF-8 locale contracts and public GNU locale/ctype/Unicode APIs
// OUTPUT:  rudroid_bionic_locale_symbol/_symbol_name table, owned 8-byte locale facades and cached native tables
// POS:    shares native classification tables while keeping guest locale state out of host globals
#define _GNU_SOURCE
#include <ctype.h>
#include <errno.h>
#include <locale.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <wchar.h>
#include <wctype.h>
#include "symbol_table.h"

typedef struct { size_t mb_max; } GuestLocale;
static _Atomic bool global_utf8 = true; // Android starts in C.UTF-8.
static _Thread_local GuestLocale *current;
#define GLOBAL ((GuestLocale *)(intptr_t)-1)
_Static_assert(sizeof(GuestLocale) == 8 && LC_ALL_MASK == 0x1fbf, "Android ARM64 locale ABI");
// The locale_t facade is C-owned, matching Android's small public-service
// token. No native locale_t is ever interpreted as this guest object.
static size_t locale_max(GuestLocale *l) {
    return l == GLOBAL ? (atomic_load_explicit(&global_utf8, memory_order_relaxed) ? 4 : 1) : l->mb_max;
}
static GuestLocale *use_locale(GuestLocale *next) {
    GuestLocale *previous = current ? current : GLOBAL;
    if (next) current = next;
    return previous;
}
static size_t mb_max(void) { return locale_max(use_locale(NULL)); }
static int locale_kind(const char *name) {
    if (!name) return -1;
    if (!strcmp(name, "C") || !strcmp(name, "POSIX")) return 0;
    if (!*name || !strcmp(name, "C.UTF-8") || !strcmp(name, "en_US.UTF-8")) return 1;
    return -1;
}
static GuestLocale *allocate_locale(size_t maximum) {
    GuestLocale *l = malloc(sizeof(*l));
    if (l) l->mb_max = maximum;
    return l;
}
static GuestLocale *new_locale(int mask, const char *name, GuestLocale *base) {
    (void)base; // Android's two-locale implementation does not consume base.
    if ((mask & ~LC_ALL_MASK) || !name) { errno = EINVAL; return NULL; }
    int kind = locale_kind(name);
    if (kind < 0) { errno = ENOENT; return NULL; }
    return allocate_locale(kind ? 4 : 1);
}
static GuestLocale *duplicate_locale(GuestLocale *l) { return allocate_locale(locale_max(l)); }
static void free_locale(GuestLocale *l) {
    if (l != GLOBAL) free(l); // Caller must stop borrowing the token first.
}
static char *set_locale(int category, const char *name) {
    if (category < 0 || category > 12) { errno = EINVAL; return NULL; }
    if (name) {
        int kind = locale_kind(name);
        if (kind < 0) { errno = ENOENT; return NULL; }
        atomic_store_explicit(&global_utf8, kind != 0, memory_order_relaxed);
    }
    return atomic_load_explicit(&global_utf8, memory_order_relaxed) ? "C.UTF-8" : "C";
}

// Cache exactly two native table handles for the pinned plugin's lifetime.
// C.UTF-8 is optional on old GNU systems; ASCII C tables are the fallback.
// The Rust UTF-8 codec does not depend on a locale installation.
static pthread_once_t native_once = PTHREAD_ONCE_INIT;
static locale_t native_c, native_utf8;
static struct lconv conventions;
static void init_native(void) {
    native_c = newlocale(LC_ALL_MASK, "C", NULL);
    native_utf8 = newlocale(LC_CTYPE_MASK, "C.UTF-8", NULL);
    if (!native_utf8) native_utf8 = native_c;
    locale_t previous = native_c ? uselocale(native_c) : NULL;
    conventions = *localeconv();
    if (previous) uselocale(previous);
}
static locale_t native_tables(bool unicode) {
    int saved = errno;
    pthread_once(&native_once, init_native);
    errno = saved;
    return unicode ? native_utf8 : native_c;
}
// All public lconv fields have the same layout on these two ARM64 ABIs.
typedef struct {
    char *decimal_point, *thousands_sep, *grouping, *int_curr_symbol, *currency_symbol;
    char *mon_decimal_point, *mon_thousands_sep, *mon_grouping, *positive_sign, *negative_sign;
    char int_frac_digits, frac_digits, p_cs_precedes, p_sep_by_space, n_cs_precedes;
    char n_sep_by_space, p_sign_posn, n_sign_posn, int_p_cs_precedes, int_p_sep_by_space;
    char int_n_cs_precedes, int_n_sep_by_space, int_p_sign_posn, int_n_sign_posn;
} GuestLconv;
#define FIELD(f) _Static_assert(offsetof(GuestLconv, f) == offsetof(struct lconv, f) && \
    sizeof(((GuestLconv *)0)->f) == sizeof(((struct lconv *)0)->f), "lconv field ABI")
_Static_assert(sizeof(GuestLconv) == sizeof(struct lconv) &&
    _Alignof(GuestLconv) == _Alignof(struct lconv), "lconv layout ABI");
FIELD(decimal_point); FIELD(thousands_sep); FIELD(grouping); FIELD(int_curr_symbol);
FIELD(currency_symbol); FIELD(mon_decimal_point); FIELD(mon_thousands_sep); FIELD(mon_grouping);
FIELD(positive_sign); FIELD(negative_sign); FIELD(int_frac_digits); FIELD(frac_digits);
FIELD(p_cs_precedes); FIELD(p_sep_by_space); FIELD(n_cs_precedes); FIELD(n_sep_by_space);
FIELD(p_sign_posn); FIELD(n_sign_posn); FIELD(int_p_cs_precedes); FIELD(int_p_sep_by_space);
FIELD(int_n_cs_precedes); FIELD(int_n_sep_by_space); FIELD(int_p_sign_posn); FIELD(int_n_sign_posn);
#undef FIELD
static struct lconv *locale_conventions(void) {
    native_tables(false);
    return &conventions; // One stable snapshot; native localeconv cannot overwrite it.
}
// Android byte classification is ASCII in both supported locales. Reuse the
// pinned native C tables, without changing the host thread's locale or reading
// a classification table outside its unsigned-byte domain (EOF is false).
#define BYTE_CLASS(fn, fallback) \
    static int guest_##fn(int c) { \
        if (c < 0 || c > 255) return 0; \
        locale_t l = native_tables(false); \
        return l ? (fn##_l(c, l) != 0) : (fallback); \
    } \
    static int guest_##fn##_l(int c, GuestLocale *l) { (void)l; return guest_##fn(c); }
#define LOWER(c) ((c) >= 'a' && (c) <= 'z')
#define UPPER(c) ((c) >= 'A' && (c) <= 'Z')
#define DIGIT(c) ((c) >= '0' && (c) <= '9')
BYTE_CLASS(isspace, c == ' ' || (c >= '\t' && c <= '\r'))
BYTE_CLASS(isalpha, LOWER(c) || UPPER(c))
BYTE_CLASS(isalnum, LOWER(c) || UPPER(c) || DIGIT(c))
BYTE_CLASS(isdigit, DIGIT(c))
BYTE_CLASS(isxdigit, DIGIT(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))
BYTE_CLASS(islower, LOWER(c))
BYTE_CLASS(isupper, UPPER(c))
BYTE_CLASS(isprint, c >= ' ' && c <= '~')
BYTE_CLASS(isgraph, c > ' ' && c <= '~')
#undef BYTE_CLASS
static int guest_tolower(int c) { return UPPER(c) ? c + ('a' - 'A') : c; }
static int guest_toupper(int c) { return LOWER(c) ? c - ('a' - 'A') : c; }
static int guest_tolower_l(int c, GuestLocale *l) { (void)l; return guest_tolower(c); }
static int guest_toupper_l(int c, GuestLocale *l) { (void)l; return guest_toupper(c); }
#undef DIGIT
#undef UPPER
#undef LOWER
// GNU FILE chooses its codec when first oriented. Android wide I/O uses
// UTF-8 even in its C locale, so choose the cached UTF-8 tables only around
// the native call; restore the host thread's selection and the I/O errno.
#define WIDE_IO(result, name, signature, invocation) \
    result rudroid_bionic_##name signature { \
        locale_t l=native_tables(true), old=l ? uselocale(l) : NULL; \
        result value=invocation; int error=errno; \
        if (old) uselocale(old); errno=error; return value; \
    }
WIDE_IO(wint_t,fgetwc,(FILE *f),fgetwc(f))
WIDE_IO(wint_t,fputwc,(wchar_t c,FILE *f),fputwc(c,f))
WIDE_IO(wint_t,ungetwc,(wint_t c,FILE *f),ungetwc(c,f))
WIDE_IO(int,fwide,(FILE *f,int mode),fwide(f,mode))
#undef WIDE_IO
#define CLASSIFY(fn) \
    static int guest_##fn(wint_t c) { \
        locale_t l = native_tables(true); return l ? fn##_l(c, l) : fn(c); \
    } \
    static int guest_##fn##_l(wint_t c, GuestLocale *l) { (void)l; return guest_##fn(c); }
CLASSIFY(iswalnum) CLASSIFY(iswalpha) CLASSIFY(iswblank) CLASSIFY(iswcntrl)
CLASSIFY(iswdigit) CLASSIFY(iswgraph) CLASSIFY(iswlower) CLASSIFY(iswprint)
CLASSIFY(iswpunct) CLASSIFY(iswspace) CLASSIFY(iswupper) CLASSIFY(iswxdigit)
#undef CLASSIFY
#define CASE(fn) \
    static wint_t guest_##fn(wint_t c) { \
        locale_t l = native_tables(true); return l ? fn##_l(c, l) : fn(c); \
    } \
    static wint_t guest_##fn##_l(wint_t c, GuestLocale *l) { (void)l; return guest_##fn(c); }
CASE(towlower) CASE(towupper)
#undef CASE
// Android ignores locale arguments in collation and numeric/time helpers.
// Use the cached native C tables, independent of the host process locale.
static int guest_strcoll_l(const char *a, const char *b, GuestLocale *l) { (void)l; return strcmp(a,b); }
static int guest_wcscoll_l(const wchar_t *a, const wchar_t *b, GuestLocale *l) { (void)l; return wcscmp(a,b); }
static size_t guest_strxfrm(char *out, const char *in, size_t n) {
    size_t length = strlen(in);
    if (n) memcpy(out, in, length < n ? length + 1 : n);
    return length;
}
static size_t guest_wcsxfrm(wchar_t *out, const wchar_t *in, size_t n) {
    size_t length = wcslen(in);
    if (n) wmemcpy(out, in, length < n ? length + 1 : n);
    return length;
}
static size_t guest_strxfrm_l(char *out, const char *in, size_t n, GuestLocale *l) {
    (void)l; return guest_strxfrm(out,in,n);
}
static size_t guest_wcsxfrm_l(wchar_t *out, const wchar_t *in, size_t n, GuestLocale *l) {
    (void)l; return guest_wcsxfrm(out,in,n);
}
static long long guest_strtoll_l(const char *s, char **end, int base, GuestLocale *l) {
    (void)l; locale_t c = native_tables(false); return c ? strtoll_l(s,end,base,c) : strtoll(s,end,base);
}
static unsigned long long guest_strtoull_l(const char *s, char **end, int base, GuestLocale *l) {
    (void)l; locale_t c = native_tables(false); return c ? strtoull_l(s,end,base,c) : strtoull(s,end,base);
}
static long double guest_strtold_l(const char *s, char **end, GuestLocale *l) {
    (void)l; locale_t c = native_tables(false); return c ? strtold_l(s,end,c) : strtold(s,end);
}
static size_t guest_strftime_l(char *out, size_t n, const char *fmt, const struct tm *tm, GuestLocale *l) {
    (void)l; locale_t c = native_tables(false); return c ? strftime_l(out,n,fmt,tm,c) : strftime(out,n,fmt,tm);
}
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("newlocale", new_locale) RUDROID_SYMBOL("duplocale", duplicate_locale)
    RUDROID_SYMBOL("freelocale", free_locale) RUDROID_SYMBOL("uselocale", use_locale)
    RUDROID_SYMBOL("setlocale", set_locale) RUDROID_SYMBOL("localeconv", locale_conventions)
    RUDROID_SYMBOL("__ctype_get_mb_cur_max", mb_max)
#define BYTE(fn) RUDROID_SYMBOL(#fn, guest_##fn) RUDROID_SYMBOL(#fn "_l", guest_##fn##_l)
    BYTE(isspace) BYTE(isalpha) BYTE(isalnum) BYTE(isdigit) BYTE(isxdigit)
    BYTE(islower) BYTE(isupper) BYTE(isprint) BYTE(isgraph) BYTE(tolower) BYTE(toupper)
#undef BYTE
#define WIDE(fn) RUDROID_SYMBOL(#fn, guest_##fn) RUDROID_SYMBOL(#fn "_l", guest_##fn##_l)
    WIDE(iswalnum) WIDE(iswalpha) WIDE(iswblank) WIDE(iswcntrl)
    WIDE(iswdigit) WIDE(iswgraph) WIDE(iswlower) WIDE(iswprint)
    WIDE(iswpunct) WIDE(iswspace) WIDE(iswupper) WIDE(iswxdigit)
    WIDE(towlower) WIDE(towupper)
    RUDROID_SYMBOL("strcoll", strcmp) RUDROID_SYMBOL("wcscoll", wcscmp)
    RUDROID_SYMBOL("strxfrm", guest_strxfrm) RUDROID_SYMBOL("wcsxfrm", guest_wcsxfrm)
    RUDROID_SYMBOL("strcoll_l", guest_strcoll_l) RUDROID_SYMBOL("wcscoll_l", guest_wcscoll_l)
    RUDROID_SYMBOL("strxfrm_l", guest_strxfrm_l) RUDROID_SYMBOL("wcsxfrm_l", guest_wcsxfrm_l)
    RUDROID_SYMBOL("strtoll_l", guest_strtoll_l) RUDROID_SYMBOL("strtoull_l", guest_strtoull_l)
    RUDROID_SYMBOL("strtold_l", guest_strtold_l) RUDROID_SYMBOL("strftime_l", guest_strftime_l)
};
#undef WIDE
RUDROID_SYMBOL_PROVIDER(locale, symbols)
