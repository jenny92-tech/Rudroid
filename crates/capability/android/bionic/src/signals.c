// INPUT:  Android ARM64 8-byte masks/32-byte sigaction and native GNU signal APIs
// OUTPUT:  rudroid_bionic_signal_symbol/_symbol_name table with explicit mask and disposition conversion
// POS:    keeps kernel/native signal state authoritative without a second callback registry
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <ucontext.h>
#include <unistd.h>
#include "symbol_table.h"
typedef uint64_t GuestMask;
typedef union { void (*handler)(int); void (*action)(int,siginfo_t *,void *); } Handler;
typedef struct { int flags; Handler callback; GuestMask mask; void (*restorer)(void); } GuestAction;
_Static_assert(sizeof(GuestAction)==32 && offsetof(GuestAction,callback)==8 &&
    offsetof(GuestAction,mask)==16 && offsetof(GuestAction,restorer)==24, "Android LP64 sigaction ABI");
_Static_assert(sizeof(stack_t)==24 && offsetof(stack_t,ss_sp)==0 && offsetof(stack_t,ss_flags)==8 &&
    offsetof(stack_t,ss_size)==16 && sizeof(siginfo_t)==128 && offsetof(siginfo_t,si_signo)==0 &&
    offsetof(siginfo_t,si_errno)==4 && offsetof(siginfo_t,si_code)==8 && offsetof(siginfo_t,si_addr)==16 &&
    sizeof(ucontext_t)==4560 && offsetof(ucontext_t,uc_stack)==16 &&
    offsetof(ucontext_t,uc_sigmask)==40 && offsetof(ucontext_t,uc_mcontext)==176,
    "ARM64 signal stack/info/context public ABI");
_Static_assert(SIG_BLOCK==0 && SIG_UNBLOCK==1 && SIG_SETMASK==2 && SA_SIGINFO==4 &&
    SA_ONSTACK==0x08000000 && SA_RESTART==0x10000000 && SA_NODEFER==0x40000000 &&
    (unsigned)SA_RESETHAND==0x80000000u, "Android/native signal flags");
static int empty(GuestMask *set) { *set=0; return 0; }
static int fill(GuestMask *set) { *set=UINT64_MAX; return 0; }
static int valid(int signal) { if (signal<=0 || signal>64) { errno=EINVAL; return 0; } return 1; }
static int add(GuestMask *set,int signal) { if (!valid(signal)) return -1; *set|=UINT64_C(1)<<(signal-1); return 0; }
static int remove_signal(GuestMask *set,int signal) { if (!valid(signal)) return -1; *set&=~(UINT64_C(1)<<(signal-1)); return 0; }
static int member(const GuestMask *set,int signal) { if (!valid(signal)) return -1; return !!(*set&(UINT64_C(1)<<(signal-1))); }
static void native_mask(GuestMask guest,sigset_t *native) {
    int saved=errno;
    sigemptyset(native);
    // GNU reserves 32/33 for NPTL. They cannot be repurposed as guest signals.
    for (int signal=1;signal<=64;++signal)
        if (signal!=32 && signal!=33 && (guest&(UINT64_C(1)<<(signal-1)))) sigaddset(native,signal);
    errno=saved;
}
static GuestMask guest_mask(const sigset_t *native) {
    int saved=errno; GuestMask mask=0;
    for (int signal=1;signal<=64;++signal)
        if (signal!=32 && signal!=33 && sigismember(native,signal)==1) mask|=UINT64_C(1)<<(signal-1);
    errno=saved; return mask;
}
// Typed private jump-buffer helpers share this facade's mask conversions.
void rudroid_bionic_jump_mask_save(uint64_t *buffer) {
    if (!(buffer[0]&1)) return;
    int saved=errno; sigset_t native;
    if (sigprocmask(SIG_BLOCK,NULL,&native)==0) buffer[1]=guest_mask(&native);
    else buffer[0]&=~UINT64_C(1);
    errno=saved;
}
void rudroid_bionic_jump_mask_restore(const uint64_t *buffer) {
    if (!(buffer[0]&1)) return;
    int saved=errno; sigset_t native; native_mask(buffer[1],&native);
    sigprocmask(SIG_SETMASK,&native,NULL); errno=saved;
}
static int disposition(int signal,const GuestAction *new_action,GuestAction *old_action) {
    if (signal==32 || signal==33) { errno=EINVAL; return -1; }
    struct sigaction next={0},old;
    if (new_action) {
        next.sa_flags=new_action->flags;
        if (next.sa_flags&SA_SIGINFO) next.sa_sigaction=new_action->callback.action;
        else next.sa_handler=new_action->callback.handler;
        next.sa_restorer=new_action->restorer;
        native_mask(new_action->mask,&next.sa_mask);
    }
    int result=sigaction(signal,new_action ? &next : NULL,old_action ? &old : NULL);
    if (result==0 && old_action) {
        old_action->flags=old.sa_flags;
        if (old.sa_flags&SA_SIGINFO) old_action->callback.action=old.sa_sigaction;
        else old_action->callback.handler=old.sa_handler;
        old_action->mask=guest_mask(&old.sa_mask); old_action->restorer=old.sa_restorer;
    }
    return result;
}
static int mask_process(int how,const GuestMask *next,GuestMask *old) {
    sigset_t native,previous;
    if (next) native_mask(*next,&native);
    int result=sigprocmask(how,next ? &native : NULL,old ? &previous : NULL);
    if (result==0 && old) *old=guest_mask(&previous);
    return result;
}
static int pending(GuestMask *result) {
    sigset_t native; int status=sigpending(&native);
    if (status==0) *result=guest_mask(&native);
    return status;
}
static int suspend_mask(const GuestMask *mask) { sigset_t native; native_mask(*mask,&native); return sigsuspend(&native); }
static int wait_signal(const GuestMask *mask,int *result) { sigset_t native; native_mask(*mask,&native); return sigwait(&native,result); }
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("sigemptyset", empty) RUDROID_SYMBOL("sigfillset", fill) RUDROID_SYMBOL("sigaddset", add)
    RUDROID_SYMBOL("sigdelset", remove_signal) RUDROID_SYMBOL("sigismember", member) RUDROID_SYMBOL("sigaction", disposition)
    RUDROID_SYMBOL("sigprocmask", mask_process) RUDROID_SYMBOL("sigpending", pending)
    RUDROID_SYMBOL("sigsuspend", suspend_mask) RUDROID_SYMBOL("sigwait", wait_signal)
    RUDROID_SYMBOL("sigaltstack", sigaltstack) RUDROID_SYMBOL("kill", kill) RUDROID_SYMBOL("raise", raise) RUDROID_SYMBOL("signal", signal)
};
RUDROID_SYMBOL_PROVIDER(signal, symbols)
