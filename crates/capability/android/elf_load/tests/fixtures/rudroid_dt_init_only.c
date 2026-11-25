// INPUT:  linker `-Wl,-init,rudroid_dt_init_only`
// OUTPUT: rudroid_dt_init_only_slot
// POS:    DT_INIT-only execution fixture; there must be no DT_INIT_ARRAY
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
int rudroid_dt_init_only_state;

void rudroid_dt_init_only(void) {
    rudroid_dt_init_only_state = 42;
}

int rudroid_dt_init_only_slot(void) {
    return rudroid_dt_init_only_state;
}
