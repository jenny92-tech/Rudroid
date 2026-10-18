// INPUT:  Android LP64 pthread prototypes, production Bionic provider and native threads
// OUTPUT:  rudroid_pthread_probe verifies contention, TLS, attributes and 4-byte-aligned facades
// POS:    exercises versioned guest calls against the shared host synchronization service
typedef struct { int private[10]; } Mutex;
typedef struct { int private[12]; } Cond;
typedef struct { int private[14]; } Rwlock;
typedef struct {
    unsigned flags; void *base; unsigned long size, guard;
    int policy, priority; char reserved[16];
} Attr;
typedef struct { long sec, nsec; } Time;
extern int pthread_mutex_init(Mutex *, const long *);
extern int pthread_mutex_destroy(Mutex *);
extern int pthread_mutex_lock(Mutex *);
extern int pthread_mutex_trylock(Mutex *);
extern int pthread_mutex_unlock(Mutex *);
extern int pthread_mutexattr_init(long *);
extern int pthread_mutexattr_settype(long *, int);
extern int pthread_mutexattr_setpshared(long *, int);
extern int pthread_mutexattr_destroy(long *);
extern int pthread_cond_wait(Cond *, Mutex *);
extern int pthread_cond_broadcast(Cond *);
extern int pthread_cond_timedwait(Cond *, Mutex *, const Time *);
extern int pthread_cond_destroy(Cond *);
extern int pthread_rwlock_rdlock(Rwlock *);
extern int pthread_rwlock_trywrlock(Rwlock *);
extern int pthread_rwlock_wrlock(Rwlock *);
extern int pthread_rwlock_unlock(Rwlock *);
extern int pthread_rwlock_destroy(Rwlock *);
extern int pthread_attr_init(Attr *);
extern int pthread_attr_setstacksize(Attr *, unsigned long);
extern int pthread_attr_getstack(Attr *, void **, unsigned long *);
extern int pthread_attr_destroy(Attr *);
extern int pthread_getattr_np(long, Attr *);
extern long pthread_self(void);
extern int pthread_equal(long, long);
extern int pthread_create(long *, const Attr *, void *(*)(void *), void *);
extern int pthread_join(long, void **);
extern int pthread_once(int *, void (*)(void));
extern int pthread_key_create(int *, void (*)(void *));
extern int pthread_key_delete(int);
extern int pthread_setspecific(int, const void *);
extern void *pthread_getspecific(int);
extern int pthread_sigmask(int, const unsigned long *, unsigned long *);
_Static_assert(sizeof(Mutex) == 40 && _Alignof(Mutex) == 4, "mutex ABI");
_Static_assert(sizeof(Cond) == 48 && _Alignof(Cond) == 4, "cond ABI");
_Static_assert(sizeof(Rwlock) == 56 && _Alignof(Rwlock) == 4, "rwlock ABI");
_Static_assert(sizeof(Attr) == 56 && _Alignof(Attr) == 8, "attr ABI");

