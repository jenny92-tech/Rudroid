// INPUT:  (none)
// OUTPUT:  rudroid_slot0, rudroid_slot1
// POS:    .init_array execution fixture: the constructor turns 40 into 41 at load time, verifying that exec loading runs constructors and file-backed mappings
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
int rudroid_slots[2] = {40, 0};
__attribute__((constructor)) static void rudroid_ctor(void) {
    rudroid_slots[1] = rudroid_slots[0] + 1;
}
int rudroid_slot0(void) { return rudroid_slots[0]; }
int rudroid_slot1(void) { return rudroid_slots[1]; }
