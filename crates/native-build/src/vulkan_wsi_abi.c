/* INPUT:  pinned upstream wsialloc v3 header and the ARM64 GNU ABI
 * OUTPUT:  compile-only layout, v3 result and Linux resource errno gates
 * POS:    checks the external allocator ABI before linking the optional WSI layer
 */
#include <stdbool.h>
#include <stddef.h>
#include <errno.h>
#include <drm/drm_fourcc.h>
#include "wsialloc.h"
#ifdef __cplusplus
#define RUDROID_STATIC_ASSERT static_assert
#else
#define RUDROID_STATIC_ASSERT _Static_assert
#endif
RUDROID_STATIC_ASSERT(sizeof(void *) == 8, "ARM64 pointer ABI");
RUDROID_STATIC_ASSERT(sizeof(bool) == 1, "bool ABI");
RUDROID_STATIC_ASSERT(sizeof(wsialloc_error) == 4, "wsialloc_error ABI");
RUDROID_STATIC_ASSERT(WSIALLOC_MAX_PLANES == 4, "wsialloc plane slots");
RUDROID_STATIC_ASSERT(WSIALLOC_INTERFACE_VERSION == 3, "wsialloc interface");
RUDROID_STATIC_ASSERT(WSIALLOC_ERROR_INVALID == -1 && WSIALLOC_ERROR_NOT_SUPPORTED == -2 &&
                      WSIALLOC_ERROR_NO_RESOURCE == -3, "wsialloc failure codes");
RUDROID_STATIC_ASSERT(EAGAIN == 11 && ENOMEM == 12 && ENFILE == 23 && EMFILE == 24 && ENOSPC == 28,
                      "Rust allocator Linux resource errno classification");
RUDROID_STATIC_ASSERT(DRM_FORMAT_ABGR8888 == 0x34324241 && DRM_FORMAT_ARGB8888 == 0x34325241 &&
                      DRM_FORMAT_XBGR8888 == 0x34324258 && DRM_FORMAT_XRGB8888 == 0x34325258 &&
                      DRM_FORMAT_MOD_INVALID == 0x00ffffffffffffffULL, "Rust allocator DRM constants");
RUDROID_STATIC_ASSERT(sizeof(wsialloc_format) == 24, "format layout");
RUDROID_STATIC_ASSERT(offsetof(wsialloc_format, modifier) == 8 && offsetof(wsialloc_format, flags) == 16, "format fields");
RUDROID_STATIC_ASSERT(sizeof(wsialloc_allocate_info) == 32, "input layout");
RUDROID_STATIC_ASSERT(offsetof(wsialloc_allocate_info, format_count) == 8 &&
                      offsetof(wsialloc_allocate_info, width) == 12 &&
                      offsetof(wsialloc_allocate_info, height) == 16 &&
                      offsetof(wsialloc_allocate_info, flags) == 24, "input fields");
RUDROID_STATIC_ASSERT(sizeof(wsialloc_allocate_result) == 80, "result layout");
RUDROID_STATIC_ASSERT(offsetof(wsialloc_allocate_result, average_row_strides) == 24 &&
                      offsetof(wsialloc_allocate_result, offsets) == 40 &&
                      offsetof(wsialloc_allocate_result, buffer_fds) == 56 &&
                      offsetof(wsialloc_allocate_result, is_disjoint) == 72, "result fields");
