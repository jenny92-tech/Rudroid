// INPUT:  Android 32-word jump buffers and versioned setjmp/signal mask interfaces
// OUTPUT:  rudroid_jump_probe and rudroid_jump_corrupt_probe for real guest C-frame recovery
// POS:    tests returns-twice in C, never across a Rust-owned frame
extern int setjmp(long *) __attribute__((returns_twice));
extern int _setjmp(long *) __attribute__((returns_twice));
extern int sigsetjmp(long *,int) __attribute__((returns_twice));
extern void longjmp(long *,int) __attribute__((noreturn));
extern void _longjmp(long *,int) __attribute__((noreturn));
extern void siglongjmp(long *,int) __attribute__((noreturn));
extern int sigprocmask(int,const unsigned long *,unsigned long *);
extern int sigismember(const unsigned long *,int);
extern int rudroid_jump_register_probe(void);
static volatile unsigned observed;
__attribute__((noinline,noreturn)) static void nested(long *buffer,int value,unsigned depth) {
    volatile unsigned stack[32];
    for (unsigned i=0;i<32;++i) stack[i]=depth+i;
    observed+=stack[depth%32];
    if (depth) nested(buffer,value,depth-1);
    longjmp(buffer,value);
}
int rudroid_jump_probe(void) {
    struct { long before,buffer[32],after; } data={101,{0},202};
    for (unsigned i=25;i<32;++i) data.buffer[i]=303;
    volatile int returned=setjmp(data.buffer);
    if (!returned) nested(data.buffer,17,8);
    if (returned!=17 || !observed || data.before!=101 || data.after!=202) return 1;
    for (unsigned i=25;i<32;++i) if (data.buffer[i]!=303) return 2;
    returned=_setjmp(data.buffer);
    if (!returned) _longjmp(data.buffer,0);
    if (returned!=1) return 3;
    returned=sigsetjmp(data.buffer,0);
    if (!returned) siglongjmp(data.buffer,-23);
    if (returned!=-23) return 4;
    unsigned long mask=1ul<<9,original,current=0;
    if (sigprocmask(0,&mask,&original)) return 5;
    returned=sigsetjmp(data.buffer,1);
    if (!returned) { sigprocmask(1,&mask,0); siglongjmp(data.buffer,7); }
    if (returned!=7 || sigprocmask(0,0,&current) || sigismember(&current,10)!=1) return 6;
    returned=setjmp(data.buffer);
    if (!returned) { sigprocmask(1,&mask,0); longjmp(data.buffer,8); }
    if (returned!=8 || sigprocmask(0,0,&current) || sigismember(&current,10)!=1) return 7;
    returned=_setjmp(data.buffer);
    if (!returned) { sigprocmask(1,&mask,0); _longjmp(data.buffer,9); }
    if (returned!=9 || sigprocmask(0,0,&current) || sigismember(&current,10)!=0) return 8;
    if (sigprocmask(2,&original,0)) return 9;
    return rudroid_jump_register_probe();
}
void rudroid_jump_corrupt_probe(void) {
    long buffer[32];
    if (!setjmp(buffer)) { buffer[2]^=4; longjmp(buffer,1); }
}
void rudroid_jump_cookie_corrupt_probe(void) {
    unsigned long buffer[32];
    if (!setjmp((long *)buffer)) {
        buffer[0]^=2;
        unsigned long checksum=0;
        for (unsigned i=0;i<24;++i) checksum^=buffer[i];
        buffer[24]=checksum;
        longjmp((long *)buffer,1);
    }
}
