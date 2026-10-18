// INPUT:  native getrandom/pthread_once and the ARM64 jump-buffer cookie contract
// OUTPUT:  rudroid_bionic_jump_symbol/_symbol_name table and private cookie validation/fatal helpers
// POS:    one 8-byte process cookie, no per-jump allocation or native jmp_buf owner
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
#include "symbol_table.h"
static _Atomic uint64_t cookie;
static pthread_once_t initialized=PTHREAD_ONCE_INIT;
__attribute__((noreturn)) void rudroid_bionic_jump_corrupt(void) {
    fputs("Bionic jump buffer validation failed\n",stderr); abort();
}
static void seed_cookie(void) {
    uint64_t random;
    do {
        size_t remaining=sizeof(random); unsigned char *next=(unsigned char *)&random;
        while (remaining) {
            long count=syscall(SYS_getrandom,next,remaining,0);
            if (count<0 && errno==EINTR) continue;
            if (count<=0) rudroid_bionic_jump_corrupt();
            next+=count; remaining-=(size_t)count;
        }
        random&=~UINT64_C(1);
    } while (!random);
    atomic_store_explicit(&cookie,random,memory_order_release);
}
uint64_t rudroid_bionic_jump_cookie(int save_mask) {
    int saved=errno;
    uint64_t value=atomic_load_explicit(&cookie,memory_order_acquire);
    if (!value) {
        pthread_once(&initialized,seed_cookie);
        value=atomic_load_explicit(&cookie,memory_order_acquire);
    }
    errno=saved; return value | (save_mask!=0);
}
void rudroid_bionic_jump_validate(uint64_t value) {
    if ((value&~UINT64_C(1))!=rudroid_bionic_jump_cookie(0)) rudroid_bionic_jump_corrupt();
}
extern int rudroid_bionic_setjmp(void *) __attribute__((returns_twice));
extern int rudroid_bionic__setjmp(void *) __attribute__((returns_twice));
extern int rudroid_bionic_sigsetjmp(void *,int) __attribute__((returns_twice));
extern void rudroid_bionic_longjmp(void *,int) __attribute__((noreturn));
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("setjmp", rudroid_bionic_setjmp) RUDROID_SYMBOL("_setjmp", rudroid_bionic__setjmp)
    RUDROID_SYMBOL("sigsetjmp", rudroid_bionic_sigsetjmp)
    RUDROID_SYMBOL("longjmp", rudroid_bionic_longjmp) RUDROID_SYMBOL("_longjmp", rudroid_bionic_longjmp)
    RUDROID_SYMBOL("siglongjmp", rudroid_bionic_longjmp)
};
RUDROID_SYMBOL_PROVIDER(jump, symbols)
