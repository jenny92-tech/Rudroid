// INPUT:  linker `-Wl,-init,rudroid_dt_init_both`
// OUTPUT: rudroid_dt_init_both_slot
// POS:    DT_INIT-before-DT_INIT_ARRAY ordering fixture
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
int rudroid_dt_init_both_state;

void rudroid_dt_init_both(void) {
    rudroid_dt_init_both_state = rudroid_dt_init_both_state * 10 + 1;
}

__attribute__((constructor)) static void rudroid_dt_init_both_array(void) {
    rudroid_dt_init_both_state = rudroid_dt_init_both_state * 10 + 2;
}

int rudroid_dt_init_both_slot(void) {
    return rudroid_dt_init_both_state;
}

/* Retired init-array entries may contain the all-ones sentinel. */
__asm__(".pushsection .init_array.00101,\"aw\",%init_array\n"
        ".balign 8\n.xword -1\n.popsection");
