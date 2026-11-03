// INPUT:  fixed Android ARM64 layouts, shared Rust asset resolver and native POSIX
// OUTPUT:  rudroid_bionic_posix_symbol/_symbol_name table, asset I/O and sysconf adapters
// POS:    exposes native services with one shared extracted-asset path boundary
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <fnmatch.h>
#include <libgen.h>
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
typedef struct {
    unsigned long st_dev, st_ino;
    unsigned st_mode, st_nlink, st_uid, st_gid;
    unsigned long st_rdev, pad1;
    long st_size;
    int st_blksize, pad2;
    long st_blocks;
    struct { long tv_sec, tv_nsec; } st_atim, st_mtim, st_ctim;
    unsigned unused4, unused5;
} AndroidStat;
typedef struct {
    unsigned long d_ino;
    long d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[256];
} AndroidDirent;
typedef struct {
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;
    const char *tm_zone;
} AndroidTm;
#define LAYOUT(model, native) \
    _Static_assert(sizeof(model) == sizeof(native), "Android/native size"); \
    _Static_assert(_Alignof(model) == _Alignof(native), "Android/native alignment")
#define FIELD(model, native, field) \
    _Static_assert(offsetof(model, field) == offsetof(native, field), "Android/native offset"); \
    _Static_assert(sizeof(((model *)0)->field) == sizeof(((native *)0)->field), "Android/native field width")
LAYOUT(AndroidStat, struct stat);
LAYOUT(AndroidDirent, struct dirent);
LAYOUT(AndroidDirent, struct dirent64);
LAYOUT(AndroidTm, struct tm);
FIELD(AndroidStat, struct stat, st_dev);
FIELD(AndroidStat, struct stat, st_ino);
FIELD(AndroidStat, struct stat, st_mode);
FIELD(AndroidStat, struct stat, st_nlink);
FIELD(AndroidStat, struct stat, st_uid);
FIELD(AndroidStat, struct stat, st_gid);
FIELD(AndroidStat, struct stat, st_rdev);
FIELD(AndroidStat, struct stat, st_size);
FIELD(AndroidStat, struct stat, st_blksize);
FIELD(AndroidStat, struct stat, st_blocks);
FIELD(AndroidStat, struct stat, st_atim);
FIELD(AndroidStat, struct stat, st_mtim);
FIELD(AndroidStat, struct stat, st_ctim);
FIELD(AndroidDirent, struct dirent, d_ino);
FIELD(AndroidDirent, struct dirent64, d_ino);
FIELD(AndroidDirent, struct dirent, d_off);
FIELD(AndroidDirent, struct dirent64, d_off);
FIELD(AndroidDirent, struct dirent, d_reclen);
FIELD(AndroidDirent, struct dirent64, d_reclen);
FIELD(AndroidDirent, struct dirent, d_type);
FIELD(AndroidDirent, struct dirent64, d_type);
FIELD(AndroidDirent, struct dirent, d_name);
FIELD(AndroidDirent, struct dirent64, d_name);
FIELD(AndroidTm, struct tm, tm_sec);
FIELD(AndroidTm, struct tm, tm_min);
FIELD(AndroidTm, struct tm, tm_hour);
FIELD(AndroidTm, struct tm, tm_mday);
FIELD(AndroidTm, struct tm, tm_mon);
FIELD(AndroidTm, struct tm, tm_year);
FIELD(AndroidTm, struct tm, tm_wday);
FIELD(AndroidTm, struct tm, tm_yday);
FIELD(AndroidTm, struct tm, tm_isdst);
FIELD(AndroidTm, struct tm, tm_gmtoff);
FIELD(AndroidTm, struct tm, tm_zone);
_Static_assert(sizeof(time_t) == 8 && sizeof(off_t) == 8 && sizeof(off64_t) == 8, "LP64 time/offset");
_Static_assert(sizeof(wchar_t) == 4 && sizeof(wint_t) == 4 && sizeof(long double) == 16, "wide/FP128 ABI");
_Static_assert(sizeof(struct timespec) == 16 && offsetof(struct timespec, tv_nsec) == 8, "timespec ABI");
_Static_assert(sizeof(struct timeval) == 16 && offsetof(struct timeval, tv_usec) == 8, "timeval ABI");
_Static_assert(sizeof(struct pollfd) == 8 && offsetof(struct pollfd, revents) == 6, "pollfd ABI");
#undef FIELD
#undef LAYOUT

