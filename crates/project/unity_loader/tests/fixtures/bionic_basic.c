// INPUT:  versioned Android LP64 C pointer/scalar APIs and host-backed errno
// OUTPUT:  rudroid_basic_probe exercises memory/strings/conversion/callback/TLS ABI
// POS:    runs real guest instructions against shared host libc, without libc headers
typedef unsigned long size_t;
extern void *memcpy(void *, const void *, size_t);
extern void *memmove(void *, const void *, size_t);
extern void *memset(void *, int, size_t);
extern int memcmp(const void *, const void *, size_t);
extern size_t strlen(const char *);
extern size_t strlcpy(char *, const char *, size_t);
extern size_t strlcat(char *, const char *, size_t);
extern char *strdup(const char *);
extern char *strndup(const char *, size_t);
extern void free(void *);
extern int strcmp(const char *, const char *);
extern long strtol(const char *, char **, int);
extern double strtod(const char *, char **);
extern int *__errno(void);
extern int gettid(void);
extern long syscall(long, ...);
extern void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
extern void *bsearch(const void *, const void *, size_t, size_t, int (*)(const void *, const void *));
static int compare(const void *a, const void *b) {
    int left = *(const int *)a, right = *(const int *)b;
    return (left > right) - (left < right);
}
int rudroid_basic_probe(void) {
    unsigned char memory[8];
    memset(memory, 0x5b, sizeof memory);
    if (memcpy(memory + 1, "abc", 3) != memory + 1) return 1;
    if (memmove(memory + 2, memory + 1, 3) != memory + 2) return 2;
    if (memcmp(memory, "[aabc[[[", 8) != 0) return 3;
    if (strlcpy((char *)memory + 1, "123456", 4) != 6) return 4;
    if (memcmp(memory, "[123\0[[[", 8) != 0) return 5;
    if (strlcat((char *)memory + 1, "xy", 4) != 5) return 6;
    if (strlen((char *)memory + 1) != 3) return 7;
    char *copy = strdup("shared");
    if (!copy || strcmp(copy, "shared") != 0) return 8;
    free(copy);
    copy = strndup("shared", 3);
    if (!copy || strcmp(copy, "sha") != 0) return 9;
    free(copy);
    char *end = 0;
    *__errno() = 0;
    if (strtol("9223372036854775808", &end, 10) != 9223372036854775807L || *__errno() != 34 || *end) return 10;
    if (strtol("123tail", &end, 10) != 123 || strcmp(end, "tail")) return 11;
    if (strtod("1.25tail", &end) != 1.25 || strcmp(end, "tail")) return 12;
    if (gettid() <= 0 || gettid() != syscall(178L)) return 13; // AArch64 __NR_gettid
    int numbers[] = {5, -1, 3, 2};
    qsort(numbers, 4, sizeof(int), compare);
    int expected[] = {-1, 2, 3, 5};
    if (memcmp(numbers, expected, sizeof numbers)) return 14;
    int key = 3;
    if (bsearch(&key, numbers, 4, sizeof(int), compare) != numbers + 2) return 15;
    return 0;
}
