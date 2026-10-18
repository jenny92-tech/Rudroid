/* INPUT:  pinned upstream wsialloc v3 header and the ARM64 GNU ABI
 * OUTPUT:  compile-only size/offset gate shared with the Rust allocator tests
 * POS:    checks the external allocator ABI before linking the optional WSI layer
 */
#include <stddef.h>
#include "wsialloc.h"
_Static_assert(sizeof(void *) == 8, "ARM64 pointer ABI");
_Static_assert(WSIALLOC_INTERFACE_VERSION == 3, "wsialloc interface");
_Static_assert(sizeof(wsialloc_format) == 24, "format layout");
_Static_assert(sizeof(wsialloc_allocate_info) == 32, "input layout");
_Static_assert(offsetof(wsialloc_allocate_info, flags) == 24, "input flags");
_Static_assert(sizeof(wsialloc_allocate_result) == 80, "result layout");
_Static_assert(offsetof(wsialloc_allocate_result, buffer_fds) == 56, "fd ownership slots");
_Static_assert(offsetof(wsialloc_allocate_result, is_disjoint) == 72, "result bool");
