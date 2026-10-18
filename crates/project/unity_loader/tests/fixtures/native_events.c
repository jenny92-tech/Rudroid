// INPUT:  independently declared Android LP64 NDK/Bionic calls and production event provider
// OUTPUT:  event ABI/lifetime probe and repeated ready-fd cost samples through the same guest ABI
// POS:    exercises mapped guest callbacks and native thread ownership without a test backend
#include <stdint.h>
#include <stddef.h>
#include <stdatomic.h>
typedef struct ALooper ALooper;
typedef struct ASensor ASensor;
typedef struct ASensorManager ASensorManager;
typedef struct ASensorEventQueue ASensorEventQueue;
typedef int (*Callback)(int,int,void *);
extern ALooper *ALooper_prepare(int), *ALooper_forThread(void);
extern void ALooper_acquire(ALooper *), ALooper_release(ALooper *), ALooper_wake(ALooper *);
extern int ALooper_addFd(ALooper *,int,int,int,Callback,void *), ALooper_removeFd(ALooper *,int);
extern int ALooper_pollOnce(int,int *,int *,void **), ALooper_pollAll(int,int *,int *,void **);
extern int pipe(int *),close(int),dup2(int,int),usleep(unsigned);
extern long read(int,void *,unsigned long),write(int,const void *,unsigned long);
extern int pthread_create(long *,const void *,void *(*)(void *),void *),pthread_join(long,void **);
typedef struct { long sec,nsec; } Time;
extern int clock_gettime(int,Time *);
extern ASensorManager *ASensorManager_getInstance(void);
extern int ASensorManager_getSensorList(ASensorManager *,const ASensor *const **);
extern const ASensor *ASensorManager_getDefaultSensor(ASensorManager *,int);
extern ASensorEventQueue *ASensorManager_createEventQueue(ASensorManager *,ALooper *,int,Callback,void *);
extern int ASensorManager_destroyEventQueue(ASensorManager *,ASensorEventQueue *);
extern int ASensorEventQueue_enableSensor(ASensorEventQueue *,const ASensor *);
extern int ASensorEventQueue_disableSensor(ASensorEventQueue *,const ASensor *);
extern int ASensorEventQueue_setEventRate(ASensorEventQueue *,const ASensor *,int32_t);
extern int ASensorEventQueue_hasEvents(ASensorEventQueue *);
extern long ASensorEventQueue_getEvents(ASensorEventQueue *,void *,unsigned long);
extern const char *ASensor_getName(const ASensor *),*ASensor_getVendor(const ASensor *);
extern int ASensor_getType(const ASensor *),ASensor_getMinDelay(const ASensor *);
extern float ASensor_getResolution(const ASensor *);
#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)
static int64_t millis(void) { Time t; clock_gettime(1,&t); return t.sec*1000+t.nsec/1000000; }
typedef struct { ALooper *looper; int calls,error,write_fd; } State;
static int replacement(int fd,int events,void *data) {
    State *state=data; char byte;
    state->calls++;
    if (!(events&1) || read(fd,&byte,1)!=1) state->error=1;
    return 1;
}
static int replace_self(int fd,int events,void *data) {
    State *state=data; char byte;
    state->calls++;
    if (!(events&1) || read(fd,&byte,1)!=1 ||
        ALooper_addFd(state->looper,fd,-2,1,replacement,state)!=1) state->error=1;
    /* Removing the old callback must not remove this newly installed one. */
    return 0;
}
static int reenter(int fd,int events,void *data) {
    State *state=data; char byte;
    state->calls++;
    if (!(events&1) || read(fd,&byte,1)!=1 || ALooper_removeFd(state->looper,fd)!=1 ||
        ALooper_pollOnce(0,NULL,NULL,NULL)!=-3) state->error=1;
    return 0;
}
static int keep_ready(int fd,int events,void *data) {
    (void)fd; (void)events; ((State *)data)->calls++; return 1;
}
static void *wake_worker(void *data) {
    ALooper *looper=data;
    usleep(20000); ALooper_wake(looper); ALooper_release(looper);
    return NULL;
}
typedef struct { ALooper *looper; int fd,error; atomic_int done; } Race;
static void *registration_worker(void *data) {
    Race *race=data;
    for (int i=0;i<1000;i++) {
        if (ALooper_removeFd(race->looper,race->fd)!=1 ||
            ALooper_addFd(race->looper,race->fd,42,1,NULL,race)!=1) { race->error=1; break; }
        ALooper_wake(race->looper);
    }
    ALooper_release(race->looper);
    atomic_store_explicit(&race->done,1,memory_order_release); return NULL;
}
typedef struct { ALooper *looper; int other,error; } PendingSwap;
static int replace_pending(int fd,int events,void *data) {
    PendingSwap *swap=data; char byte;
    if (!(events&1) || read(fd,&byte,1)!=1 || ALooper_removeFd(swap->looper,swap->other)!=1 ||
        ALooper_addFd(swap->looper,swap->other,77,1,NULL,swap)!=1) swap->error=1;
    return 0;
}
static int check_concurrent_and_pending(ALooper *looper) {
    int p[2],q[2],fd,events; void *data; char byte;
    CHECK(pipe(p)==0 && pipe(q)==0);
    PendingSwap swap={.looper=looper,.other=q[0]};
    CHECK(ALooper_addFd(looper,p[0],-2,1,replace_pending,&swap)==1);
    CHECK(ALooper_addFd(looper,q[0],76,1,NULL,&swap)==1);
    CHECK(write(p[1],"x",1)==1 && write(q[1],"x",1)==1);
    /* An old ident already in a kernel batch must not escape after replacement. */
    CHECK(ALooper_pollOnce(0,&fd,&events,&data)==-2 && !swap.error);
    CHECK(ALooper_pollOnce(0,&fd,&events,&data)==77 && fd==q[0] && data==&swap);
    CHECK(read(fd,&byte,1)==1 && ALooper_removeFd(looper,q[0])==1);
    CHECK(close(q[0])==0 && close(q[1])==0);
    Race race={.looper=looper,.fd=p[0]}; long thread;
    CHECK(ALooper_addFd(looper,p[0],42,1,NULL,&race)==1 && write(p[1],"x",1)==1);
    ALooper_acquire(looper);
    CHECK(pthread_create(&thread,NULL,registration_worker,&race)==0);
    do {
        int result=ALooper_pollOnce(0,&fd,&events,&data);
        CHECK(result==42 || result==-1 || result==-3);
        if (result==42) CHECK(fd==p[0] && (events&1) && data==&race);
    } while (!atomic_load_explicit(&race.done,memory_order_acquire));
    CHECK(pthread_join(thread,NULL)==0 && !race.error);
    CHECK(ALooper_removeFd(looper,p[0])==1 && read(p[0],&byte,1)==1);
    CHECK(close(p[0])==0 && close(p[1])==0);
    /* Drain the worker's final wake before subsequent timeout checks. */
    int result=ALooper_pollOnce(0,NULL,NULL,NULL); CHECK(result==-1 || result==-3);
    return 0;
}
static int check_all(void) {
    CHECK(ALooper_forThread()==NULL);
    ALooper *looper=ALooper_prepare(1);
    CHECK(looper && ALooper_forThread()==looper && ALooper_prepare(0)==looper);
    int fd=99,events=99; void *data=(void *)1;
    CHECK(ALooper_pollOnce(0,&fd,&events,&data)==-3 && !fd && !events && !data);
    int pipes[20][2];
    for (int i=0;i<20;i++) {
        CHECK(pipe(pipes[i])==0);
        CHECK(ALooper_addFd(looper,pipes[i][0],100+i,1,NULL,(void *)(uintptr_t)(i+1))==1);
        CHECK(write(pipes[i][1],"x",1)==1);
    }
    unsigned seen=0;
    for (int i=0;i<20;i++) {
        int ident=ALooper_pollOnce(100,&fd,&events,&data),index=ident-100; char byte;
        CHECK(index>=0 && index<20 && !(seen&(1u<<index)));
        CHECK(fd==pipes[index][0] && (events&1) && data==(void *)(uintptr_t)(index+1));
        seen|=1u<<index; CHECK(read(fd,&byte,1)==1);
    }
    CHECK(seen==((1u<<20)-1));
    for (int i=0;i<20;i++) {
        CHECK(ALooper_removeFd(looper,pipes[i][0])==1);
        CHECK(ALooper_removeFd(looper,pipes[i][0])==0);
        CHECK(close(pipes[i][0])==0 && close(pipes[i][1])==0);
    }
    int p[2]; CHECK(pipe(p)==0);
    CHECK(ALooper_addFd(looper,p[0],5,1,NULL,NULL)==1);
    CHECK(ALooper_addFd(looper,p[0],-1,1,NULL,NULL)==-1);
    CHECK(write(p[1],"x",1)==1 && ALooper_pollOnce(0,&fd,NULL,NULL)==5);
    char byte; CHECK(read(fd,&byte,1)==1);
    State state={.looper=looper};
    CHECK(ALooper_addFd(looper,p[0],-2,1,replace_self,&state)==1);
    CHECK(write(p[1],"x",1)==1 && ALooper_pollOnce(0,NULL,NULL,NULL)==-2);
    CHECK(state.calls==1 && !state.error);
    CHECK(write(p[1],"x",1)==1 && ALooper_pollOnce(0,NULL,NULL,NULL)==-2);
    CHECK(state.calls==2 && !state.error);
    CHECK(ALooper_addFd(looper,p[0],-2,1,reenter,&state)==1);
    CHECK(write(p[1],"x",1)==1 && ALooper_pollOnce(0,NULL,NULL,NULL)==-2);
    CHECK(state.calls==3 && !state.error && ALooper_removeFd(looper,p[0])==0);
    /* Callback and ident ready together: callback still executes before return. */
    int q[2]; CHECK(pipe(q)==0);
    CHECK(ALooper_addFd(looper,p[0],-2,1,replacement,&state)==1);
    CHECK(ALooper_addFd(looper,q[0],7,1,NULL,&state)==1);
    CHECK(write(p[1],"x",1)==1 && write(q[1],"x",1)==1);
    CHECK(ALooper_pollOnce(0,&fd,&events,&data)==7 && fd==q[0] && data==&state);
    CHECK(state.calls==4 && !state.error && read(q[0],&byte,1)==1);
    CHECK(ALooper_removeFd(looper,q[0])==1 && close(q[0])==0 && close(q[1])==0);
    /* Callback-only pollAll must honor its original deadline. */
    CHECK(ALooper_addFd(looper,p[0],-2,1,keep_ready,&state)==1 && write(p[1],"x",1)==1);
    int64_t before=millis();
    CHECK(ALooper_pollAll(30,&fd,&events,&data)==-3);
    CHECK(millis()-before>=20 && millis()-before<1000 && !fd && !events && !data);
    CHECK(ALooper_removeFd(looper,p[0])==1 && read(p[0],&byte,1)==1);
    /* Close/reuse the descriptor number while the stale registry exists. */
    CHECK(ALooper_addFd(looper,p[0],9,1,NULL,NULL)==1);
    int old=p[0]; CHECK(close(old)==0 && pipe(q)==0);
    if (q[0]!=old) { CHECK(dup2(q[0],old)==old && close(q[0])==0); q[0]=old; }
    CHECK(ALooper_addFd(looper,q[0],10,1,NULL,&state)==1);
    CHECK(write(q[1],"x",1)==1 && ALooper_pollOnce(0,&fd,NULL,NULL)==10 && fd==old);
    CHECK(read(fd,&byte,1)==1 && ALooper_removeFd(looper,fd)==1);
    CHECK(close(q[0])==0 && close(q[1])==0 && close(p[1])==0);
    ALooper_wake(looper); CHECK(ALooper_pollAll(-1,NULL,NULL,NULL)==-1);
    long thread; ALooper_acquire(looper);
    CHECK(pthread_create(&thread,NULL,wake_worker,looper)==0);
    before=millis(); CHECK(ALooper_pollOnce(-1,NULL,NULL,NULL)==-1);
    CHECK(millis()-before>=10 && millis()-before<1000 && pthread_join(thread,NULL)==0);
    for (int i=0;i<10000;i++) {
        CHECK(ALooper_forThread()==looper && ALooper_prepare(1)==looper);
        ALooper_acquire(looper); ALooper_release(looper);
        CHECK(ALooper_pollOnce(0,NULL,NULL,NULL)==-3);
    }
    int concurrent=check_concurrent_and_pending(looper); if (concurrent) return concurrent;
    ASensorManager *manager=ASensorManager_getInstance();
    const ASensor *const *list=(void *)1;
    CHECK(manager && ASensorManager_getInstance()==manager && ASensorManager_getSensorList(manager,&list)==0 && !list);
    CHECK(ASensorManager_getSensorList(NULL,&list)<0 && !list);
    CHECK(!ASensorManager_getDefaultSensor(manager,1) && !ASensorManager_getDefaultSensor(manager,4));
    CHECK(!ASensorManager_createEventQueue(manager,looper,1,replacement,&state));
    CHECK(ASensorManager_destroyEventQueue(manager,NULL)<0 && ASensorEventQueue_enableSensor(NULL,NULL)<0);
    CHECK(ASensorEventQueue_disableSensor(NULL,NULL)<0 && ASensorEventQueue_setEventRate(NULL,NULL,100)<0);
    CHECK(ASensorEventQueue_hasEvents(NULL)<0);
    uint64_t canary=0x123456789abcdef0ULL;
    CHECK(ASensorEventQueue_getEvents(NULL,&canary,1)<0 && canary==0x123456789abcdef0ULL);
    CHECK(!ASensor_getName(NULL) && !ASensor_getVendor(NULL) && ASensor_getType(NULL)<0);
    CHECK(ASensor_getMinDelay(NULL)<0 && ASensor_getResolution(NULL)==0.0f);
    return 0;
}
static void *full_worker(void *unused) { (void)unused; return (void *)(uintptr_t)check_all(); }
typedef struct { ALooper *retained; int error; } Retained;
static void *short_worker(void *data) {
    Retained *state=data;
    if (ALooper_forThread()) { state->error=1; return NULL; }
    ALooper *looper=ALooper_prepare(0);
    int p[2];
    if (!looper || pipe(p)) { state->error=2; return NULL; }
    if (ALooper_addFd(looper,p[0],1,1,NULL,NULL)!=-1) state->error=3;
    /* Leave a registration for the TLS destructor to free, but retain caller fds. */
    if (ALooper_addFd(looper,p[0],-2,1,keep_ready,NULL)!=1) state->error=4;
    close(p[0]); close(p[1]);
    if (state->retained) { ALooper_acquire(looper); state->retained=looper; }
    return NULL;
}
int rudroid_native_events_probe(uint64_t *iterations) {
    CHECK(ALooper_forThread()==NULL && ALooper_pollOnce(0,NULL,NULL,NULL)==-4);
    long thread; void *result=NULL;
    CHECK(pthread_create(&thread,NULL,full_worker,NULL)==0 && pthread_join(thread,&result)==0);
    if (result) return (int)(uintptr_t)result;
    Retained state={.retained=(void *)1};
    CHECK(pthread_create(&thread,NULL,short_worker,&state)==0 && pthread_join(thread,NULL)==0 && !state.error);
    CHECK(state.retained && state.retained!=(void *)1);
    ALooper_wake(state.retained); ALooper_release(state.retained);
    state.retained=NULL;
    for (int i=0;i<2000;i++) {
        CHECK(pthread_create(&thread,NULL,short_worker,&state)==0 && pthread_join(thread,NULL)==0 && !state.error);
    }
    CHECK(ALooper_forThread()==NULL);
    *iterations=2000; return 0;
}