_Alignas(8) static struct { int prefix; Mutex lock; int suffix; } mutex;
_Alignas(8) static struct { int prefix; Cond cond; int suffix; } condition;
_Alignas(8) static struct { int prefix; Rwlock lock; int suffix; } rw;
static int key, once, once_calls, tls_calls, ready, go, counter;
static void initialize_once(void) { ++once_calls; }
static void tls_destructor(void *value) {
    if (value == &counter) __atomic_fetch_add(&tls_calls, 1, __ATOMIC_RELAXED);
}
static void *worker(void *argument) {
    Attr attr; void *base = 0; unsigned long size = 0;
    if (pthread_getattr_np(pthread_self(), &attr) != 0) return (void *)1;
    if (pthread_attr_getstack(&attr, &base, &size) != 0 || !base || size < 131072) return (void *)2;
    if (pthread_attr_destroy(&attr) != 0) return (void *)3;
    if (!pthread_equal(pthread_self(), pthread_self())) return (void *)4;
    if (pthread_setspecific(key, &counter) || pthread_getspecific(key) != &counter) return (void *)5;
    if (pthread_once(&once, initialize_once)) return (void *)6;
    if (pthread_mutex_lock(&mutex.lock)) return (void *)7;
    ++ready;
    pthread_cond_broadcast(&condition.cond);
    while (!go) if (pthread_cond_wait(&condition.cond, &mutex.lock)) return (void *)8;
    pthread_mutex_unlock(&mutex.lock);
    for (int i = 0; i < 2000; ++i) {
        if (pthread_mutex_lock(&mutex.lock)) return (void *)9;
        ++counter;
        if (pthread_mutex_unlock(&mutex.lock)) return (void *)10;
    }
    return argument;
}
int rudroid_pthread_probe(void) {
    mutex.prefix = mutex.suffix = condition.prefix = condition.suffix = rw.prefix = rw.suffix = 0x13579bdf;
    if ((unsigned long)&mutex.lock % 8 != 4 || (unsigned long)&condition.cond % 8 != 4 || (unsigned long)&rw.lock % 8 != 4) return 1;
    // Static recursive initializer must not become a normal native lock.
    mutex.lock.private[0] = 1 << 14;
    if (pthread_mutex_lock(&mutex.lock) || pthread_mutex_trylock(&mutex.lock)) return 2;
    if (pthread_mutex_unlock(&mutex.lock) || pthread_mutex_unlock(&mutex.lock)) return 3;
    if (pthread_mutex_destroy(&mutex.lock)) return 4;
    long attr;
    if (pthread_mutexattr_init(&attr) || pthread_mutexattr_setpshared(&attr, 1)) return 5;
    if (pthread_mutex_init(&mutex.lock, &attr) != 95) return 6; // ENOTSUP, never fake a shared lock.
    if (pthread_mutexattr_setpshared(&attr, 0) || pthread_mutexattr_settype(&attr, 2)) return 7;
    if (pthread_mutex_init(&mutex.lock, &attr) || pthread_mutexattr_destroy(&attr)) return 8;
    if (pthread_mutex_lock(&mutex.lock) || pthread_mutex_lock(&mutex.lock) != 35) return 9; // EDEADLK.
    Time past = { 0, 0 };
    if (pthread_cond_timedwait(&condition.cond, &mutex.lock, &past) != 110) return 10;
    if (pthread_mutex_unlock(&mutex.lock)) return 11;
    struct { unsigned long before, mask, after; } masks = { 71, 0, 82 };
    unsigned long original = 0, blocked = 1UL << (10 - 1); // Linux ARM64 SIGUSR1.
    if (pthread_sigmask(0, &blocked, &original) || pthread_sigmask(0, 0, &masks.mask)) return 25;
    if (!(masks.mask & blocked) || masks.before != 71 || masks.after != 82) return 26;
    if (pthread_sigmask(2, &original, 0)) return 27;
    if (pthread_rwlock_rdlock(&rw.lock) || pthread_rwlock_trywrlock(&rw.lock) != 16) return 12;
    if (pthread_rwlock_unlock(&rw.lock) || pthread_rwlock_wrlock(&rw.lock) || pthread_rwlock_unlock(&rw.lock)) return 13;
    Attr thread_attr;
    if (pthread_attr_init(&thread_attr) || pthread_attr_setstacksize(&thread_attr, 131072)) return 14;
    if (pthread_key_create(&key, tls_destructor)) return 15;
    long threads[2];
    if (pthread_create(&threads[0], &thread_attr, worker, (void *)31)) return 16;
    if (pthread_create(&threads[1], &thread_attr, worker, (void *)32)) return 17;
    if (pthread_attr_destroy(&thread_attr)) return 18;
    if (pthread_mutex_lock(&mutex.lock)) return 19;
    while (ready != 2) if (pthread_cond_wait(&condition.cond, &mutex.lock)) return 20;
    go = 1;
    pthread_cond_broadcast(&condition.cond);
    pthread_mutex_unlock(&mutex.lock);
    for (int i = 0; i < 2; ++i) {
        void *result = 0;
        if (pthread_join(threads[i], &result) || result != (void *)(unsigned long)(31 + i)) return 21;
    }
    if (counter != 4000 || once_calls != 1 || tls_calls != 2 || pthread_getspecific(key)) return 22;
    if (pthread_key_delete(key) || pthread_cond_destroy(&condition.cond) || pthread_rwlock_destroy(&rw.lock) || pthread_mutex_destroy(&mutex.lock)) return 23;
    if (mutex.prefix != 0x13579bdf || mutex.suffix != 0x13579bdf || condition.prefix != 0x13579bdf || condition.suffix != 0x13579bdf || rw.prefix != 0x13579bdf || rw.suffix != 0x13579bdf) return 24;
    return 0;
}
