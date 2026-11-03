// INPUT:  versioned Bionic allocation imports resolved to the shared host C heap
// OUTPUT:  rudroid_allocation_probe returns zero after allocation/reallocation/alignment/free
// POS:    ARM64 guest execution probe; the link-only stub provider is never deployed
typedef unsigned long size_t;
extern void *malloc(size_t);
extern void *calloc(size_t, size_t);
extern void *realloc(void *, size_t);
extern void free(void *);
extern int posix_memalign(void **, size_t, size_t);
extern void *memalign(size_t, size_t);
extern void *aligned_alloc(size_t, size_t);
extern size_t malloc_usable_size(void *);

int rudroid_allocation_probe(void *host_pointer) {
    volatile unsigned char *p = calloc(8, 8);
    if (!p) return 1;
    for (size_t i = 0; i < 64; ++i) {
        if (p[i] != 0) { free((void *)p); return 2; }
        p[i] = 0x5a;
    }
    void *grown = realloc((void *)p, 256);
    if (!grown) { free((void *)p); return 3; }
    p = grown;
    for (size_t i = 0; i < 64; ++i) {
        if (p[i] != 0x5a) { free(grown); return 4; }
    }
    if (malloc_usable_size(grown) < 256) { free(grown); return 5; }
    free(grown);
    void *aligned = 0;
    if (posix_memalign(&aligned, 64, 128) != 0 || !aligned) return 6;
    if ((size_t)aligned % 64) { free(aligned); return 7; }
    free(aligned);
    aligned = memalign(64, 128);
    if (!aligned) return 8;
    if ((size_t)aligned % 64) { free(aligned); return 9; }
    free(aligned);
    aligned = aligned_alloc(64, 128);
    if (!aligned) return 10;
    if ((size_t)aligned % 64) { free(aligned); return 11; }
    free(aligned);
    void *block = malloc(32);
    if (!block) return 12;
    free(block);
    free(0);
    // Demonstrate that the guest free belongs to the host's native heap.
    free(host_pointer);
    return 0;
}
