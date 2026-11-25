// INPUT:  none
// OUTPUT: rudroid_anort_ready
// POS:    preload-order fixture: the first constructor establishes state
int rudroid_anort_ready;

__attribute__((constructor)) static void rudroid_anort_ctor(void) {
    rudroid_anort_ready = 42;
}
