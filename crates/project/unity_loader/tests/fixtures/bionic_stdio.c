// INPUT:  versioned stdio imports, Bionic LP64 FILE layout and AAPCS64 va_list
// OUTPUT:  rudroid_stdio_probe/standard_close_probe execute real native I/O through facades
// POS:    validates shared streams, wide I/O, buffer bounds, varargs and file lifetime in ARM64 guest
typedef unsigned long size_t;
typedef __builtin_va_list va_list;
typedef struct { unsigned char *base; size_t size; } Buffer;
typedef struct File {
    unsigned char *p; int r, w, flags, fd; Buffer bf; int lbfsize;
    void *cookie, *close, *read, *seek, *write; Buffer ext;
    unsigned char *up; int ur; unsigned char ubuf[3], nbuf[1];
    Buffer lb; int blksize; long unused;
} File;
_Static_assert(sizeof(File) == 152, "Bionic LP64 stride");
_Static_assert(__builtin_offsetof(File, cookie) == 48, "Bionic cookie offset");
_Static_assert(sizeof(va_list) == 32, "AAPCS64 va_list");
extern File __sF[3];
extern File *stdin, *stdout, *stderr;
extern File *tmpfile(void), *fopen(const char *, const char *), *fdopen(int, const char *);
extern File *fopen64(const char *, const char *);
extern int fclose(File *), fflush(File *), fileno(File *);
extern size_t fread(void *, size_t, size_t, File *), fwrite(const void *, size_t, size_t, File *);
extern int fseek(File *, long, int), fseeko64(File *, long, int);
extern long ftell(File *), ftello64(File *);
extern int fgetpos(File *, long *), fsetpos(File *, const long *);
extern int fgetc(File *), fputc(int, File *), ungetc(int, File *);
extern unsigned fgetwc(File *), fputwc(unsigned, File *), ungetwc(unsigned, File *);
extern int fwide(File *, int);
extern int fputs(const char *, File *), feof(File *), ferror(File *);
extern char *fgets(char *, int, File *);
extern void clearerr(File *), rewind(File *), setbuf(File *, char *);
extern int setvbuf(File *, char *, int, size_t);
extern void flockfile(File *), funlockfile(File *);
extern int ftrylockfile(File *);
extern int fprintf(File *, const char *, ...), fscanf(File *, const char *, ...);
extern int printf(const char *, ...), vprintf(const char *, va_list), puts(const char *), putchar(int);
extern int vfprintf(File *, const char *, va_list), vfscanf(File *, const char *, va_list);
extern int vsnprintf(char *, size_t, const char *, va_list), vsscanf(const char *, const char *, va_list);
extern int vasprintf(char **, const char *, va_list);
extern void free(void *);

