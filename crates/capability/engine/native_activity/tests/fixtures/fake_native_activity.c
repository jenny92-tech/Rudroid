// INPUT:  minimal ANativeActivity POD matching the engine profile's repr(C)
// OUTPUT:  side-effect counters for onCreate / onDestroy observation
// POS:    ARM64 guest fixture proving EngineProfile::run without Unity

typedef struct ANativeActivity ANativeActivity;

typedef struct ANativeActivityCallbacks {
    void (*onStart)(ANativeActivity *activity);
    void (*onResume)(ANativeActivity *activity);
    void *(*onSaveInstanceState)(ANativeActivity *activity, unsigned long *outSize);
    void (*onPause)(ANativeActivity *activity);
    void (*onStop)(ANativeActivity *activity);
    void (*onDestroy)(ANativeActivity *activity);
    void (*onWindowFocusChanged)(ANativeActivity *activity, int hasFocus);
    void (*onNativeWindowCreated)(ANativeActivity *activity, void *window);
    void (*onNativeWindowResized)(ANativeActivity *activity, void *window);
    void (*onNativeWindowRedrawNeeded)(ANativeActivity *activity, void *window);
    void (*onNativeWindowDestroyed)(ANativeActivity *activity, void *window);
    void (*onInputQueueCreated)(ANativeActivity *activity, void *queue);
    void (*onInputQueueDestroyed)(ANativeActivity *activity, void *queue);
    void (*onContentRectChanged)(ANativeActivity *activity, const void *rect);
    void (*onConfigurationChanged)(ANativeActivity *activity);
    void (*onLowMemory)(ANativeActivity *activity);
} ANativeActivityCallbacks;

struct ANativeActivity {
    ANativeActivityCallbacks *callbacks;
    void *vm;
    void *env;
    void *clazz;
    const char *internalDataPath;
    const char *externalDataPath;
    int sdkVersion;
    void *instance;
    void *assetManager;
    const char *obbPath;
};

static volatile int g_created;
static volatile int g_destroyed;
static volatile int g_window_created;

static void on_destroy(ANativeActivity *activity) {
    (void)activity;
    g_destroyed = 1;
}

static void on_native_window_created(ANativeActivity *activity, void *window) {
    (void)activity;
    (void)window;
    g_window_created = 1;
}

void ANativeActivity_onCreate(ANativeActivity *activity, void *savedState, unsigned long savedStateSize) {
    (void)savedState;
    (void)savedStateSize;
    g_created = 1;
    if (activity && activity->callbacks) {
        /* Host Phase-5 proof stubs window lifecycle; callback is registered for ABI fidelity. */
        activity->callbacks->onNativeWindowCreated = on_native_window_created;
        activity->callbacks->onDestroy = on_destroy;
    }
}

int rudroid_na_probe_created(void) { return g_created; }
int rudroid_na_probe_destroyed(void) { return g_destroyed; }
