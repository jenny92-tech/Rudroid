// INPUT:  fixed Android ARM64 layouts, shared Rust asset resolver and native POSIX
// OUTPUT:  rudroid_bionic_posix_symbol/_symbol_name table, LP64 asset I/O, seeded rand and sysconf adapters
// POS:    exposes native services with one shared extracted-asset path boundary
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <fnmatch.h>

#include <pwd.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <sys/prctl.h>
#include <sys/ptrace.h>
#include <sys/utsname.h>
#include <sys/vfs.h>
#include <sched.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/sendfile.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <syslog.h>
#include <time.h>
#include <unistd.h>
#include <utime.h>
#include <wchar.h>
#include <wctype.h>
#include <sys/ioctl.h>
#include <sys/inotify.h>
#include <asm/termbits.h>
#include "symbol_table.h"

#if !defined(__aarch64__) || !defined(__LP64__)
#error This provider attests only Linux ARM64 LP64
#endif
// Bionic's termios is the kernel's 36-byte structure, not GNU's larger
// userspace structure. Send it straight to the matching Linux ioctls.
// Contract: AOSP libc/include/bits/termios_inlines.h and asm-generic/termbits.h.
_Static_assert(sizeof(struct termios) == 36 && NCCS == 19 &&
    offsetof(struct termios,c_cc) == 17, "Android ARM64 kernel termios ABI");
static char *guest_basename(const char *path) {
    static _Thread_local char result[PATH_MAX];
    if (!path || !*path) path = ".";
    size_t end = strlen(path);
    while (end > 1 && path[end - 1] == '/') --end;
    size_t start = end;
    if (end == 1 && path[0] == '/') start = 0;
    else while (start && path[start - 1] != '/') --start;
    size_t length = end - start;
    if (length >= sizeof(result)) {
        errno = ERANGE;
        return NULL;
    }
    memmove(result, path + start, length);
    result[length] = 0;
    return result;
}

static int guest_tcgetattr(int fd, struct termios *out) { return ioctl(fd, TCGETS, out); }
static int guest_tcsetattr(int fd, int action, const struct termios *in) {
    unsigned long request;
    switch (action) {
        case 0: request = TCSETS; break;
        case 1: request = TCSETSW; break;
        case 2: request = TCSETSF; break;
        default: errno = EINVAL; return -1;
    }
    return ioctl(fd, request, in);
}
static int guest_tcflush(int fd, int queue) { return ioctl(fd, TCFLSH, (unsigned long)queue); }
// Models are based on Android 15 headers, not the native typedefs. Check every
// public field offset/width as well as total size/alignment at compile time.
#include "posix_layout.h"

// Android's pathconf namespace differs too; sharing native storage does not
// mean forwarding numeric constants. Names are fixed by Android 15 unistd.h.
static int pathconf_name(int name) {
    static const int names[] = {
        _PC_FILESIZEBITS, _PC_LINK_MAX, _PC_MAX_CANON, _PC_MAX_INPUT, _PC_NAME_MAX,
        _PC_PATH_MAX, _PC_PIPE_BUF, _PC_2_SYMLINKS, _PC_ALLOC_SIZE_MIN,
        _PC_REC_INCR_XFER_SIZE, _PC_REC_MAX_XFER_SIZE, _PC_REC_MIN_XFER_SIZE,
        _PC_REC_XFER_ALIGN, _PC_SYMLINK_MAX, _PC_CHOWN_RESTRICTED, _PC_NO_TRUNC,
        _PC_VDISABLE, _PC_ASYNC_IO, _PC_PRIO_IO, _PC_SYNC_IO
    };
    if ((unsigned)name >= sizeof(names)/sizeof(names[0])) { errno = EINVAL; return -1; }
    return names[name];
}
static long guest_pathconf(const char *path, int name) {
    int native = pathconf_name(name);
    return native < 0 ? -1 : pathconf(path,native);
}
static long guest_fpathconf(int fd, int name) {
    int native = pathconf_name(name);
    return native < 0 ? -1 : fpathconf(fd,native);
}

