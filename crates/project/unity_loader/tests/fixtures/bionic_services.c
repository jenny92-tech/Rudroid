// INPUT:  versioned Android semaphore, socket/DNS and public POSIX interfaces
// OUTPUT:  rudroid_services_probe native wakeup, socket I/O, DNS fields and canary checks
// POS:    exercises production services through actual ARM64 guest imports
extern int *__errno(void);
extern int sigemptyset(unsigned long *),sigaddset(unsigned long *,int),sigismember(const unsigned long *,int);
extern int sigprocmask(int,const unsigned long *,unsigned long *),raise(int);
typedef struct { int flags; void (*handler)(int,void *,void *); unsigned long mask; void (*restorer)(void); } Action;
extern int sigaction(int,const Action *,Action *),sigaltstack(const void *,void *);
static volatile int signal_seen;
static void received(int signal,void *info,void *context) {
    if (signal==10 && info && context && *(int *)info==10) ++signal_seen;
}
extern int __android_log_print(int,const char *,const char *,...);
extern int sem_init(void *,int,unsigned),sem_destroy(void *),sem_post(void *),sem_wait(void *);
extern int sem_trywait(void *),sem_getvalue(void *,int *),sem_timedwait(void *,const void *);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_join(unsigned long,void **);
extern int socketpair(int,int,int,int *),close(int),fcntl(int,int,...);
extern long send(int,const void *,unsigned long,int),recv(int,void *,unsigned long,int);
typedef struct { void *base; unsigned long length; } Iovec;
typedef struct { void *name; unsigned length; Iovec *vectors; unsigned long count; void *control; unsigned long controls; int flags; } Message;
extern int getpid(void),prctl(int,...);
extern long ptrace(int,int,void *,void *);
extern long process_vm_readv(int,const Iovec *,unsigned long,const Iovec *,unsigned long,unsigned long);
extern long sendmsg(int,const Message *,int),recvmsg(int,Message *,int);
extern int select(int,void *,void *,void *,void *),__FD_ISSET_chk(int,const void *,unsigned long);
extern void __FD_SET_chk(int,void *,unsigned long);
typedef struct Address { int flags,family,type,protocol; unsigned length; char *canonical; void *address; struct Address *next; } Address;
extern int getaddrinfo(const char *,const char *,const Address *,Address **);
extern void freeaddrinfo(Address *);
extern int getnameinfo(const void *,unsigned,char *,unsigned,char *,unsigned,int);
extern int inet_pton(int,const char *,void *);
extern const char *inet_ntop(int,const void *,char *,unsigned);
extern int strcmp(const char *,const char *),statfs(const char *,void *),uname(void *);
typedef struct { long quotient,remainder; } Division;
extern Division ldiv(long,long);
struct __attribute__((aligned(8))) Semaphore { unsigned before,words[4],after; };
static void *wake(void *p) { return (void *)(long)(sem_wait(p)!=0); }
int rudroid_services_probe(void) {
    struct { Action action; unsigned long canary; } previous={{0,0,0,0},456};
    Action action={4,received,0,0};
    unsigned long mask[2]={0,345},old[2]={0,678};
    if (sigemptyset(mask) || sigaddset(mask,10) || sigismember(mask,10)!=1 || mask[1]!=345) return 24;
    if (sigprocmask(0,mask,old) || old[1]!=678 || sigprocmask(2,old,0)) return 25;
    if (sigaction(10,&action,&previous.action) || previous.canary!=456) return 26;
    signal_seen=0;
    int status=raise(10);
    int restored=sigaction(10,&previous.action,0);
    if (status || restored || signal_seen!=1) return 27;
    struct { void *pointer; int flags; unsigned long size; unsigned long canary; } stack={0,0,0,789};
    if (sigaltstack(0,&stack) || stack.canary!=789) return 28;
    struct Semaphore object={11,{0,0,0,0},22}; void *sem=object.words;
    if (sem_init(sem,0,0)) return 1;
    int value=-1;
    if (sem_getvalue(sem,&value) || value || sem_trywait(sem)!=-1 || *__errno()!=11) return 2;
    long deadline[2]={0,0};
    if (sem_timedwait(sem,deadline)!=-1 || *__errno()!=110) return 3;
    unsigned long worker; void *result=(void *)1;
    if (pthread_create(&worker,0,wake,sem) || sem_post(sem) || pthread_join(worker,&result) || result) return 4;
    if (sem_destroy(sem) || object.before!=11 || object.after!=22) return 5;
    if (sem_init(sem,1,0)!=-1 || *__errno()!=95) return 6;
    int fd[2]; char bytes[16]={0};
    if (socketpair(1,1,0,fd) || send(fd[0],"native",6,0)!=6 || recv(fd[1],bytes,6,0)!=6 || strcmp(bytes,"native")) return 7;
    if (fcntl(fd[0],2,1) || !(fcntl(fd[0],1)&1)) return 8;
    Iovec vector={(void *)"shared",6}; Message message={0,0,&vector,1,0,0,0};
    if (sendmsg(fd[1],&message,0)!=6) return 9;
    unsigned long set[16]={0}; long timeout[2]={0,0};
    __FD_SET_chk(fd[0],set,sizeof(set));
    if (select(fd[0]+1,set,0,0,timeout)!=1 || !__FD_ISSET_chk(fd[0],set,sizeof(set))) return 10;
    vector.base=bytes;
    if (recvmsg(fd[0],&message,0)!=6 || strcmp(bytes,"shared")) return 11;
    if (close(fd[0]) || close(fd[1])) return 12;
    unsigned char ip[4]; char text[16];
    if (inet_pton(2,"127.0.0.1",ip)!=1 || ip[0]!=127 || !inet_ntop(2,ip,text,sizeof(text)) || strcmp(text,"127.0.0.1")) return 13;
    Address hints={12,2,1,0,0,0,0,0},*addresses=0;
    if (getaddrinfo("127.0.0.1","4567",&hints,&addresses) || !addresses) return 14;
    for (Address *p=addresses;p;p=p->next) {
        if (p->family!=2 || p->type!=1 || p->length!=16 || !p->address) return 15;
        char host[32],service[8];
        if (getnameinfo(p->address,p->length,host,sizeof(host),service,sizeof(service),10) ||
            strcmp(host,"127.0.0.1") || strcmp(service,"4567")) return 23;
        unsigned char *raw=p->address;
        if (raw[4]!=127 || raw[5]!=0 || raw[6]!=0 || raw[7]!=1) return 16;
    }
    freeaddrinfo(addresses);
    hints.flags=0x200;
    if (getaddrinfo("127.0.0.1","4567",&hints,&addresses)!=3 || addresses) return 17;
    hints.flags=12;
    if (getaddrinfo("invalid","4567",&hints,&addresses)!=8 || addresses) return 18;
    struct { unsigned long fields[15],canary; } filesystem={{0},123};
    if (statfs("/tmp",&filesystem) || !filesystem.fields[1] || filesystem.canary!=123) return 19;
    struct { char fields[390]; char canary[8]; } system={{0},{1,2,3,4,5,6,7,8}};
    if (uname(&system) || strcmp(system.fields,"Linux") || system.canary[0]!=1 || system.canary[7]!=8) return 20;
    Division d=ldiv(-17,5);
    if (d.quotient!=-3 || d.remainder!=-2) return 21;
    char name[16];
    if (prctl(16,name,0ul,0ul,0ul) || !name[0] || prctl(3,0ul,0ul,0ul,0ul)<0) return 29;
    int source=12347,destination=0;
    Iovec local={&destination,sizeof(destination)},remote={&source,sizeof(source)};
    if (process_vm_readv(getpid(),&local,1,&remote,1,0)!=sizeof(source) || destination!=source) return 30;
    if (ptrace(2,getpid(),&source,0)!=-1 || (*__errno()!=1 && *__errno()!=3)) return 31;
    if (__android_log_print(4,"RUDROID_GUEST","native log %d %.2f",7,1.25)<=0) return 22;
    return 0;
}
