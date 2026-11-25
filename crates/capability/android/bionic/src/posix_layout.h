/* INPUT: ARM64 Bionic layout records and native Linux headers from posix.c
 * OUTPUT: compile-time field, size, alignment and constant assertions
 * POS: ABI preconditions for the POSIX compatibility provider */
#ifndef RUDROID_BIONIC_POSIX_LAYOUT_H
#define RUDROID_BIONIC_POSIX_LAYOUT_H
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

#endif