typedef struct {
    unsigned long f_bsize, f_frsize, f_blocks, f_bfree, f_bavail;
    unsigned long f_files, f_ffree, f_favail, f_fsid, f_flag, f_namemax;
    uint32_t reserved[6];
} AndroidStatvfs;
_Static_assert(sizeof(AndroidStatvfs) == sizeof(struct statvfs) &&
    _Alignof(AndroidStatvfs) == _Alignof(struct statvfs), "statvfs layout ABI");
#define FIELD(f) _Static_assert(offsetof(AndroidStatvfs, f) == offsetof(struct statvfs, f) && \
    sizeof(((AndroidStatvfs *)0)->f) == sizeof(((struct statvfs *)0)->f), "statvfs field ABI")
FIELD(f_bsize); FIELD(f_frsize); FIELD(f_blocks); FIELD(f_bfree); FIELD(f_bavail);
FIELD(f_files); FIELD(f_ffree); FIELD(f_favail); FIELD(f_fsid); FIELD(f_flag); FIELD(f_namemax);
#undef FIELD

// Public Linux ARM64 structures match Android only after these layout gates.
typedef struct {
    unsigned long f_type,f_bsize,f_blocks,f_bfree,f_bavail,f_files,f_ffree;
    int f_fsid[2]; unsigned long f_namelen,f_frsize,f_flags,f_spare[4];
} AndroidStatfs;
typedef struct {
    char *pw_name,*pw_passwd; unsigned pw_uid,pw_gid; char *pw_gecos,*pw_dir,*pw_shell;
} AndroidPasswd;
typedef struct { char sysname[65],nodename[65],release[65],version[65],machine[65],domainname[65]; } AndroidUtsname;
#define FIELD(model,native,f) _Static_assert(offsetof(model,f)==offsetof(native,f) && \
    sizeof(((model *)0)->f)==sizeof(((native *)0)->f), "native public field ABI")
_Static_assert(sizeof(AndroidStatfs)==sizeof(struct statfs) && sizeof(AndroidPasswd)==sizeof(struct passwd) &&
    sizeof(AndroidUtsname)==sizeof(struct utsname), "native public structure ABI");
FIELD(AndroidStatfs,struct statfs,f_type); FIELD(AndroidStatfs,struct statfs,f_bsize);
FIELD(AndroidStatfs,struct statfs,f_blocks); FIELD(AndroidStatfs,struct statfs,f_bfree);
FIELD(AndroidStatfs,struct statfs,f_bavail); FIELD(AndroidStatfs,struct statfs,f_files);
FIELD(AndroidStatfs,struct statfs,f_ffree); FIELD(AndroidStatfs,struct statfs,f_fsid);
FIELD(AndroidStatfs,struct statfs,f_namelen); FIELD(AndroidStatfs,struct statfs,f_frsize);
FIELD(AndroidStatfs,struct statfs,f_flags); FIELD(AndroidStatfs,struct statfs,f_spare);
FIELD(AndroidPasswd,struct passwd,pw_name); FIELD(AndroidPasswd,struct passwd,pw_passwd);
FIELD(AndroidPasswd,struct passwd,pw_uid); FIELD(AndroidPasswd,struct passwd,pw_gid);
FIELD(AndroidPasswd,struct passwd,pw_gecos); FIELD(AndroidPasswd,struct passwd,pw_dir);
FIELD(AndroidPasswd,struct passwd,pw_shell);
FIELD(AndroidUtsname,struct utsname,sysname); FIELD(AndroidUtsname,struct utsname,nodename);
FIELD(AndroidUtsname,struct utsname,release); FIELD(AndroidUtsname,struct utsname,version);
FIELD(AndroidUtsname,struct utsname,machine); FIELD(AndroidUtsname,struct utsname,domainname);
#undef FIELD
_Static_assert(sizeof(struct flock)==32 && offsetof(struct flock,l_start)==8 &&
    offsetof(struct flock,l_len)==16 && offsetof(struct flock,l_pid)==24 &&
    F_GETLK==5 && F_SETLK==6 && F_SETLKW==7, "ARM64 fcntl locking ABI");
