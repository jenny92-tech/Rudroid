// INPUT:  (none) — freestanding C, no libc dependency
// OUTPUT:  rudroid_swappy_call (.dynsym) and hidden SwappyGL_getSwapIntervalNS (.symtab only)
// POS:    so_symbol_all comparison fixture for the Swappy invalid-interval fallback: the target lives only in the unstripped .symtab and always returns UINT64_MAX
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Built with `-O0` so the exported caller keeps a real branch into the
// hidden function: once the profile detours it, `rudroid_swappy_call` observes
// the fallback interval instead of UINT64_MAX.
__attribute__((visibility("hidden"))) unsigned long long SwappyGL_getSwapIntervalNS(void) {
    return ~0ULL;
}

unsigned long long rudroid_swappy_call(void) {
    return SwappyGL_getSwapIntervalNS();
}
