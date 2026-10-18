// INPUT:  scoped guest atfork registration and native process creation/wait
// OUTPUT:  rudroid_fork_probe verifies prepare/post order, child execution and DSO retirement
// POS:    uses the guest fork facade without publishing guest callbacks into host exit tables
typedef void (*Callback)(void);
extern int __register_atfork(Callback,Callback,Callback,void *);
extern int fork(void),waitpid(int,int *,int);
extern void _exit(int),__cxa_finalize(void *);
static int prepared,parented,childed;
static void p1(void) { prepared=prepared*10+1; }
static void p2(void) { prepared=prepared*10+2; }
static void a1(void) { parented=parented*10+1; }
static void a2(void) { parented=parented*10+2; }
static void c1(void) { childed=childed*10+1; }
static void c2(void) { childed=childed*10+2; }
int rudroid_fork_probe(void) {
    void *dso=(void *)0x123480;
    prepared=parented=childed=0;
    if (__register_atfork(p1,a1,c1,dso) || __register_atfork(p2,a2,c2,dso)) return 1;
    int pid=fork();
    if (pid<0) return 2;
    if (!pid) _exit(prepared==21 && childed==12 && parented==0 ? 0 : 71);
    int status=-1;
    if (waitpid(pid,&status,0)!=pid || status || prepared!=21 || parented!=12 || childed) return 3;
    __cxa_finalize(dso);
    prepared=parented=childed=0;
    pid=fork();
    if (pid<0) return 4;
    if (!pid) _exit(prepared==0 && childed==0 && parented==0 ? 0 : 72);
    status=-1;
    if (waitpid(pid,&status,0)!=pid || status || prepared || parented || childed) return 5;
    return 0;
}