/* Identical harness for the C baseline and Rust implementation. Setup/teardown,
 * fd allocation and timer calls stay outside the measured zero-allocation poll. */
typedef struct { uint64_t samples[4][5]; int error; } Cost;
static int64_t nanos(void) { Time t; clock_gettime(1,&t); return t.sec*1000000000+t.nsec; }
static int measure_cost(Cost *cost) {
    const int counts[4]={1,16,64,256}, loops=32768;
    ALooper *looper=ALooper_prepare(1); CHECK(looper);
    for (int n=0;n<4;n++) {
        int pipes[256][2]; State state={.looper=looper};
        for (int i=0;i<counts[n];i++) {
            CHECK(pipe(pipes[i])==0);
            CHECK(ALooper_addFd(looper,pipes[i][0],-2,1,keep_ready,&state)==1);
            CHECK(write(pipes[i][1],"x",1)==1);
        }
        for (int warm=0;warm<1024;warm++) CHECK(ALooper_pollOnce(0,NULL,NULL,NULL)==-2);
        for (int sample=0;sample<5;sample++) {
            state.calls=0; int64_t start=nanos();
            for (int i=0;i<loops;i++) CHECK(ALooper_pollOnce(0,NULL,NULL,NULL)==-2);
            cost->samples[n][sample]=(uint64_t)(nanos()-start);
            CHECK(state.calls==loops*(counts[n]<16?counts[n]:16));
        }
        for (int i=0;i<counts[n];i++) {
            CHECK(ALooper_removeFd(looper,pipes[i][0])==1);
            CHECK(close(pipes[i][0])==0 && close(pipes[i][1])==0);
        }
    }
    return 0;
}
static void *cost_worker(void *data) { Cost *cost=data; cost->error=measure_cost(cost); return NULL; }
int rudroid_native_events_cost(uint64_t *samples) {
    Cost cost={0}; long thread;
    CHECK(pthread_create(&thread,NULL,cost_worker,&cost)==0 && pthread_join(thread,NULL)==0);
    if (cost.error) return cost.error;
    for (int n=0;n<4;n++) for (int i=0;i<5;i++) samples[n*5+i]=cost.samples[n][i];
    return 0;
}
