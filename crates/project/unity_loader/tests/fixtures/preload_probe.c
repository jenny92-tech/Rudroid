// INPUT:  rudroid_u8t_ready from the second preload
// OUTPUT: rudroid_preload_probe
// POS:    preload-order fixture: the normal module reads the completed chain
extern int rudroid_u8t_ready;

int rudroid_preload_probe(void) {
    return rudroid_u8t_ready;
}
