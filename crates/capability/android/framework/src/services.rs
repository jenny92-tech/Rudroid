//! INPUT: Android window/display/resource/locale requests and authoritative Host Config
//! OUTPUT: cached JNI services, default single-display routing and explicit host-presentation compatibility control
//! POS: shared Android façade metadata; Linux rendering/audio devices remain owned by their backends
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{JObject, JniEnv, Native};
use std::ffi::{c_void, CStr};
use std::sync::{atomic::{AtomicBool, Ordering}, OnceLock};
static WINDOW: OnceLock<usize> = OnceLock::new();
static VIEW: OnceLock<usize> = OnceLock::new();
static RESOURCES: OnceLock<usize> = OnceLock::new();
static METRICS: OnceLock<usize> = OnceLock::new();
static DISPLAY: OnceLock<usize> = OnceLock::new();
static DISPLAY_MANAGER: OnceLock<usize> = OnceLock::new();
static WINDOW_MANAGER: OnceLock<usize> = OnceLock::new();
static INPUT_MANAGER: OnceLock<usize> = OnceLock::new();
static AUDIO_MANAGER: OnceLock<usize> = OnceLock::new();
static POWER_MANAGER: OnceLock<usize> = OnceLock::new();
static MEDIA_ROUTER: OnceLock<usize> = OnceLock::new();
static MEDIA_ROUTE: OnceLock<usize> = OnceLock::new();
// A virtual presentation route changes native Unity audio initialization.
// Enable it only through a version profile with measured compatibility evidence.
static HOST_PRESENTATION_ROUTE: AtomicBool = AtomicBool::new(false);
static LOCALE: OnceLock<usize> = OnceLock::new();
static LANGUAGE: OnceLock<usize> = OnceLock::new();
static COUNTRY: OnceLock<usize> = OnceLock::new();
static TAG: OnceLock<usize> = OnceLock::new();
static LOCALE_TEXT: OnceLock<usize> = OnceLock::new();
static LOCALE_CONFIG: OnceLock<String> = OnceLock::new();
static RESOLVER: OnceLock<usize> = OnceLock::new();
static MODE: OnceLock<usize> = OnceLock::new();
fn singleton(env: JniEnv, cache: &OnceLock<usize>, class: &CStr) -> *mut c_void {
    super::java::singleton(env, cache, || super::java::allocate(env, class))
}
extern "C" fn window(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &WINDOW, c"android/view/Window")
}
extern "C" fn view(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &VIEW, c"android/view/View")
}
extern "C" fn resources(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &RESOURCES, c"android/content/res/Resources")
}
extern "C" fn resolver(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &RESOLVER, c"android/content/ContentResolver")
}
extern "C" fn display(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &DISPLAY, c"android/view/Display")
}
extern "C" fn default_locale(env: JniEnv, _class: JObject) -> JObject {
    singleton(env, &LOCALE, c"java/util/Locale")
}
fn locale_config() -> &'static str {
    LOCALE_CONFIG.get_or_init(|| {
        let host = super::HOST.get().unwrap();
        host.config_string(c"locale.tag", 4096)
            .unwrap_or_else(|| "en-US".to_owned())
    })
}
fn locale_string(env: JniEnv, cache: &OnceLock<usize>, value: &str) -> *mut c_void {
    super::java::singleton(env, cache, || super::preferences::string(env, value))
}
extern "C" fn language(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(
        env,
        &LANGUAGE,
        locale_config().split(['-', '_']).next().unwrap_or(""),
    )
}
extern "C" fn country(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(
        env,
        &COUNTRY,
        locale_config()
            .split_once(['-', '_'])
            .map(|(_, country)| country)
            .unwrap_or(""),
    )
}
extern "C" fn locale_tag(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(env, &TAG, locale_config())
}
extern "C" fn locale_text(env: JniEnv, _locale: JObject) -> JObject {
    super::java::singleton(env, &LOCALE_TEXT, || {
        super::preferences::string(env, &locale_config().replace('-', "_"))
    })
}
extern "C" fn fixed_policy(_env: JniEnv, _object: JObject, _value: i32) {}
extern "C" fn fixed_flags(_env: JniEnv, _object: JObject, _a: i32, _b: i32) {}
extern "C" fn permission(env: JniEnv, _context: JObject, name: JObject) -> i32 {
    match super::preferences::text(env, name).as_deref() {
        Some("android.permission.MODIFY_AUDIO_SETTINGS") => 0,
        _ => -1,
    }
}
extern "C" fn system_service(
    env: JniEnv,
    _context: JObject,
    name: JObject,
) -> JObject {
    match super::preferences::text(env, name).as_deref() {
        Some("display") => singleton(
            env,
            &DISPLAY_MANAGER,
            c"android/hardware/display/DisplayManager",
        ),
        Some("window") => singleton(env, &WINDOW_MANAGER, c"android/view/WindowManager"),
        Some("input") => singleton(env, &INPUT_MANAGER, c"android/hardware/input/InputManager"),
        Some("audio") => singleton(env, &AUDIO_MANAGER, c"android/media/AudioManager"),
        Some("power") => singleton(env, &POWER_MANAGER, c"android/os/PowerManager"),
        Some("media_router") => singleton(env, &MEDIA_ROUTER, c"android/media/MediaRouter"),
        _ => std::ptr::null_mut(),
    }
}
static DPI: OnceLock<i32> = OnceLock::new();
fn dpi() -> i32 {
    *DPI.get_or_init(|| {
        super::HOST.get().unwrap().config_i64(c"device.displayDpi", 100)
            .clamp(1, i32::MAX as i64) as i32
    })
}
fn dimension(width: bool) -> i32 {
    let host = super::HOST.get().unwrap();
    // SDL publishes the real drawable before guest preparation. Do not pin
    // a requested size if a service was queried during an earlier phase.
    rudroid_common::display_dimensions::RenderDimensions::from_host(host)
        .map(|size| if width { size.width } else { size.height }).unwrap_or(0)
}
extern "C" fn width(_env: JniEnv, _object: JObject) -> i32 {
    dimension(true)
}
extern "C" fn height(_env: JniEnv, _object: JObject) -> i32 {
    dimension(false)
}
fn fill_metrics(env: JniEnv, object: *mut c_void) {
    let class = env.get_object_class(object);
    for (name, value) in [
        (c"widthPixels", dimension(true)),
        (c"heightPixels", dimension(false)),
        (c"densityDpi", dpi()),
    ] {
        env.set_int_field(object, env.get_field_id(class, name, c"I"), value);
    }
    for (name, value) in [
        (c"density", dpi() as f32 / 160.0),
        (c"scaledDensity", dpi() as f32 / 160.0),
        (c"xdpi", dpi() as f32),
        (c"ydpi", dpi() as f32),
    ] {
        env.set_float_field(object, env.get_field_id(class, name, c"F"), value);
    }
    super::preferences::delete(env, class);
}
extern "C" fn metrics(env: JniEnv, _resources: JObject) -> JObject {
    super::java::singleton(env, &METRICS, || {
        let object = super::java::allocate(env, c"android/util/DisplayMetrics");
        fill_metrics(env, object);
        object
    })
}
extern "C" fn get_metrics(env: JniEnv, _display: JObject, metrics: JObject) {
    fill_metrics(env, metrics)
}
extern "C" fn supported_modes(env: JniEnv, _display: JObject) -> JObject {
    let mode = singleton(env, &MODE, c"android/view/Display$Mode");
    let class = env.find_class(c"android/view/Display$Mode");
    let result = env.new_object_array(1, class, mode);
    super::preferences::delete(env, class);
    super::preferences::delete(env, mode);
    result
}
extern "C" fn get_display(env: JniEnv, manager: JObject, id: i32) -> JObject {
    // Android's default lookup and the reference host presentation identity
    // describe the same cached display; neither creates another host window.
    if matches!(id, 0 | 1) {
        display(env, manager)
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn identifier(
    _env: JniEnv,
    _resource: JObject,
    _a: JObject,
    _b: JObject,
    _c: JObject,
) -> i32 {
    0
}
extern "C" fn timing_offset(_env: JniEnv, _object: JObject) -> i64 {
    0
}
extern "C" fn rotation(_env: JniEnv, _object: JObject) -> i32 {
    super::HOST.get().unwrap().config_i64(c"device.displayRotation", 0).clamp(0, 3) as i32
}
extern "C" fn requested_orientation(_env: JniEnv, _activity: JObject) -> i32 {
    // The host owns a fixed display, as in javastubs/android_misc.cpp.
    // Android ActivityInfo orientation constants are distinct from rotation.
    super::HOST.get().unwrap().config_i64(c"device.displayOrientation", 0)
        .clamp(i32::MIN as i64, i32::MAX as i64) as i32
}
extern "C" fn display_listener(
    _env: JniEnv,
    _manager: JObject,
    _listener: JObject,
    _handler: JObject,
) {
}
extern "C" fn audio_devices(
    env: JniEnv,
    _manager: JObject,
    _flags: i32,
) -> JObject {
    let class = rudroid_common::jni_abi::declare_class(env, c"android/media/AudioDeviceInfo");
    let result = env.new_object_array(0, class, std::ptr::null_mut());
    super::preferences::delete(env, class);
    result
}
extern "C" fn selected_route(
    env: JniEnv,
    _router: JObject,
    flags: i32,
) -> JObject {
    if flags & 2 != 0 && HOST_PRESENTATION_ROUTE.load(Ordering::Acquire) {
        singleton(env, &MEDIA_ROUTE, c"android/media/MediaRouter$RouteInfo")
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn configure_host_presentation(_env: JniEnv, _class: JObject, enabled: u8) {
    HOST_PRESENTATION_ROUTE.store(enabled != 0, Ordering::Release);
}
extern "C" fn presentation_display(env: JniEnv, object: JObject) -> JObject {
    if HOST_PRESENTATION_ROUTE.load(Ordering::Acquire) {
        display(env, object)
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn display_id(_env: JniEnv, _object: JObject) -> i32 {
    // Preserve the primary host display identity independently of whether a
    // version profile enables the virtual presentation compatibility route.
    1
}
extern "C" fn system_ui_visibility(_env: JniEnv, _object: JObject) -> i32 {
    0
}
extern "C" fn refresh_rate(_env: JniEnv, _object: JObject) -> f32 {
    static RATE: OnceLock<f32> = OnceLock::new();
    *RATE.get_or_init(|| {
        let rate = super::HOST.get().unwrap().config_f64(c"device.displayRefreshRate", 60.0);
        if rate.is_finite() && rate > 1.0 {
            rate.min(1000.0) as f32
        } else {
            60.0
        }
    })
}
extern "C" fn unsupported(_env: JniEnv, _object: JObject) -> u8 {
    0
}
extern "C" fn feature(_env: JniEnv, _object: JObject, _name: JObject) -> u8 {
    0
}
extern "C" fn metrics_init(env: JniEnv, object: JObject) {
    fill_metrics(env, object);
}
extern "C" fn listener(_env: JniEnv, _object: JObject, _listener: JObject) {}
extern "C" fn encode_uri(
    env: JniEnv,
    _class: JObject,
    input: JObject,
) -> JObject {
    let Some(input) = super::preferences::text(env, input) else {
        return std::ptr::null_mut();
    };
    let mut output = String::with_capacity(input.len());
    use std::fmt::Write;
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || b"_-!.~'()*".contains(&b) {
            output.push(b as char);
        } else {
            let _ = write!(output, "%{b:02X}");
        }
    }
    super::preferences::string(env, &output)
}
extern "C" fn audio_property(
    env: JniEnv,
    _object: JObject,
    name: JObject,
) -> JObject {
    let Some(name)=env.string_utf(name) else { return std::ptr::null_mut(); };
    match name.as_c_str().to_bytes() {
        b"android.media.property.OUTPUT_SAMPLE_RATE" => env.new_string_utf(c"48000"),
        b"android.media.property.OUTPUT_FRAMES_PER_BUFFER" => env.new_string_utf(c"256"),
        _ => std::ptr::null_mut(),
    }
}
extern "C" fn configuration(env: JniEnv, _resources: JObject) -> JObject {
    static CONFIG: OnceLock<usize> = OnceLock::new();
    super::java::singleton(env, &CONFIG, || {
        let object = super::java::allocate(env, c"android/content/res/Configuration");
        let class = env.find_class(c"android/content/res/Configuration");
        for (name, value) in [
            (
                c"orientation",
                if dimension(true) > dimension(false) {
                    2
                } else {
                    1
                },
            ),
            (c"keyboard", 1),
            (c"keyboardHidden", 2),
            (c"hardKeyboardHidden", 2),
            (c"navigation", 1),
            (c"navigationHidden", 2),
            (c"touchscreen", 1),
            (c"densityDpi", dpi()),
        ] {
            env.set_int_field(object, env.get_field_id(class, name, c"I"), value);
        }
        env.set_float_field(object, env.get_field_id(class, c"fontScale", c"F"), 1.0);
        let locale = default_locale(env, object);
        env.set_object_field(
            object,
            env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
            locale,
        );
        super::preferences::delete(env, locale);
        super::preferences::delete(env, class);
        object
    })
}
extern "C" fn locales(env: JniEnv, _configuration: JObject) -> JObject {
    static LIST: OnceLock<usize> = OnceLock::new();
    super::java::singleton(env, &LIST, || {
        let object = super::java::allocate(env, c"android/os/LocaleList");
        let class = env.find_class(c"android/os/LocaleList");
        let locale = default_locale(env, object);
        env.set_object_field(
            object,
            env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
            locale,
        );
        super::preferences::delete(env, class);
        super::preferences::delete(env, locale);
        object
    })
}
extern "C" fn locale_at(env: JniEnv, list: JObject, index: i32) -> JObject {
    if index != 0 {
        let class = env.find_class(c"java/lang/IndexOutOfBoundsException");
        env.throw_new(class, c"LocaleList index");
        super::preferences::delete(env, class);
        return std::ptr::null_mut();
    }
    let class = env.find_class(c"android/os/LocaleList");
    let result = env.get_object_field(
        list,
        env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
    );
    super::preferences::delete(env, class);
    result
}
extern "C" fn locale_count(_: JniEnv, _: JObject) -> i32 {
    1
}
extern "C" fn unavailable_setting(
    _: JniEnv,
    _: JObject,
    _: JObject,
    _: JObject,
) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn empty_trace(
    env: JniEnv,
    _class: JObject,
    _throwable: JObject,
) -> JObject {
    super::preferences::string(
        env,
        "Rudroid: Java exception (native diagnostics contain details)",
    )
}

#[path = "service_registration.rs"]
mod registration;
pub(super) use registration::register;
