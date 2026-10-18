// INPUT:  (none)
// OUTPUT:  rudroid_symtab_exported (.dynsym), rudroid_symtab_hidden and rudroid_symtab_tls (.symtab only)
// POS:    so_symbol_all comparison fixture: the hidden function stays only in the unstripped .symtab, verifying the .symtab fallback after a dynsym miss
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
__attribute__((visibility("hidden"))) __thread int rudroid_symtab_tls = 3;
__attribute__((visibility("hidden"))) int rudroid_symtab_hidden(void) { return 5150; }
int rudroid_symtab_exported(void) { return rudroid_symtab_hidden() + 1; }
