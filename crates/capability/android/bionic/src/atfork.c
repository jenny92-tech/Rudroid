// INPUT:  guest callback/DSO registrations, native fork and quiescent image cleanup
// OUTPUT:  rudroid_bionic_atfork_symbol/_symbol_name table and scoped callback retirement
// POS:    guest fork uses native process creation with one guest-owned callback ledger
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "symbol_table.h"

typedef void (*Callback)(void);
typedef struct { Callback prepare,parent,child; void *dso; } Entry;
static pthread_mutex_t mutex=PTHREAD_MUTEX_INITIALIZER;
static Entry *entries;
static size_t length,capacity;
static int register_atfork(Callback prepare,Callback parent,Callback child,void *dso) {
    if (!prepare && !parent && !child) return 0;
    pthread_mutex_lock(&mutex);
    if (length==capacity) {
        size_t next=capacity ? capacity*2 : 4;
        if (next<capacity || next>SIZE_MAX/sizeof(Entry)) {
            pthread_mutex_unlock(&mutex); return ENOMEM;
        }
        Entry *grown=realloc(entries,next*sizeof(Entry));
        if (!grown) { pthread_mutex_unlock(&mutex); return ENOMEM; }
        entries=grown; capacity=next;
    }
    entries[length++]=(Entry){prepare,parent,child,dso};
    pthread_mutex_unlock(&mutex);
    return 0;
}
static int atfork(Callback prepare,Callback parent,Callback child) {
    return register_atfork(prepare,parent,child,NULL);
}
static pid_t guest_fork(void) {
    // Snapshot before any callback. Allocation failure is an actual failed
    // fork, not a skipped prepare phase. Guest callbacks may re-register.
    pthread_mutex_lock(&mutex);
    size_t count=length;
    Entry *snapshot=count ? malloc(count*sizeof(Entry)) : NULL;
    if (count && !snapshot) {
        pthread_mutex_unlock(&mutex); errno=ENOMEM; return -1;
    }
    if (count) memcpy(snapshot,entries,count*sizeof(Entry));
    pthread_mutex_unlock(&mutex);
    for (size_t i=count;i>0;--i) if (snapshot[i-1].prepare) snapshot[i-1].prepare();
    // Keep our ledger mutex out of a vanished thread's ownership in the
    // child. No guest callback is invoked under it; native fork executes
    // the host's own pthread_atfork hooks and resets its allocator locks.
    pthread_mutex_lock(&mutex);
    pid_t result=fork();
    int error=errno;
    pthread_mutex_unlock(&mutex);
    for (size_t i=0;i<count;++i) {
        Callback callback=result==0 ? snapshot[i].child : snapshot[i].parent;
        if (callback) callback();
    }
    free(snapshot);
    errno=error;
    return result;
}
static int in_range(uintptr_t address,uintptr_t start,size_t size) {
    return address>=start && address-start<size;
}
static void retire(void *dso,uintptr_t start,size_t size,int range) {
    // Same quiescence requirement as executable image teardown: no
    // concurrent fork/callback may still execute code from this range.
    pthread_mutex_lock(&mutex);
    size_t retained=0;
    for (size_t i=0;i<length;++i) {
        Entry entry=entries[i];
        if ((!range && entry.dso==dso) || (range && (in_range((uintptr_t)entry.dso,start,size) ||
            in_range((uintptr_t)entry.prepare,start,size) ||
            in_range((uintptr_t)entry.parent,start,size) ||
            in_range((uintptr_t)entry.child,start,size)))) continue;
        entries[retained++]=entry;
    }
    length=retained;
    if (!length) { free(entries); entries=NULL; capacity=0; }
    pthread_mutex_unlock(&mutex);
}
void rudroid_bionic_atfork_finalize_range(uintptr_t start,size_t size) { retire(NULL,start,size,1); }
void rudroid_bionic_atfork_finalize_dso(void *dso) { if (dso) retire(dso,0,0,0); }
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("__register_atfork", register_atfork)
    RUDROID_SYMBOL("pthread_atfork", atfork)
    RUDROID_SYMBOL("fork", guest_fork)
};
RUDROID_SYMBOL_PROVIDER(atfork, symbols)
