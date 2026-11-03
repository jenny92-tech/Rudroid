// INPUT:  rudroid86_override exported by rudroid86_provider
// OUTPUT:  rudroid86_ctor_get
// POS:    Ticket 86 constructor fixture: an .init_array constructor calls the provider at load time
// Ticket 86 fixture: the consumer's CONSTRUCTOR imports from the provider —
// constructor-time imports must resolve in load order (the constructor runs
// during exec_load, before any exported call).
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
extern int rudroid86_override(void);
static int rudroid86_ctor_result;
__attribute__((constructor)) static void rudroid86_ctor(void) {
    rudroid86_ctor_result = rudroid86_override();
}
int rudroid86_ctor_get(void) { return rudroid86_ctor_result; }
