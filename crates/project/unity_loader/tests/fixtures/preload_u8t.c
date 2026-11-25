// INPUT:  rudroid_anort_ready from the earlier preload
// OUTPUT: rudroid_u8t_ready
// POS:    preload-order fixture: the second constructor observes its provider
extern int rudroid_anort_ready;
int rudroid_u8t_ready;

__attribute__((constructor)) static void rudroid_u8t_ctor(void) {
    if (rudroid_anort_ready == 42) {
        rudroid_u8t_ready = 7;
    }
}