_Static_assert(FNM_PATHNAME==1 && FNM_NOESCAPE==2 && FNM_PERIOD==4 &&
    FNM_LEADING_DIR==8 && FNM_CASEFOLD==16, "fnmatch flags ABI");

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
static DIR *guest_opendir(const char *path) { ASSET_CALL(DIR *, opendir(native_path)); }
static int guest_access(const char *path, int mode) { ASSET_CALL(int, access(native_path, mode)); }
static int guest_stat(const char *path, struct stat *out) { ASSET_CALL(int, stat(native_path, out)); }
static int guest_lstat(const char *path, struct stat *out) { ASSET_CALL(int, lstat(native_path, out)); }
static int guest_fstatat(int dir, const char *path, struct stat *out, int flags) {
    ASSET_CALL(int, fstatat(dir, native_path, out, flags));
}
#undef ASSET_CALL
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("environ", &environ)
    RUDROID_SYMBOL_SAME(prctl) RUDROID_SYMBOL_SAME(ptrace)
    RUDROID_SYMBOL_SAME(fcntl)
    RUDROID_SYMBOL_SAME(flock)
    RUDROID_SYMBOL_SAME(fnmatch)
    RUDROID_SYMBOL_SAME(basename)
    RUDROID_SYMBOL_SAME(getpriority)
    RUDROID_SYMBOL_SAME(setpriority)
    RUDROID_SYMBOL_SAME(getpwuid)
    RUDROID_SYMBOL_SAME(getpwuid_r)
    RUDROID_SYMBOL_SAME(uname)
    RUDROID_SYMBOL_SAME(div)
    RUDROID_SYMBOL_SAME(ldiv)
    RUDROID_SYMBOL_SAME(lldiv)
    RUDROID_SYMBOL_SAME(drand48)
    RUDROID_SYMBOL_SAME(erand48)
    RUDROID_SYMBOL_SAME(lrand48)
    RUDROID_SYMBOL_SAME(nrand48)
    RUDROID_SYMBOL_SAME(srand48)
    RUDROID_SYMBOL_SAME(seed48)
    RUDROID_SYMBOL_SAME(statfs)
    RUDROID_SYMBOL_SAME(fstatfs)
    RUDROID_SYMBOL_SAME(statfs64)
    RUDROID_SYMBOL_SAME(fstatfs64)
    RUDROID_SYMBOL_SAME(execl)
    RUDROID_SYMBOL("access", guest_access)
    RUDROID_SYMBOL_SAME(chdir)
    RUDROID_SYMBOL_SAME(getcwd)
    RUDROID_SYMBOL_SAME(close)
    RUDROID_SYMBOL_SAME(dup)
    RUDROID_SYMBOL_SAME(dup2)
    RUDROID_SYMBOL_SAME(read)
    RUDROID_SYMBOL_SAME(write)
    RUDROID_SYMBOL_SAME(pread)
    RUDROID_SYMBOL_SAME(pwrite)
    RUDROID_SYMBOL_SAME(lseek)
    RUDROID_SYMBOL_SAME(lseek64)
    RUDROID_SYMBOL("open", guest_open)
    RUDROID_SYMBOL("openat", guest_openat)
    RUDROID_SYMBOL_SAME(chmod)
    RUDROID_SYMBOL_SAME(fchmod)
    RUDROID_SYMBOL_SAME(fchmodat)
    RUDROID_SYMBOL_SAME(truncate)
    RUDROID_SYMBOL_SAME(ftruncate)
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
    RUDROID_SYMBOL_SAME(perror)
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
    RUDROID_SYMBOL("__system_property_read", property_read)
};
RUDROID_SYMBOL_PROVIDER(posix, symbols)
