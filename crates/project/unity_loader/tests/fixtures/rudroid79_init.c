// INPUT:  (none) — freestanding C, no libc dependency (-nostdlib)
// OUTPUT:  rudroid_slots, rudroid_slot0/rudroid_slot1 and one .init_array constructor
// POS:    Ticket 79/84 constructor fixture: rudroid_slot1() returns 41 only after the constructor ran at load time
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
/* Ticket 79 contract fixture: .init_array constructor writes an observable
 * value; rudroid_slot1() returns 41 only if the constructor ran during load. */
int rudroid_slots[2] = {40, 0};
__attribute__((constructor)) static void rudroid_ctor(void) {
    rudroid_slots[1] = rudroid_slots[0] + 1;
}
int rudroid_slot0(void) { return rudroid_slots[0]; }
int rudroid_slot1(void) { return rudroid_slots[1]; }
