// INPUT:  isolated CLI child's marker path and host C runtime finalizers
// OUTPUT:  a marker only if ordinary process exit replays a retired callback
// POS:    preload regression fixture for post-guest success/failure termination

#include <fcntl.h>
#include <stdlib.h>
#include <unistd.h>

extern int __cxa_atexit(void (*callback)(void *), void *argument, void *dso);

static void finalizer(void *unused) {
    (void)unused;
    const char *path = getenv("RUDROID_FINALIZER_MARKER");
    if (!path) return;
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return;
    (void)write(fd, "replayed", 8);
    (void)close(fd);
}

__attribute__((constructor)) static void register_finalizer(void) {
    (void)__cxa_atexit(finalizer, NULL, NULL);
}
