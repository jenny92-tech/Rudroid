// INPUT:  Linux ARM64 LP64 atomics/futex, native malloc/free and errno/write
// OUTPUT:  exact libstdc++.so@LIBC_O C++ allocation, guard and pure-virtual ABI entries
// POS:    a small AOSP-compatible C boundary; no C++ exception runtime or host libstdc++ rename
//
// ABI references (Apache-2.0): AOSP bionic libc/bionic/new.cpp,
// __cxa_guard.cpp and __cxa_pure_virtual.cpp. This is an independent bridge
// with the same observable LP64 contract.
#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <linux/futex.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
#include "symbol_table.h"

_Static_assert(sizeof(void *) == 8 && sizeof(size_t) == 8, "requires Android LP64");

static void report(const char *text) {
    size_t length = strlen(text);
    while (length) {
        ssize_t written = write(STDERR_FILENO, text, length);
        if (written > 0) {
            text += written;
            length -= (size_t)written;
        } else if (written < 0 && errno == EINTR) {
            continue;
        } else {
            break;
        }
    }
}
static __attribute__((noreturn)) void fatal(const char *text) {
    report(text);
    abort();
}

static void *cxx_new(size_t size, const char *failure) {
    void *allocation = malloc(size);
    if (!allocation) fatal(failure);
    return allocation;
}
static void *cxx_new_scalar(size_t size) { return cxx_new(size, "new failed to allocate\n"); }
static void *cxx_new_array(size_t size) { return cxx_new(size, "new[] failed to allocate\n"); }
static void cxx_delete(void *allocation) { free(allocation); }
static void cxx_delete_array(void *allocation) { free(allocation); }

// ARM64 uses the Itanium eight-byte guard with an initialized low byte. The
// other 24 bits carry the AOSP waiting states and must remain untouched while
// construction is underway.
#define GUARD_COMPLETE 1
#define GUARD_UNDER_WAY 0x100
#define GUARD_WAITING 0x200
typedef union {
    _Atomic int state;
    int64_t aligner;
} Guard;
_Static_assert(sizeof(Guard) == 8 && _Alignof(Guard) == 8, "Android ARM64 guard ABI");
_Static_assert(sizeof(_Atomic int) == 4 && _Alignof(_Atomic int) == 4, "32-bit guard state");
_Static_assert(offsetof(Guard, state) == 0, "guard state starts at the initialized byte");

static void guard_wait(Guard *guard) {
    while (syscall(SYS_futex, &guard->state, FUTEX_WAIT_PRIVATE, GUARD_WAITING,
                   NULL, NULL, 0) == -1 && errno == EINTR) {
    }
}
static void guard_wake(Guard *guard) {
    (void)syscall(SYS_futex, &guard->state, FUTEX_WAKE_PRIVATE, INT_MAX,
                  NULL, NULL, 0);
}

static int guard_acquire(Guard *guard) {
    int old = __atomic_load_n(&guard->state, __ATOMIC_ACQUIRE);
    for (;;) {
        if (old == GUARD_COMPLETE) return 0;
        if (old == 0) {
            if (__atomic_compare_exchange_n(&guard->state, &old, GUARD_UNDER_WAY,
                                            1, __ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE)) {
                return 1;
            }
            continue;
        }
        if (old == GUARD_UNDER_WAY) {
            if (!__atomic_compare_exchange_n(&guard->state, &old, GUARD_WAITING,
                                             1, __ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE)) {
                continue;
            }
        }
        guard_wait(guard);
        old = __atomic_load_n(&guard->state, __ATOMIC_ACQUIRE);
    }
}

static void guard_release(Guard *guard) {
    int old = __atomic_exchange_n(&guard->state, GUARD_COMPLETE, __ATOMIC_RELEASE);
    if (old == GUARD_WAITING) guard_wake(guard);
}

static void guard_abort(Guard *guard) {
    int old = __atomic_exchange_n(&guard->state, 0, __ATOMIC_RELEASE);
    if (old == GUARD_WAITING) guard_wake(guard);
}

static __attribute__((noreturn)) void pure_virtual(void) {
    fatal("Pure virtual function called. Are you calling virtual methods from a destructor?\n");
}

// Only the seven legacy allocation/guard/pure-virtual imports plus the guard
// abort pairing are visible. A bare query, libc.so, or any version other than
// libstdc++.so@LIBC_O is rejected by registry.rs.
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("_Znwm", cxx_new_scalar)
    RUDROID_SYMBOL("_Znam", cxx_new_array)
    RUDROID_SYMBOL("_ZdlPv", cxx_delete)
    RUDROID_SYMBOL("_ZdaPv", cxx_delete_array)
    RUDROID_SYMBOL("__cxa_guard_acquire", guard_acquire)
    RUDROID_SYMBOL("__cxa_guard_release", guard_release)
    RUDROID_SYMBOL("__cxa_guard_abort", guard_abort)
    RUDROID_SYMBOL("__cxa_pure_virtual", pure_virtual)
};
RUDROID_SYMBOL_PROVIDER(libstdcxx, symbols)