// These fixed capability facts are shared by get/find/read; no property
// area, Android private prop_info layout, or per-query string allocation.
// Publish only the architecture this runtime actually supports.
typedef struct { const char *name, *value; } Property;
static const Property properties[] = {
    {"ro.product.cpu.abilist", "arm64-v8a"},
    {"ro.product.cpu.abi", "arm64-v8a"},
    {"swappy.disable", "1"} // Native SDL presentation supplies frame pacing.
};
static const Property *property_find(const char *name) {
    if (name) for (size_t i=0; i<sizeof(properties)/sizeof(properties[0]); ++i)
        if (!strcmp(name,properties[i].name)) return &properties[i];
    return NULL;
}
static int property_get(const char *name, char *value) {
    if (!value) return 0;
    const Property *p = property_find(name);
    const char *text = p ? p->value : "";
    strcpy(value,text);
    return (int)strlen(text);
}
static int property_read(const Property *p, char *name, char *value) {
    if (name) strcpy(name,p ? p->name : "");
    if (value) strcpy(value,p ? p->value : "");
    return p ? (int)strlen(p->value) : 0;
}
static const Property *property_find_nth(unsigned index) {
    return index < sizeof(properties) / sizeof(properties[0]) ? &properties[index] : NULL;
}

// Bionic sysconf numbers differ from GNU. Resource limits are deliberately
// queried from the native service each time; available memory is not cached.
// Feature-advertisement queries are excluded until the corresponding guest
// adapter exists (e.g. process-shared locks must not be advertised as supported).
static long bridge_sysconf(int name) {
    switch (name) {
        case 0x0000: return sysconf(_SC_ARG_MAX);
        case 0x0005: return sysconf(_SC_CHILD_MAX);
        case 0x0006: return sysconf(_SC_CLK_TCK);
        case 0x000a: return sysconf(_SC_NGROUPS_MAX);
        case 0x000b: return sysconf(_SC_OPEN_MAX);
        case 0x001b: return sysconf(_SC_STREAM_MAX);
        case 0x001c: return sysconf(_SC_TZNAME_MAX);
        case 0x0026: return sysconf(_SC_IOV_MAX);
        case 0x0027: return sysconf(_SC_PAGESIZE);
        case 0x0028: return sysconf(_SC_PAGE_SIZE);
        case 0x002e: return sysconf(_SC_AIO_LISTIO_MAX);
        case 0x002f: return sysconf(_SC_AIO_MAX);
        case 0x0030: return sysconf(_SC_AIO_PRIO_DELTA_MAX);
        case 0x0031: return sysconf(_SC_DELAYTIMER_MAX);
        case 0x0032: return sysconf(_SC_MQ_OPEN_MAX);
        case 0x0033: return sysconf(_SC_MQ_PRIO_MAX);
        case 0x0034: return sysconf(_SC_RTSIG_MAX);
        case 0x0036: return sysconf(_SC_SEM_VALUE_MAX);
        case 0x0037: return sysconf(_SC_SIGQUEUE_MAX);
        case 0x0038: return sysconf(_SC_TIMER_MAX);
        case 0x0049: return sysconf(_SC_LOGIN_NAME_MAX);
        case 0x004a: return sysconf(_SC_THREAD_DESTRUCTOR_ITERATIONS);
        case 0x004b: return sysconf(_SC_THREAD_KEYS_MAX);
        case 0x004c: return sysconf(_SC_THREAD_STACK_MIN);
        case 0x004e: return sysconf(_SC_TTY_NAME_MAX);
        case 0x0060: return sysconf(_SC_NPROCESSORS_CONF);
        case 0x0061: return sysconf(_SC_NPROCESSORS_ONLN);
        case 0x0062: return sysconf(_SC_PHYS_PAGES);
        case 0x0063: return sysconf(_SC_AVPHYS_PAGES);
        case 0x0064: return sysconf(_SC_MONOTONIC_CLOCK);
        case 0x006f: return sysconf(_SC_HOST_NAME_MAX);
        default: errno = EINVAL; return -1;
    }
}
// Android strerror_r returns an int, writes even a truncated message, and
// preserves errno. Its GNU alias always returns the caller's buffer. Native
// GNU strerror_r can instead return a static string without filling that
// buffer; exposing its same-named address would violate both contracts.
static int bridge_strerror_r(int error, char *buffer, size_t size) {
    int saved_errno = errno;
    char scratch[128];
    const char *message = strerror_r(error, scratch, sizeof(scratch));
    size_t length = strlen(message);
    if (size) {
        size_t copied = length < size - 1 ? length : size - 1;
        memcpy(buffer, message, copied);
        buffer[copied] = '\0';
    }
    errno = saved_errno;
    return length >= size ? ERANGE : 0;
}
static char *bridge_gnu_strerror_r(int error, char *buffer, size_t size) {
    bridge_strerror_r(error, buffer, size);
    return buffer;
}
extern int rudroid_bionic_ioctl(int fd, int opcode, ...);
extern char *rudroid_bionic_asset_path(const char *path);
// Every mapped string is C-owned and borrowed only for the native call.
// Preserve its I/O errno across free on older supported glibc versions.
#define ASSET_CALL(type, call) \
    char *mapped = rudroid_bionic_asset_path(path); \
    const char *native_path = mapped ? mapped : path; \
    type result = (call); int saved_errno = errno; \
    free(mapped); errno = saved_errno; return result
