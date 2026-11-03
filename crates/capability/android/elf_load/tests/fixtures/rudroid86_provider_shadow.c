// INPUT:  (none)
// OUTPUT:  rudroid86_value, rudroid86_override
// POS:    Ticket 86 second provider fixture: same-named exports as rudroid86_provider, to verify the newest load takes precedence in the image chain
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
int rudroid86_value = 99;
int rudroid86_override(void) { return 99; }
