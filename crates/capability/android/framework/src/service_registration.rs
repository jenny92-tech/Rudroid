//! INPUT: checked framework native callbacks
//! OUTPUT: class hierarchy and native method bindings
//! POS: framework service registrar
#![forbid(unsafe_code)]
use super::*;

pub(crate) fn register(env: JniEnv) -> i32 {
    if crate::listener_interfaces::register(env) != 0 { return -1; }
    if declare_field(
        env,
        c"android/os/LocaleList",
        c"locale",
        c"Ljava/util/Locale;",
        false,
    ) != 0
    {
        return -1;
    }
    for (child, parent) in [
        (
            c"android/view/ContextThemeWrapper",
            c"android/content/ContextWrapper",
        ),
        (
            c"android/content/ContextWrapper",
            c"android/content/Context",
        ),
    ] {
        if declare_superclass(env, child, parent) != 0 {
            return -1;
        }
    }
    for (class, fields, static_field) in [
        (
            c"android/content/Context",
            &[
                (c"DISPLAY_SERVICE", c"Ljava/lang/String;"),
                (c"WINDOW_SERVICE", c"Ljava/lang/String;"),
                (c"AUDIO_SERVICE", c"Ljava/lang/String;"),
                (c"POWER_SERVICE", c"Ljava/lang/String;"),
                (c"MEDIA_ROUTER_SERVICE", c"Ljava/lang/String;"),
                (c"LOCATION_SERVICE", c"Ljava/lang/String;"),
                (c"INPUT_SERVICE", c"Ljava/lang/String;"),
            ] as &[(&CStr, &CStr)],
            true,
        ),
        (
            c"android/content/pm/ActivityInfo",
            &[
                (c"SCREEN_ORIENTATION_UNSPECIFIED", c"I"),
                (c"SCREEN_ORIENTATION_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_REVERSE_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_REVERSE_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_FULL_USER", c"I"),
                (c"SCREEN_ORIENTATION_FULL_SENSOR", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR", c"I"),
                (c"SCREEN_ORIENTATION_USER_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_USER_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR_LANDSCAPE", c"I"),
            ],
            true,
        ),
        (
            c"android/view/View",
            &[
                (c"SYSTEM_UI_FLAG_IMMERSIVE_STICKY", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_STABLE", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN", c"I"),
                (c"SYSTEM_UI_FLAG_FULLSCREEN", c"I"),
                (c"SYSTEM_UI_FLAG_HIDE_NAVIGATION", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION", c"I"),
            ],
            true,
        ),
        (
            c"android/media/AudioManager",
            &[
                (c"STREAM_MUSIC", c"I"),
                (c"GET_DEVICES_OUTPUTS", c"I"),
                (c"PROPERTY_OUTPUT_SAMPLE_RATE", c"Ljava/lang/String;"),
                (c"PROPERTY_OUTPUT_FRAMES_PER_BUFFER", c"Ljava/lang/String;"),
            ],
            true,
        ),
        (
            c"android/content/pm/PackageManager",
            &[
                (c"FEATURE_AUDIO_LOW_LATENCY", c"Ljava/lang/String;"),
                (c"PERMISSION_GRANTED", c"I"),
                (c"GET_META_DATA", c"I"),
                (c"GET_INTENT_FILTERS", c"I"),
            ],
            true,
        ),
        (
            c"android/content/Intent",
            &[(c"ACTION_MAIN", c"Ljava/lang/String;")],
            true,
        ),
        (
            c"android/media/MediaRouter",
            &[(c"ROUTE_TYPE_LIVE_VIDEO", c"I")],
            true,
        ),
        (
            c"android/content/res/Configuration",
            &[
                (c"KEYBOARD_QWERTY", c"I"),
                (c"HARDKEYBOARDHIDDEN_YES", c"I"),
            ],
            true,
        ),
        (
            c"android/content/res/Configuration",
            &[
                (c"orientation", c"I"),
                (c"keyboard", c"I"),
                (c"keyboardHidden", c"I"),
                (c"hardKeyboardHidden", c"I"),
                (c"navigation", c"I"),
                (c"navigationHidden", c"I"),
                (c"touchscreen", c"I"),
                (c"colorMode", c"I"),
                (c"densityDpi", c"I"),
                (c"fontScale", c"F"),
                (c"locale", c"Ljava/util/Locale;"),
                (c"mcc", c"I"),
                (c"mnc", c"I"),
                (c"screenLayout", c"I"),
                (c"uiMode", c"I"),
                (c"screenWidthDp", c"I"),
                (c"screenHeightDp", c"I"),
                (c"smallestScreenWidthDp", c"I"),
            ],
            false,
        ),
        (
            c"android/util/DisplayMetrics",
            &[
                (c"widthPixels", c"I"),
                (c"heightPixels", c"I"),
                (c"densityDpi", c"I"),
                (c"density", c"F"),
                (c"scaledDensity", c"F"),
                (c"xdpi", c"F"),
                (c"ydpi", c"F"),
            ],
            false,
        ),
    ] {
        for (name, sig) in fields {
            if declare_field(env, class, name, sig, static_field) != 0 {
                return -1;
            }
        }
    }
    let configuration_class = env.find_class(c"android/content/res/Configuration");
    for name in [c"KEYBOARD_QWERTY", c"HARDKEYBOARDHIDDEN_YES"] {
        env.set_static_int_field(
            configuration_class,
            env.get_static_field_id(configuration_class, name, c"I"),
            2,
        );
    }
    crate::preferences::delete(env, configuration_class);
    for (class, name, value) in [
        (c"android/content/Context", c"DISPLAY_SERVICE", "display"),
        (c"android/content/Context", c"WINDOW_SERVICE", "window"),
        (c"android/content/Context", c"AUDIO_SERVICE", "audio"),
        (c"android/content/Context", c"POWER_SERVICE", "power"),
        (c"android/content/Context", c"LOCATION_SERVICE", "location"),
        (c"android/content/Context", c"INPUT_SERVICE", "input"),
        (c"android/content/Intent", c"ACTION_MAIN", "android.intent.action.MAIN"),
        (
            c"android/content/Context",
            c"MEDIA_ROUTER_SERVICE",
            "media_router",
        ),
        (
            c"android/media/AudioManager",
            c"PROPERTY_OUTPUT_SAMPLE_RATE",
            "android.media.property.OUTPUT_SAMPLE_RATE",
        ),
        (
            c"android/media/AudioManager",
            c"PROPERTY_OUTPUT_FRAMES_PER_BUFFER",
            "android.media.property.OUTPUT_FRAMES_PER_BUFFER",
        ),
        (
            c"android/content/pm/PackageManager",
            c"FEATURE_AUDIO_LOW_LATENCY",
            "android.hardware.audio.low_latency",
        ),
    ] {
        let class = env.find_class(class);
        let value = crate::preferences::string(env, value);
        env.set_static_object_field(
            class,
            env.get_static_field_id(class, name, c"Ljava/lang/String;"),
            value,
        );
        crate::preferences::delete(env, value);
        crate::preferences::delete(env, class);
    }
    for (class, name, value) in [
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_UNSPECIFIED",
            -1,
        ),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_IMMERSIVE_STICKY",
            4096,
        ),
        (c"android/media/AudioManager", c"STREAM_MUSIC", 3),
        (c"android/media/AudioManager", c"GET_DEVICES_OUTPUTS", 2),
        (c"android/content/pm/PackageManager", c"GET_META_DATA", 128),
        (c"android/content/pm/PackageManager", c"GET_INTENT_FILTERS", 32),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_FULL_SENSOR", 10),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR", 4),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_USER_PORTRAIT", 12),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_USER_LANDSCAPE", 11),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR_PORTRAIT", 7),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR_LANDSCAPE", 6),
        (
            c"android/content/pm/PackageManager",
            c"PERMISSION_GRANTED",
            0,
        ),
        (c"android/media/MediaRouter", c"ROUTE_TYPE_LIVE_VIDEO", 2),
        (c"android/view/View", c"SYSTEM_UI_FLAG_LAYOUT_STABLE", 256),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_PORTRAIT",
            1,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_REVERSE_PORTRAIT",
            9,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_LANDSCAPE",
            0,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_REVERSE_LANDSCAPE",
            8,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_FULL_USER",
            13,
        ),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN",
            1024,
        ),
        (c"android/view/View", c"SYSTEM_UI_FLAG_FULLSCREEN", 4),
        (c"android/view/View", c"SYSTEM_UI_FLAG_HIDE_NAVIGATION", 2),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION",
            512,
        ),
    ] {
        let class = env.find_class(class);
        env.set_static_int_field(class, env.get_static_field_id(class, name, c"I"), value);
        crate::preferences::delete(env, class);
    }
    for (class, methods) in [
        (
            c"android/content/Context",
            vec![
                Native::new0(
                    c"getResources",
                    c"()Landroid/content/res/Resources;",
                    resources
                ),
                Native::new0(
                    c"getContentResolver",
                    c"()Landroid/content/ContentResolver;",
                    resolver
                ),
                Native::new1(
                    c"getSystemService",
                    c"(Ljava/lang/String;)Ljava/lang/Object;",
                    system_service
                ),
                Native::new1(
                    c"checkCallingOrSelfPermission",
                    c"(Ljava/lang/String;)I",
                    permission
                ),
            ],
        ),
        (
            c"android/app/Activity",
            vec![
                Native::new0(c"getWindow", c"()Landroid/view/Window;", window),
                Native::new1(c"setRequestedOrientation", c"(I)V", fixed_policy),
                Native::new0(c"getRequestedOrientation", c"()I", requested_orientation),
            ],
        ),
        (
            c"android/view/Window",
            vec![
                Native::new0(c"getDecorView", c"()Landroid/view/View;", view),
                Native::new1(c"addFlags", c"(I)V", fixed_policy),
                Native::new2(c"setFlags", c"(II)V", fixed_flags),
            ],
        ),
        (
            c"android/view/View",
            vec![
                Native::new1(c"setSystemUiVisibility", c"(I)V", fixed_policy),
                Native::new0(c"getSystemUiVisibility", c"()I", system_ui_visibility),
                Native::new1(c"removeOnLayoutChangeListener",c"(Landroid/view/View$OnLayoutChangeListener;)V",listener),
                Native::new1(c"setOnSystemUiVisibilityChangeListener", c"(Landroid/view/View$OnSystemUiVisibilityChangeListener;)V", listener),
                Native::new0(c"getWidth", c"()I", width),
                Native::new0(c"getHeight", c"()I", height),
            ],
        ),
        (
            c"android/view/WindowManager",
            vec![Native::new0(
                c"getDefaultDisplay",
                c"()Landroid/view/Display;",
                display
            )],
        ),
        (
            c"android/view/Display",
            vec![
                Native::new0(c"getDisplayId", c"()I", display_id),
                Native::new0(c"getRotation", c"()I", rotation),
                Native::new0(c"getAppVsyncOffsetNanos", c"()J", timing_offset),
                Native::new0(c"getPresentationDeadlineNanos", c"()J", timing_offset),
                Native::new0(c"isWideColorGamut", c"()Z", unsupported),
                Native::new0(c"isHdr", c"()Z", unsupported),
                Native::new0(c"getRefreshRate", c"()F", refresh_rate),
                Native::new0(c"getWidth", c"()I", width),
                Native::new0(c"getHeight", c"()I", height),
                Native::new1(
                    c"getMetrics",
                    c"(Landroid/util/DisplayMetrics;)V",
                    get_metrics
                ),
                Native::new1(
                    c"getRealMetrics",
                    c"(Landroid/util/DisplayMetrics;)V",
                    get_metrics
                ),
                Native::new0(
                    c"getSupportedModes",
                    c"()[Landroid/view/Display$Mode;",
                    supported_modes
                ),
            ],
        ),
        (
            c"android/hardware/display/DisplayManager",
            vec![Native::new1(c"unregisterDisplayListener",c"(Landroid/hardware/display/DisplayManager$DisplayListener;)V",listener),Native::new2(c"registerDisplayListener", c"(Landroid/hardware/display/DisplayManager$DisplayListener;Landroid/os/Handler;)V", display_listener), Native::new1(
                c"getDisplay",
                c"(I)Landroid/view/Display;",
                get_display
            )],
        ),
        (
            c"android/view/Display$Mode",
            vec![
                Native::new0(c"getRefreshRate", c"()F", refresh_rate),
                Native::new0(c"getPhysicalWidth", c"()I", width),
                Native::new0(c"getPhysicalHeight", c"()I", height),
            ],
        ),
        (
            c"android/content/res/Resources",
            vec![
                Native::new0(c"getConfiguration", c"()Landroid/content/res/Configuration;", configuration),
                Native::new0(
                    c"getDisplayMetrics",
                    c"()Landroid/util/DisplayMetrics;",
                    metrics
                ),
                Native::new3(
                    c"getIdentifier",
                    c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                    identifier
                ),
            ],
        ),
        (c"android/content/res/Configuration",vec![Native::new0(c"getLocales",c"()Landroid/os/LocaleList;",locales)]),
        (c"android/os/LocaleList",vec![Native::new1(c"get",c"(I)Ljava/util/Locale;",locale_at),Native::new0(c"size",c"()I",locale_count),Native::new0(c"isEmpty",c"()Z",unsupported)]),
        (c"android/provider/Settings$Secure",vec![Native::new2(c"getString",c"(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",unavailable_setting)]),
        (
            c"java/util/Locale",
            vec![
                Native::new0(c"getDefault", c"()Ljava/util/Locale;", default_locale),
                Native::new0(c"getLanguage", c"()Ljava/lang/String;", language),
                Native::new0(c"getCountry", c"()Ljava/lang/String;", country),
                Native::new0(c"toLanguageTag", c"()Ljava/lang/String;", locale_tag),
                Native::new0(c"toString", c"()Ljava/lang/String;", locale_text),
            ],
        ),
        (c"android/util/DisplayMetrics", vec![Native::new0(c"<init>", c"()V", metrics_init)]),
        (c"android/os/PowerManager", vec![Native::new0(c"isSustainedPerformanceModeSupported", c"()Z", unsupported)]),
        (c"android/content/pm/PackageManager", vec![Native::new1(c"hasSystemFeature", c"(Ljava/lang/String;)Z", feature)]),
        (c"android/media/AudioManager", vec![Native::new1(c"getProperty", c"(Ljava/lang/String;)Ljava/lang/String;", audio_property), Native::new1(c"getDevices", c"(I)[Landroid/media/AudioDeviceInfo;", audio_devices)]),
        (c"android/media/MediaRouter", vec![
            Native::new1(c"getSelectedRoute", c"(I)Landroid/media/MediaRouter$RouteInfo;", selected_route),
            Native::new1(c"_rudroidSetHostPresentationRoute", c"(Z)V", configure_host_presentation),
        ]),
        (c"android/media/MediaRouter$RouteInfo", vec![Native::new0(c"getPresentationDisplay", c"()Landroid/view/Display;", presentation_display)]),
        (c"android/net/Uri", vec![
            Native::new1(c"encode", c"(Ljava/lang/String;)Ljava/lang/String;", encode_uri),
            Native::new1(c"decode", c"(Ljava/lang/String;)Ljava/lang/String;", crate::uri::decode),
        ]),
        (
            c"android/util/Log",
            vec![Native::new1(
                c"getStackTraceString",
                c"(Ljava/lang/Throwable;)Ljava/lang/String;",
                empty_trace
            )],
        ),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    if crate::threads::register(env) != 0
        || crate::input::register(env) != 0
        || crate::lifecycle::register(env) != 0
        || crate::trust_store::register(env) != 0
    {
        return -1;
    }
    crate::choreographer::register(env)
}
