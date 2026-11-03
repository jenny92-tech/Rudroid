// INPUT:  (none)
// OUTPUT:  rudroid86_tls (STT_TLS), rudroid86_plain
// POS:    Ticket 86 TLS fixture: exports a TLS variable not referenced by code, to verify symbol lookup precisely rejects TLS
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
__thread int rudroid86_tls = 5;
int rudroid86_plain(void) { return 7; }
