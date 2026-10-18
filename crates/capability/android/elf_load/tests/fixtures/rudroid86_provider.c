// INPUT:  (none)
// OUTPUT:  rudroid86_value, rudroid86_override, rudroid86_get, rudroid86_call_own
// POS:    Ticket 86 provider fixture: exports data and functions and references them internally through GOT/PLT
// Ticket 86 fixture: the provider exports data AND a function the consumers
// import; rudroid86_override is the override-priority probe (guest export 42; a
// registered host source shadows other modules' IMPORTS of it with 777, while
// rudroid86_get/rudroid86_call_own — the provider's own references — keep binding
// to its definition, reference so_relocate_rel).
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
int rudroid86_value = 42;
int rudroid86_override(void) { return 42; }
int rudroid86_get(void) { return rudroid86_value; }
int rudroid86_call_own(void) { return rudroid86_override(); }
