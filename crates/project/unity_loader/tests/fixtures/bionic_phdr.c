// INPUT:  versioned libdl iterator and shared guest ELF metadata
// OUTPUT:  rudroid_phdr_probe checks constructor visibility, reentry and scoped libdl services
// POS:    guest queries the same core image table as relocation and symbol lookup
typedef unsigned long Size;
typedef struct {
    unsigned type,flags;
    Size offset,vaddr,paddr,filesz,memsz,align;
} Phdr;
typedef struct { Size base; const char *name; const Phdr *phdr; unsigned short count; } Info;
extern int dl_iterate_phdr(int (*)(Info *,Size,void *),void *);
typedef struct { const char *name; void *base; const char *symbol; void *address; } DlInfo;
extern void *dlopen(const char *,int);
extern int dlclose(void *);
extern void *dlsym(void *,const char *);
extern char *dlerror(void);
extern int dladdr(const void *,DlInfo *);
extern char *strstr(const char *,const char *);
static int constructor_seen;
static int locate(Info *info,Size size,void *opaque) {
    if (size!=32 || !info->name || !info->phdr) return 93;
    if (!strstr(info->name,"guest.so")) return 0;
    int *seen=opaque;
    Size marker=(Size)&constructor_seen;
    for (unsigned i=0;i<info->count;i++) {
        const Phdr *p=&info->phdr[i];
        if (p->type==1 && marker>=info->base+p->vaddr && marker-info->base-p->vaddr<p->memsz) {
            *seen+=1;
            return 17;
        }
    }
    return 94;
}
__attribute__((constructor)) static void constructor(void) {
    if (dl_iterate_phdr(locate,&constructor_seen)!=17) constructor_seen=-100;
}
static int nested(Info *info,Size size,void *opaque) {
    if (size!=32 || !info->name) return 95;
    if (!strstr(info->name,"guest.so")) return 0;
    int found=0;
    if (dl_iterate_phdr(locate,&found)!=17 || found!=1) return 96;
    *(int *)opaque+=1;
    return 23;
}
int rudroid_phdr_probe(void) {
    if (constructor_seen!=1) return 1;
    int found=0;
    if (dl_iterate_phdr(locate,&found)!=17 || found!=1) return 2;
    found=0;
    if (dl_iterate_phdr(nested,&found)!=23 || found!=1) return 3;
    void *a=dlopen("guest.so",2),*b=dlopen("guest.so",4);
    if (!a || a!=b || dlsym(a,"rudroid_phdr_probe")!=(void *)&rudroid_phdr_probe) return 4;
    DlInfo info;
    if (dladdr((void *)&rudroid_phdr_probe,&info)!=1 || !strstr(info.name,"guest.so") || !info.base) return 5;
    if (dlsym((void *)0,"rudroid_phdr_probe")!=(void *)&rudroid_phdr_probe) return 6;
    if (dlclose(a)!=0 || !dlsym(b,"rudroid_phdr_probe") || dlclose(b)!=0) return 7;
    if (dlclose(b)!=-1 || !dlerror() || dlerror()) return 8;
    if (dlopen("never-loaded.so",2) || !dlerror() || dlerror()) return 9;
    return 0;
}
