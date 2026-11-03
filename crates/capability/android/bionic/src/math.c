// INPUT:  public GNU libm prototypes and Android ARM64 float/double/FP128 ABI
// OUTPUT:  rudroid_bionic_math_symbol/_symbol_name table explicit numeric function addresses
// POS:    shares native libm code without wrapper allocation or a second math runtime
#define _GNU_SOURCE
#include <float.h>
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <math.h>
#include <stdint.h>
#include <string.h>
#include "symbol_table.h"
_Static_assert(sizeof(float)==4 && sizeof(double)==8 && sizeof(long double)==16 &&
    LDBL_MANT_DIG==113, "Android ARM64 floating-point ABI");
// GNU made these faster implementations the default in 2.29. Keep direct
// linking on ARM64's public 2.17 ABI and select the newer public versions only
// when this host provides them. One cold cache, no per-math-call branching.
extern double legacy_exp(double) __attribute__((visibility("default")));
extern double legacy_exp2(double) __attribute__((visibility("default")));
extern double legacy_log(double) __attribute__((visibility("default")));
extern double legacy_log2(double) __attribute__((visibility("default")));
extern double legacy_pow(double,double) __attribute__((visibility("default")));
__asm__(".symver legacy_exp,exp@GLIBC_2.17");
__asm__(".symver legacy_exp2,exp2@GLIBC_2.17");
__asm__(".symver legacy_log,log@GLIBC_2.17");
__asm__(".symver legacy_log2,log2@GLIBC_2.17");
__asm__(".symver legacy_pow,pow@GLIBC_2.17");
static const char *const names[]={"exp","exp2","log","log2","pow"};
static uintptr_t selected[5];
static pthread_once_t selection=PTHREAD_ONCE_INIT;
static void select_math(void) {
    const uintptr_t legacy[]={(uintptr_t)legacy_exp,(uintptr_t)legacy_exp2,
        (uintptr_t)legacy_log,(uintptr_t)legacy_log2,(uintptr_t)legacy_pow};
    for (unsigned i=0;i<5;++i) {
        void *modern=dlvsym(RTLD_DEFAULT,names[i],"GLIBC_2.29");
        selected[i]=modern ? (uintptr_t)modern : legacy[i];
    }
    // Clear a missing optional version's native loader error.
    dlerror();
}
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL_SAME(acos)
    RUDROID_SYMBOL_SAME(acosf)
    RUDROID_SYMBOL_SAME(acosh)
    RUDROID_SYMBOL_SAME(asin)
    RUDROID_SYMBOL_SAME(asinf)
    RUDROID_SYMBOL_SAME(asinh)
    RUDROID_SYMBOL_SAME(atan)
    RUDROID_SYMBOL_SAME(atanf)
    RUDROID_SYMBOL_SAME(atanh)
    RUDROID_SYMBOL_SAME(atan2)
    RUDROID_SYMBOL_SAME(atan2f)
    RUDROID_SYMBOL_SAME(cbrt)
    RUDROID_SYMBOL_SAME(cbrtf)
    RUDROID_SYMBOL_SAME(cos)
    RUDROID_SYMBOL_SAME(cosf)
    RUDROID_SYMBOL_SAME(cosh)
    RUDROID_SYMBOL_SAME(coshf)
    RUDROID_SYMBOL_SAME(expf)
    RUDROID_SYMBOL_SAME(exp2f)
    RUDROID_SYMBOL_SAME(fmod)
    RUDROID_SYMBOL_SAME(fmodf)
    RUDROID_SYMBOL_SAME(remainderf)
    RUDROID_SYMBOL_SAME(hypot)
    RUDROID_SYMBOL_SAME(hypotf)
    RUDROID_SYMBOL_SAME(ldexp)
    RUDROID_SYMBOL_SAME(ldexpf)
    RUDROID_SYMBOL_SAME(logf)
    RUDROID_SYMBOL_SAME(log10)
    RUDROID_SYMBOL_SAME(log10f)
    RUDROID_SYMBOL_SAME(log2f)
    RUDROID_SYMBOL_SAME(logb)
    RUDROID_SYMBOL_SAME(logbf)
    RUDROID_SYMBOL_SAME(modf)
    RUDROID_SYMBOL_SAME(modff)
    RUDROID_SYMBOL_SAME(nearbyint)
    RUDROID_SYMBOL_SAME(nearbyintf)
    RUDROID_SYMBOL_SAME(nextafter)
    RUDROID_SYMBOL_SAME(nextafterf)
    RUDROID_SYMBOL_SAME(nexttoward)
    RUDROID_SYMBOL_SAME(nexttowardf)
    RUDROID_SYMBOL_SAME(powf)
    RUDROID_SYMBOL_SAME(scalbn)
    RUDROID_SYMBOL_SAME(scalbnf)
    RUDROID_SYMBOL_SAME(sin)
    RUDROID_SYMBOL_SAME(sinf)
    RUDROID_SYMBOL_SAME(sincos)
    RUDROID_SYMBOL_SAME(sincosf)
    RUDROID_SYMBOL_SAME(sinh)
    RUDROID_SYMBOL_SAME(sinhf)
    RUDROID_SYMBOL_SAME(sqrt)
    RUDROID_SYMBOL_SAME(sqrtf)
    RUDROID_SYMBOL_SAME(tan)
    RUDROID_SYMBOL_SAME(tanf)
    RUDROID_SYMBOL_SAME(tanh)
    RUDROID_SYMBOL_SAME(tanhf)
    RUDROID_SYMBOL_SAME(ceil)
    RUDROID_SYMBOL_SAME(ceilf)
    RUDROID_SYMBOL_SAME(floor)
    RUDROID_SYMBOL_SAME(floorf)
    RUDROID_SYMBOL_SAME(round)
    RUDROID_SYMBOL_SAME(roundf)
    RUDROID_SYMBOL_SAME(trunc)
    RUDROID_SYMBOL_SAME(truncf)
    RUDROID_SYMBOL_SAME(fabs)
    RUDROID_SYMBOL_SAME(fabsf)
    RUDROID_SYMBOL_SAME(copysign)
    RUDROID_SYMBOL_SAME(copysignf)
    RUDROID_SYMBOL_SAME(frexp)
    RUDROID_SYMBOL_SAME(frexpf)
};
uintptr_t rudroid_bionic_math_symbol(const char *name) {
    for (unsigned i=0;i<5;++i) if (!strcmp(name,names[i])) {
        int saved=errno; pthread_once(&selection,select_math); errno=saved;
        return selected[i];
    }
    return rudroid_symbol_find(symbols, RUDROID_SYMBOL_COUNT(symbols), name);
}
// Selected names first, then the fixed table: the same set the lookup answers.
const char *rudroid_bionic_math_symbol_name(size_t index) {
    size_t selected_names = RUDROID_SYMBOL_COUNT(names);
    if (index < selected_names) return names[index];
    return rudroid_symbol_name(symbols, RUDROID_SYMBOL_COUNT(symbols), index - selected_names);
}
