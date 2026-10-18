// INPUT:  native zlib.h with ARM64 LP64 stream and callback layout
// OUTPUT:  rudroid_native_zlib_symbol for explicit native zlib compression/checksum functions
// POS:    zero-marshalling guest ABI boundary with no extra stream, buffer, cache or allocator
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <zlib.h>
_Static_assert(sizeof(void *)==8 && sizeof(uInt)==4 && sizeof(uLong)==8,"ARM64 zlib scalar ABI");
_Static_assert(sizeof(z_off_t)==8 && sizeof(z_crc_t)==4,"ARM64 zlib offset/CRC ABI");
_Static_assert(sizeof(z_stream)==112 && _Alignof(z_stream)==8,"Android LP64 z_stream ABI");
#define FIELD(n,o,s) _Static_assert(offsetof(z_stream,n)==o && sizeof(((z_stream *)0)->n)==s,"z_stream." #n)
FIELD(next_in,0,8);
FIELD(avail_in,8,4);
FIELD(total_in,16,8);
FIELD(next_out,24,8);
FIELD(avail_out,32,4);
FIELD(total_out,40,8);
FIELD(msg,48,8);
FIELD(state,56,8);
FIELD(zalloc,64,8);
FIELD(zfree,72,8);
FIELD(opaque,80,8);
FIELD(data_type,88,4);
FIELD(adler,96,8);
FIELD(reserved,104,8);
#undef FIELD
// Type checks make allocator/free callback declarations part of the gate.
typedef void *(*GuestAlloc)(void *,unsigned int,unsigned int);
typedef void (*GuestFree)(void *,void *);
_Static_assert(__builtin_types_compatible_p(alloc_func,GuestAlloc),"zalloc prototype");
_Static_assert(__builtin_types_compatible_p(free_func,GuestFree),"zfree prototype");
uintptr_t rudroid_native_zlib_symbol(const char *name) {
#define SYMBOL(f) if (!strcmp(name,#f)) return (uintptr_t)&f
    SYMBOL(zlibVersion);
    SYMBOL(inflate);
    SYMBOL(inflateInit_);
    SYMBOL(inflateInit2_);
    SYMBOL(inflateEnd);
    SYMBOL(inflateReset);
    SYMBOL(inflateSync);
    SYMBOL(inflateCopy);
    SYMBOL(inflateSetDictionary);
    SYMBOL(deflate);
    SYMBOL(deflateInit_);
    SYMBOL(deflateInit2_);
    SYMBOL(deflateEnd);
    SYMBOL(deflateReset);
    SYMBOL(deflateCopy);
    SYMBOL(deflateSetDictionary);
    SYMBOL(deflateParams);
    SYMBOL(deflateBound);
    SYMBOL(compress);
    SYMBOL(compress2);
    SYMBOL(compressBound);
    SYMBOL(uncompress);
    SYMBOL(crc32);
    SYMBOL(crc32_combine);
    SYMBOL(adler32);
    SYMBOL(adler32_combine);
    SYMBOL(zError);
    SYMBOL(get_crc_table);
#undef SYMBOL
    return 0;
}