static int print(File *f, const char *format, ...) {
    va_list args; __builtin_va_start(args, format);
    int n = vfprintf(f, format, args); __builtin_va_end(args); return n;
}
static int scan(File *f, const char *format, ...) {
    va_list args; __builtin_va_start(args, format);
    int n = vfscanf(f, format, args); __builtin_va_end(args); return n;
}
static int format(char *buffer, size_t size, const char *fmt, ...) {
    va_list args; __builtin_va_start(args, fmt);
    int n = vsnprintf(buffer, size, fmt, args); __builtin_va_end(args); return n;
}
static int parse(const char *text, const char *fmt, ...) {
    va_list args; __builtin_va_start(args, fmt);
    int n = vsscanf(text, fmt, args); __builtin_va_end(args); return n;
}
static int allocate(char **text, const char *fmt, ...) {
    va_list args; __builtin_va_start(args, fmt);
    int n = vasprintf(text, fmt, args); __builtin_va_end(args); return n;
}
static int output(const char *fmt, ...) {
    va_list args; __builtin_va_start(args, fmt);
    int n = vprintf(fmt, args); __builtin_va_end(args); return n;
}
static int equal(const char *a, const char *b) {
    while (*a && *a == *b) { ++a; ++b; } return *a == *b;
}
#define REQUIRE(condition, code) do { if (!(condition)) { fclose(file); return code; } } while (0)
int rudroid_stdio_probe(const char *path) {
    if (stdin != &__sF[0] || stdout != &__sF[1] || stderr != &__sF[2]) return 1;
    if (fileno(stdin) != 0 || fileno(stdout) != 1 || fileno(stderr) != 2) return 2;
    if (__sF[0].fd != 0 || __sF[1].fd != 1 || __sF[2].fd != 2) return 3;
    File *file = tmpfile();
    if (!file) return 4;
    REQUIRE(file->cookie != 0 && file->fd == fileno(file), 5);
    REQUIRE(fwrite("abc\n", 2, 2, file) == 2 && fflush(file) == 0 && ftell(file) == 4, 6);
    struct { long position; unsigned long canary; } pos = {0, 0x123456789abcdef0UL};
    REQUIRE(fgetpos(file, &pos.position) == 0 && pos.position == 4 && pos.canary == 0x123456789abcdef0UL, 7);
    rewind(file);
    char text[256];
    REQUIRE(fgets(text, sizeof(text), file) == text && equal(text, "abc\n"), 8);
    REQUIRE(fgetc(file) == -1 && feof(file) != 0 && ferror(file) == 0, 9);
    clearerr(file);
    REQUIRE(feof(file) == 0 && fsetpos(file, &pos.position) == 0 && ftell(file) == 4, 10);
    REQUIRE(fseek(file, 0, 0) == 0 && fgetc(file) == 'a' && ungetc('Z', file) == 'Z' && fgetc(file) == 'Z', 11);
    if (fclose(file) != 0) return 12; // success branch consumes file exactly once
    file = tmpfile(); if (!file) return 42;
    REQUIRE(fwide(file, 1) > 0 && fputwc(0x4e2d, file) == 0x4e2d && fflush(file) == 0, 43);
    rewind(file);
    REQUIRE(fgetwc(file) == 0x4e2d && ungetwc('Z', file) == 'Z' && fgetwc(file) == 'Z', 44);
    REQUIRE(ungetwc(~0u, file) == ~0u, 45);
    if (fclose(file) != 0) return 46;
    file = tmpfile(); if (!file) return 13;
    REQUIRE(fprintf(file, "%d %.2f %lld %s\n", 42, 1.25, 1234567890123LL, "ok") > 0, 14);
    rewind(file);
    int integer = 0; double floating = 0; long long wide = 0; char word[8];
    REQUIRE(fscanf(file, "%d %lf %lld %7s", &integer, &floating, &wide, word) == 4, 15);
    REQUIRE(integer == 42 && floating == 1.25 && wide == 1234567890123LL && equal(word, "ok"), 16);
    rewind(file);
    // Spill both GP and FP varargs to the stack, not just argument registers.
    REQUIRE(print(file, "%d %d %d %d %d %d %d %d %d %.1f %.1f %.1f %.1f %.1f %.1f %.1f %.1f %.1f",
        1,2,3,4,5,6,7,8,9, 1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0) > 0, 17);
    rewind(file);
    REQUIRE(fgets(text, sizeof(text), file) == text && equal(text, "1 2 3 4 5 6 7 8 9 1.0 2.0 3.0 4.0 5.0 6.0 7.0 8.0 9.0"), 18);
    rewind(file); REQUIRE(scan(file, "%d", &integer) == 1 && integer == 1, 19);
    REQUIRE(format(text, sizeof(text), "%d %.1f %s", 7, 2.5, "yes") == 9 && equal(text, "7 2.5 yes"), 20);
    REQUIRE(parse(text, "%d %lf %7s", &integer, &floating, word) == 3 && integer == 7 && floating == 2.5 && equal(word, "yes"), 21);
    char *allocated = 0;
    int n = allocate(&allocated, "%d %.1f", 7, 2.5);
    int good = n == 5 && allocated && equal(allocated, "7 2.5"); free(allocated);
    REQUIRE(good, 22);
    rewind(file);
    File *saved = stdout; stdout = file;
    good = printf("%d", 7) == 1 && output(" %.1f", 2.5) == 4 && puts(" yes") >= 0 && putchar('!') == '!';
    stdout = saved;
    REQUIRE(good, 39);
    rewind(file); REQUIRE(fread(text, 1, 11, file) == 11, 40);
    text[11] = 0; REQUIRE(equal(text, "7 2.5 yes\n!"), 41);
    flockfile(file); REQUIRE(ftrylockfile(file) == 0, 23); funlockfile(file); funlockfile(file);
    if (fclose(file) != 0) return 24;
    file = tmpfile(); if (!file) return 25;
    struct { char buffer[1024]; unsigned char guard[7168]; } backing;
    for (size_t i=0;i<sizeof(backing.guard);++i) backing.guard[i]=0x5a;
    setbuf(file, backing.buffer);
    for (int i=0;i<9000;++i) REQUIRE(fputc('q', file) == 'q', 26);
    REQUIRE(fflush(file) == 0, 27);
    for (size_t i=0;i<sizeof(backing.guard);++i) REQUIRE(backing.guard[i] == 0x5a, 28);
    if (fclose(file) != 0) return 29;
    file = fopen(path, "w+"); if (!file) return 30;
    REQUIRE(setvbuf(file, 0, 2, 0) == 0 && fputs("shared", file) >= 0, 31);
    REQUIRE(fseeko64(file, 0, 0) == 0 && ftello64(file) == 0 && fread(text, 1, 6, file) == 6, 32);
    text[6] = 0; REQUIRE(equal(text, "shared"), 33);
    if (fclose(file) != 0) return 34;
    file = fopen64(path, "r"); if (!file) return 35;
    REQUIRE(fputc('x', file) == -1 && ferror(file) != 0, 36);
    clearerr(file); REQUIRE(ferror(file) == 0, 37);
    if (fclose(file) != 0) return 38;
    return 0;
}
int rudroid_standard_close_probe(void) {
    File *old = stderr;
    if (fclose(stderr) != 0) return 1;
    if (stderr != old || stderr != &__sF[2]) return 2;
    if (fileno(stderr) != -1 || fclose(stderr) != -1) return 3;
    return 0;
}

int rudroid_fdopen_probe(int fd) {
    File *file = fdopen(fd, "w+");
    if (!file) return 1;
    int good = fileno(file) == fd && fwrite("owned", 1, 5, file) == 5;
    int closed = fclose(file);
    return good && closed == 0 ? 0 : 2;
}
