// INPUT:  glRudroid / glad_glRudroid provided by a host source
// OUTPUT:  rudroid86_gl_read
// POS:    Ticket 86 glad_ rewrite fixture: imports a GL entry-point name
// Ticket 86 fixture: the glad_ rewrite rule — the guest imports glRudroid;
// the thunk layer registers glad_glRudroid, and the layered lookup resolves
// the rewritten name (reference so_resolve_link behavior).
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
extern int glRudroid(void);
int rudroid86_gl_read(void) { return glRudroid(); }