static int guest_open(const char *path, int flags, ...) {
    mode_t mode = 0;
    if ((flags & O_CREAT) || ((flags & O_TMPFILE) == O_TMPFILE)) {
        va_list args; va_start(args, flags); mode = va_arg(args, mode_t); va_end(args);
    }
    ASSET_CALL(int, open(native_path, flags, mode));
}
static int guest_openat(int dir, const char *path, int flags, ...) {
    mode_t mode = 0;
    if ((flags & O_CREAT) || ((flags & O_TMPFILE) == O_TMPFILE)) {
        va_list args; va_start(args, flags); mode = va_arg(args, mode_t); va_end(args);
    }
    ASSET_CALL(int, openat(dir, native_path, flags, mode));
}
// FORTIFY variants with no variadic mode argument. Their compiler contract
// only permits flags that do not create a file; malformed callers fail closed.
static int bridge_open_2(const char *path, int flags) {
    if ((flags & O_CREAT) || ((flags & O_TMPFILE) == O_TMPFILE)) abort();
    return guest_open(path, flags);
}
static int bridge_openat_2(int dir, const char *path, int flags) {
    if ((flags & O_CREAT) || ((flags & O_TMPFILE) == O_TMPFILE)) abort();
    return guest_openat(dir, path, flags);
}
static ssize_t bridge_read_chk(int fd, void *buffer, size_t count, size_t capacity) {
    if (count > SSIZE_MAX || count > capacity) abort();
    return read(fd, buffer, count);
}
static char *bridge_strchr_chk(const char *text, int byte, size_t capacity) {
    for (;; ++text, --capacity) {
        if (capacity == 0) abort();
        if (*text == (char)byte) return (char *)text;
        if (*text == '\0') return NULL;
    }
}
static char *bridge_strcpy_chk(char *destination, const char *source, size_t capacity) {
    size_t length = strlen(source);
    if (length >= capacity) abort();
    memcpy(destination, source, length + 1);
    return destination;
}
// Scan no farther than the caller-owned limit. FORTIFY must detect an
// impossible append before the source scan reaches an unrelated guard page.
static size_t bounded_string_length(const char *source, size_t limit) {
    for (size_t length = 0; length < limit; ++length) {
        if (source[length] == '\0') return length;
    }
    return limit;
}
static char *bridge_strcat_chk(char *destination, const char *source, size_t capacity) {
    size_t used = strnlen(destination, capacity);
    if (used == capacity) abort();
    size_t remaining = capacity - used;
    size_t length = bounded_string_length(source, remaining);
    if (length == remaining) abort();
    memcpy(destination + used, source, length + 1);
    return destination;
}
static char *bridge_strncat_chk(char *destination, const char *source, size_t count,
    size_t capacity) {
    if (count == 0) return destination;
    size_t used = strnlen(destination, capacity);
    if (used == capacity) abort();
    size_t remaining = capacity - used;
    size_t limit = count < remaining ? count : remaining;
    size_t length = bounded_string_length(source, limit);
    if (length == remaining) abort();
    memcpy(destination + used, source, length);
    destination[used + length] = '\0';
    return destination;
}
static char *bridge_strncpy_chk(char *destination, const char *source, size_t count,
    size_t capacity) {
    if (count > capacity) abort();
    return strncpy(destination, source, count);
}
static char *bridge_strncpy_chk2(char *destination, const char *source, size_t count,
    size_t capacity, size_t source_length) {
    if (count > capacity) abort();
    for (size_t index = 0; index < count; ++index) {
        if (index == source_length) abort();
        char value = source[index];
        destination[index] = value;
        if (value == '\0') {
            memset(destination + index + 1, 0, count - index - 1);
            return destination;
        }
    }
    return destination;
}
static char *bridge_strrchr_chk(const char *text, int byte, size_t capacity) {
    const char *found = NULL;
    for (size_t index = 0; index < capacity; ++index) {
        if (text[index] == (char)byte) found = text + index;
        if (text[index] == '\0') return (char *)found;
    }
    abort();
}
static size_t bridge_strlcpy_chk(char *destination, const char *source, size_t size,
    size_t capacity) {
    if (size > capacity) abort();
    size_t length = strlen(source);
    if (size) {
        size_t copied = length < size - 1 ? length : size - 1;
        memcpy(destination, source, copied);
        destination[copied] = '\0';
    }
    return length;
}
extern void *rudroid_bionic_stdio_native(void *guest);
static char *bridge_fgets_chk(char *destination, int size, void *guest, size_t capacity) {
    if (size <= 0 || (size_t)size > capacity) abort();
    void *stream = rudroid_bionic_stdio_native(guest);
    return stream ? fgets(destination, size, stream) : NULL;
}
static DIR *guest_opendir(const char *path) { ASSET_CALL(DIR *, opendir(native_path)); }
static int guest_access(const char *path, int mode) { ASSET_CALL(int, access(native_path, mode)); }
static int guest_stat(const char *path, struct stat *out) { ASSET_CALL(int, stat(native_path, out)); }
static int guest_lstat(const char *path, struct stat *out) { ASSET_CALL(int, lstat(native_path, out)); }
static int guest_fstatat(int dir, const char *path, struct stat *out, int flags) {
    ASSET_CALL(int, fstatat(dir, native_path, out, flags));
}
static int guest_inotify_add_watch(int fd, const char *path, uint32_t mask) {
    ASSET_CALL(int, inotify_add_watch(fd, native_path, mask));
}
#undef ASSET_CALL
// AOSP bionic android-15 stdio.h: L_tmpnam is 4096 and tmpnam returns one
// process-static buffer for a null caller buffer. An explicit TMPDIR is used
// verbatim; without one, this host adaptation uses the host temp directory
// because the Android /data/local/tmp default does not exist here. mktemp only
// selects an unused name and never creates the file (a dangling symlink counts
// as occupied).
static char guest_tmpnam_buffer[4096];
static char *guest_tmpnam(char *buffer) {
    const char *directory = getenv("TMPDIR");
    if (!directory || !*directory) directory = P_tmpdir;
    char *output = buffer ? buffer : guest_tmpnam_buffer;
    int length = snprintf(output, sizeof(guest_tmpnam_buffer), "%s/tmpnam.XXXXXXXXXX", directory);
    if (length < 0 || (size_t)length >= sizeof(guest_tmpnam_buffer)) {
        errno = ENAMETOOLONG;
        return NULL;
    }
    // glibc mktemp returns the template even when it cannot find a free name,
    // with output[0] cleared; Android returns NULL. Preserve the native errno.
    char *result = mktemp(output);
    return result && result[0] ? result : NULL;
}
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("environ", &environ)
    RUDROID_SYMBOL_SAME(prctl) RUDROID_SYMBOL_SAME(ptrace)
    RUDROID_SYMBOL_SAME(fcntl)
    RUDROID_SYMBOL_SAME(flock)
    RUDROID_SYMBOL_SAME(fnmatch)
    RUDROID_SYMBOL("basename", guest_basename)
    RUDROID_SYMBOL_SAME(getpriority)
    RUDROID_SYMBOL_SAME(setpriority)
    RUDROID_SYMBOL_SAME(nice)
    RUDROID_SYMBOL_SAME(getpwuid)
    RUDROID_SYMBOL_SAME(getpwuid_r)
    RUDROID_SYMBOL_SAME(uname)
    RUDROID_SYMBOL_SAME(div)
    RUDROID_SYMBOL_SAME(ldiv)
    RUDROID_SYMBOL_SAME(lldiv)
    RUDROID_SYMBOL_SAME(abs)
    RUDROID_SYMBOL_SAME(drand48)
    RUDROID_SYMBOL_SAME(erand48)
    RUDROID_SYMBOL_SAME(lrand48)
    RUDROID_SYMBOL_SAME(nrand48)
    RUDROID_SYMBOL_SAME(srand48)
    RUDROID_SYMBOL_SAME(seed48)
    RUDROID_SYMBOL_SAME(random)
    RUDROID_SYMBOL_SAME(statfs)
    RUDROID_SYMBOL_SAME(fstatfs)
    RUDROID_SYMBOL_SAME(statfs64)
    RUDROID_SYMBOL_SAME(fstatfs64)
    RUDROID_SYMBOL_SAME(execl)
    RUDROID_SYMBOL_SAME(execv)
    RUDROID_SYMBOL("access", guest_access)
    RUDROID_SYMBOL_SAME(chdir)
    RUDROID_SYMBOL_SAME(getcwd)
    RUDROID_SYMBOL_SAME(close)
    RUDROID_SYMBOL_SAME(dup)
    RUDROID_SYMBOL_SAME(dup2)
    RUDROID_SYMBOL_SAME(read)
    RUDROID_SYMBOL("__read_chk", bridge_read_chk)
    RUDROID_SYMBOL("__strchr_chk", bridge_strchr_chk)
    RUDROID_SYMBOL("__strcpy_chk", bridge_strcpy_chk)
    RUDROID_SYMBOL("__strcat_chk", bridge_strcat_chk)
    RUDROID_SYMBOL("__strncat_chk", bridge_strncat_chk)
    RUDROID_SYMBOL("__strncpy_chk", bridge_strncpy_chk)
    RUDROID_SYMBOL("__strncpy_chk2", bridge_strncpy_chk2)
    RUDROID_SYMBOL("__strrchr_chk", bridge_strrchr_chk)
    RUDROID_SYMBOL("__strlcpy_chk", bridge_strlcpy_chk)
    RUDROID_SYMBOL("__fgets_chk", bridge_fgets_chk)
    RUDROID_SYMBOL_SAME(write)
    RUDROID_SYMBOL_SAME(pread)
    RUDROID_SYMBOL("pread64", pread)
    RUDROID_SYMBOL_SAME(pwrite)
    RUDROID_SYMBOL("pwrite64", pwrite)
    RUDROID_SYMBOL_SAME(lseek)
    RUDROID_SYMBOL_SAME(lseek64)
    RUDROID_SYMBOL("open", guest_open)
    RUDROID_SYMBOL("open64", guest_open)
    RUDROID_SYMBOL("__open_2", bridge_open_2)
    RUDROID_SYMBOL("openat", guest_openat)
    RUDROID_SYMBOL("openat64", guest_openat)
    RUDROID_SYMBOL("__openat_2", bridge_openat_2)
    RUDROID_SYMBOL_SAME(chmod)
    RUDROID_SYMBOL_SAME(fchmod)
    RUDROID_SYMBOL_SAME(fchmodat)
    RUDROID_SYMBOL_SAME(truncate)
    RUDROID_SYMBOL_SAME(ftruncate)
    RUDROID_SYMBOL("ftruncate64", ftruncate)
    RUDROID_SYMBOL_SAME(mkdir)
    RUDROID_SYMBOL_SAME(rmdir)
    RUDROID_SYMBOL_SAME(unlink)
    RUDROID_SYMBOL_SAME(unlinkat)
    RUDROID_SYMBOL_SAME(rename)
    RUDROID_SYMBOL_SAME(remove)
    RUDROID_SYMBOL_SAME(link)
    RUDROID_SYMBOL_SAME(symlink)
    RUDROID_SYMBOL_SAME(readlink)
    RUDROID_SYMBOL_SAME(realpath)
    RUDROID_SYMBOL_SAME(sendfile)
    RUDROID_SYMBOL("opendir", guest_opendir)
    RUDROID_SYMBOL_SAME(fdopendir)
    RUDROID_SYMBOL_SAME(closedir)
    RUDROID_SYMBOL_SAME(readdir)
    RUDROID_SYMBOL_SAME(readdir64)
    RUDROID_SYMBOL_SAME(dirfd)
    RUDROID_SYMBOL_SAME(rewinddir)
    RUDROID_SYMBOL("stat", guest_stat)
    RUDROID_SYMBOL("stat64", guest_stat)
    RUDROID_SYMBOL("lstat", guest_lstat)
    RUDROID_SYMBOL("lstat64", guest_lstat)
    RUDROID_SYMBOL_SAME(fstat)
    RUDROID_SYMBOL("fstat64", fstat)
    RUDROID_SYMBOL("fstatat", guest_fstatat)
    RUDROID_SYMBOL("fstatat64", guest_fstatat)
    RUDROID_SYMBOL_SAME(statvfs)
    RUDROID_SYMBOL_SAME(fstatvfs)
    RUDROID_SYMBOL_SAME(statvfs64)
    RUDROID_SYMBOL_SAME(fstatvfs64)
    RUDROID_SYMBOL_SAME(clock_gettime)
    RUDROID_SYMBOL_SAME(clock_getres)
    RUDROID_SYMBOL_SAME(nanosleep)
    RUDROID_SYMBOL_SAME(clock)
    RUDROID_SYMBOL_SAME(time)
    RUDROID_SYMBOL_SAME(difftime)
    RUDROID_SYMBOL_SAME(gettimeofday)
    RUDROID_SYMBOL_SAME(settimeofday)
    RUDROID_SYMBOL_SAME(gmtime)
    RUDROID_SYMBOL_SAME(gmtime_r)
    RUDROID_SYMBOL_SAME(localtime)
    RUDROID_SYMBOL_SAME(localtime_r)
    RUDROID_SYMBOL_SAME(mktime)
    RUDROID_SYMBOL_SAME(strftime)
    RUDROID_SYMBOL_SAME(utime)
    RUDROID_SYMBOL_SAME(utimes)
    RUDROID_SYMBOL_SAME(utimensat)
    RUDROID_SYMBOL_SAME(futimens)
    RUDROID_SYMBOL_SAME(usleep)
    RUDROID_SYMBOL_SAME(sleep)
    RUDROID_SYMBOL_SAME(sched_yield)
    RUDROID_SYMBOL_SAME(sched_getaffinity)
    RUDROID_SYMBOL_SAME(sched_setaffinity)
    RUDROID_SYMBOL("tcgetattr", guest_tcgetattr)
    RUDROID_SYMBOL("tcsetattr", guest_tcsetattr)
    RUDROID_SYMBOL("tcflush", guest_tcflush)
    RUDROID_SYMBOL_SAME(getauxval)
    RUDROID_SYMBOL_SAME(getpagesize)
    RUDROID_SYMBOL_SAME(mmap)
    RUDROID_SYMBOL("mmap64", mmap)
    RUDROID_SYMBOL_SAME(munmap)
    RUDROID_SYMBOL_SAME(mprotect)
    RUDROID_SYMBOL_SAME(madvise)
    RUDROID_SYMBOL_SAME(mincore)
    RUDROID_SYMBOL_SAME(pipe)
    RUDROID_SYMBOL_SAME(poll)
    RUDROID_SYMBOL_SAME(isatty)
    RUDROID_SYMBOL_SAME(gethostname)
    RUDROID_SYMBOL_SAME(waitpid)
    RUDROID_SYMBOL_SAME(strcasecmp)
    RUDROID_SYMBOL_SAME(strncasecmp)
    RUDROID_SYMBOL_SAME(strcasestr)
    RUDROID_SYMBOL_SAME(strcspn)
    RUDROID_SYMBOL_SAME(strspn)
    RUDROID_SYMBOL_SAME(strpbrk)
    RUDROID_SYMBOL_SAME(rand)
    RUDROID_SYMBOL_SAME(srand)
    RUDROID_SYMBOL_SAME(llabs)
    RUDROID_SYMBOL("tmpnam", guest_tmpnam)
    RUDROID_SYMBOL_SAME(memmem)
    RUDROID_SYMBOL_SAME(perror)
    RUDROID_SYMBOL_SAME(strtok)
    RUDROID_SYMBOL_SAME(strtok_r)
    RUDROID_SYMBOL_SAME(strerror)
    RUDROID_SYMBOL_SAME(strtold)
    RUDROID_SYMBOL_SAME(wcslen)
    RUDROID_SYMBOL_SAME(wmemchr)
    RUDROID_SYMBOL_SAME(wmemcmp)
    RUDROID_SYMBOL_SAME(wmemcpy)
    RUDROID_SYMBOL_SAME(wmemmove)
    RUDROID_SYMBOL_SAME(wmemset)
    RUDROID_SYMBOL_SAME(wcstol)
    RUDROID_SYMBOL_SAME(wcstoll)
    RUDROID_SYMBOL_SAME(wcstoul)
    RUDROID_SYMBOL_SAME(wcstoull)
    RUDROID_SYMBOL_SAME(wcstof)
    RUDROID_SYMBOL_SAME(wcstod)
    RUDROID_SYMBOL_SAME(wcstold)
    RUDROID_SYMBOL_SAME(swprintf)
    RUDROID_SYMBOL_SAME(btowc)
    RUDROID_SYMBOL_SAME(wctob)
    RUDROID_SYMBOL_SAME(sprintf)
    RUDROID_SYMBOL_SAME(syslog)
    RUDROID_SYMBOL_SAME(vsyslog)
    RUDROID_SYMBOL_SAME(closelog)
    RUDROID_SYMBOL("sysconf", bridge_sysconf)
    RUDROID_SYMBOL("strerror_r", bridge_strerror_r)
    RUDROID_SYMBOL("__gnu_strerror_r", bridge_gnu_strerror_r)
    RUDROID_SYMBOL("ioctl", rudroid_bionic_ioctl)
    RUDROID_SYMBOL("pathconf", guest_pathconf)
    RUDROID_SYMBOL("fpathconf", guest_fpathconf)
    RUDROID_SYMBOL("__system_property_get", property_get)
    RUDROID_SYMBOL("__system_property_find", property_find)
    RUDROID_SYMBOL("__system_property_find_nth", property_find_nth)
    RUDROID_SYMBOL("__system_property_read", property_read)
    RUDROID_SYMBOL_SAME(inotify_init)
    RUDROID_SYMBOL("inotify_add_watch", guest_inotify_add_watch)
};
RUDROID_SYMBOL_PROVIDER(posix, symbols)
