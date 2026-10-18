// INPUT:  guest-scoped __cxa_* providers, two DSO tokens and host observation buffer
// OUTPUT:  rudroid_destructor_probe registers process/TLS destructors for explicit/unmap cleanup
// POS:    executes guest callbacks before executable memory is released, without host DSO abuse
extern int __cxa_atexit(void (*)(void *), void *, void *);
extern void __cxa_finalize(void *);
extern int __cxa_thread_atexit_impl(void (*)(void *), void *, void *);
static char first, second, module;
static int *output;
static int initialization;
static void record(void *value) {
    int index = output[0]++;
    output[index + 1] = (int)(unsigned long)value;
}
static void reentrant(void *unused) {
    (void)unused;
    record((void *)3);
    __cxa_atexit(record, (void *)4, &first);
    __cxa_finalize(&second);
}
static void thread_reentrant(void *unused) {
    (void)unused;
    record((void *)7);
    __cxa_thread_atexit_impl(record, (void *)8, &first);
}
static void module_destructor(void *unused) {
    (void)unused;
    if (output) record((void *)9);
}
__attribute__((constructor)) static void module_constructor(void) {
    initialization = __cxa_atexit(module_destructor, 0, &module);
}
int rudroid_thread_destructor_probe(int *results) {
    output = results;
    if (__cxa_thread_atexit_impl(record, (void *)6, &first) != 0) return 1;
    if (__cxa_thread_atexit_impl(thread_reentrant, 0, &first) != 0) return 2;
    return 0;
}
int rudroid_destructor_probe(int *results) {
    output = results;
    if (initialization != 0) return 8;
    if (__cxa_atexit(record, (void *)1, &first) != 0) return 1;
    if (__cxa_atexit(record, (void *)2, &second) != 0) return 2;
    if (__cxa_atexit(reentrant, 0, &first) != 0) return 3;
    __cxa_finalize(&first);
    __cxa_finalize(&first); // no repeat calls
    if (output[0] != 4 || output[1] != 3 || output[2] != 2 || output[3] != 4 || output[4] != 1) return 4;
    if (__cxa_atexit(record, (void *)5, &first) != 0) return 5;
    if (__cxa_thread_atexit_impl(record, (void *)6, &first) != 0) return 6;
    if (__cxa_thread_atexit_impl(thread_reentrant, 0, &first) != 0) return 7;
    // Expected on pre-unmap cleanup: thread 7,8,6 then process 5, constructor 9.
    return 0;
}
