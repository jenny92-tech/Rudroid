// INPUT:  rudroid86_value, rudroid86_override exported by rudroid86_provider
// OUTPUT:  rudroid86_read, rudroid86_read_override
// POS:    Ticket 86 consumer fixture: imports data and functions from the previously loaded provider
// Ticket 86 fixture: imports data (rudroid86_value) and a function
// (rudroid86_override) from the previously loaded provider — the production
// exec chain must resolve both.
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
extern int rudroid86_value;
extern int rudroid86_override(void);
int rudroid86_read(void) { return rudroid86_value; }
int rudroid86_read_override(void) { return rudroid86_override(); }
