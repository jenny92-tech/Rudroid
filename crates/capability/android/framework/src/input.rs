//! INPUT:  shared JNI objects populated by the selected host input backend
//! OUTPUT:  VM-owned input factories, device identity and Android event getters
//! POS:    one VM-owned event representation shared by host input and Android façades
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Method, Native};
use std::ffi::{c_void, CStr};
use std::sync::OnceLock;
const DATA: &CStr = c"org/rudroid/runtime/InputEventData";
struct Fields {
    action: usize,
    source: usize,
    code: usize,
    scan: usize,
    repeat: usize,
    time: usize,
    down: usize,
    x: usize,
    y: usize,
    device: usize,
    axes: [usize; 8],
    buttons: usize,
}
static FIELDS: OnceLock<Fields> = OnceLock::new();
static CHARACTER_MAP: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
static DEVICE_FIELDS: OnceLock<[usize; 6]> = OnceLock::new();
struct DeviceSource {
    class: usize,
    device: Method,
    ids: Method,
}
static DEVICE_SOURCE: OnceLock<DeviceSource> = OnceLock::new();
struct Listener {
    listener: usize,
    handler: usize,
}
static LISTENERS: std::sync::Mutex<Vec<Listener>> = std::sync::Mutex::new(Vec::new());
static LISTENERS_STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const NOTIFY: &CStr = c"org/rudroid/runtime/InputDeviceNotification";
static NOTIFY_FIELDS: OnceLock<[usize; 3]> = OnceLock::new();
pub(super) fn shutdown(env: JniEnv) {
    LISTENERS_STOPPED.store(true, std::sync::atomic::Ordering::Release);
    let listeners = std::mem::take(&mut *LISTENERS.lock().unwrap());
    for l in listeners {
        env.delete_global_ref(l.listener as JObject);
        env.delete_global_ref(l.handler as JObject);
    }
}
fn notify_one(env: JniEnv, listener: JObject, kind: i32, id: i32) {
    if LISTENERS_STOPPED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let name = match kind {
        0 => c"onInputDeviceAdded",
        1 => c"onInputDeviceRemoved",
        _ => c"onInputDeviceChanged",
    };
    let class = env.get_object_class(listener);
    let method = env.method(class, name, c"(I)V");
    env.delete_local_ref(class);
    if let Some(method) = method {
        env.call_void(listener, method, &[Arg::int(id)]);
    }
}
extern "C" fn notify_run(env: JniEnv, task: JObject) {
    let fields = NOTIFY_FIELDS.get().unwrap();
    let listener = env.get_object_field(task, fields[0] as JObject);
    notify_one(
        env,
        listener,
        env.get_int_field(task, fields[1] as JObject),
        env.get_int_field(task, fields[2] as JObject),
    );
    env.delete_local_ref(listener);
}
fn notify(env: JniEnv, kind: i32, id: i32) {
    let leases: Vec<_> = {
        let listeners = LISTENERS.lock().unwrap();
        // Reference-only VM operations retain under the source lock; no Java callback runs here.
        listeners
            .iter()
            .map(|l| {
                (
                    env.new_local_ref(l.listener as JObject),
                    env.new_local_ref(l.handler as JObject),
                )
            })
            .collect()
    };
    for (listener, handler) in leases {
        if !env.exception_check() && !listener.is_null() {
            if handler.is_null() {
                notify_one(env, listener, kind, id);
            } else {
                let task = allocate(env, NOTIFY);
                if task.is_null() {
                    env.delete_local_ref(listener);
                    env.delete_local_ref(handler);
                    continue;
                }
                let fields = NOTIFY_FIELDS.get().unwrap();
                env.set_object_field(task, fields[0] as JObject, listener);
                env.set_int_field(task, fields[1] as JObject, kind);
                env.set_int_field(task, fields[2] as JObject, id);
                let class = env.get_object_class(handler);
                let post = env.method(class, c"post", c"(Ljava/lang/Runnable;)Z");
                env.delete_local_ref(class);
                if let Some(post) = post {
                    env.call_boolean(handler, post, &[Arg::object(task)]);
                }
                env.delete_local_ref(task);
            }
        }
        env.delete_local_ref(listener);
        env.delete_local_ref(handler);
    }
}
extern "C" fn added(env: JniEnv, _class: JObject, id: i32) {
    notify(env, 0, id)
}
extern "C" fn removed(env: JniEnv, _class: JObject, id: i32) {
    notify(env, 1, id)
}
extern "C" fn changed(env: JniEnv, _class: JObject, id: i32) {
    notify(env, 2, id)
}
extern "C" fn absent_device(_env: JniEnv, _class: JObject, _id: i32) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn empty_ids(env: JniEnv, _class: JObject) -> JObject {
    env.new_int_array(0)
}
extern "C" fn device(env: JniEnv, _class: JObject, id: i32) -> JObject {
    let source = DEVICE_SOURCE.get().unwrap();
    env.call_static_object(source.class as *mut c_void, source.device, &[Arg::int(id)])
}
extern "C" fn device_ids(env: JniEnv, _class: JObject) -> JObject {
    let source = DEVICE_SOURCE.get().unwrap();
    env.call_static_object(source.class as *mut c_void, source.ids, &[])
}
extern "C" fn event_device_id(env: JniEnv, event: JObject) -> i32 {
    int(env, event, FIELDS.get().unwrap().device)
}
extern "C" fn event_device(env: JniEnv, event: JObject) -> JObject {
    device(env, std::ptr::null_mut(), event_device_id(env, event))
}
extern "C" fn device_factory(env: JniEnv, _class: JObject) -> JObject {
    allocate(env, c"android/view/InputDevice")
}
extern "C" fn device_id(env: JniEnv, object: JObject) -> i32 {
    int(env, object, DEVICE_FIELDS.get().unwrap()[0])
}
extern "C" fn device_sources(env: JniEnv, object: JObject) -> i32 {
    int(env, object, DEVICE_FIELDS.get().unwrap()[1])
}
extern "C" fn device_name(env: JniEnv, object: JObject) -> JObject {
    env.get_object_field(object, DEVICE_FIELDS.get().unwrap()[2] as *mut c_void)
}
extern "C" fn device_vendor(env: JniEnv, object: JObject) -> i32 {
    int(env, object, DEVICE_FIELDS.get().unwrap()[4])
}
extern "C" fn device_product(env: JniEnv, object: JObject) -> i32 {
    int(env, object, DEVICE_FIELDS.get().unwrap()[5])
}
extern "C" fn device_supports(env: JniEnv, object: JObject, source: i32) -> u8 {
    u8::from(device_sources(env, object) & source == source)
}
extern "C" fn virtual_device(env: JniEnv, object: JObject) -> u8 {
    u8::from(device_id(env, object) <= 0)
}
extern "C" fn keyboard_type(env: JniEnv, object: JObject) -> i32 {
    if device_sources(env, object) & 0x100 != 0 {
        2
    } else {
        0
    }
}
const RANGE: &CStr = c"android/view/InputDevice$MotionRange";
struct RangeFields {
    axis: usize,
    source: usize,
    min: usize,
    max: usize,
    flat: usize,
}
static RANGE_FIELDS: OnceLock<RangeFields> = OnceLock::new();
fn range_spec(
    env: JniEnv,
    object: JObject,
    axis: i32,
    requested_source: i32,
) -> Option<(i32, f32, f32, f32)> {
    let sources = device_sources(env, object);
    if sources & 0x01000010 == 0x01000010 {
        // The backend must advertise only axes it will actually emit. In
        // particular Unity selects a different gamepad layout when HAT ranges exist.
        if matches!(axis, 15 | 16)
            && env.get_boolean_field(object, DEVICE_FIELDS.get().unwrap()[3] as JObject) == 0
        {
            return None;
        }
        if requested_source != 0 && sources & requested_source != requested_source {
            return None;
        }
        match axis {
            0 | 1 | 11 | 14 => Some((0x01000010, -1.0, 1.0, 0.12)),
            15 | 16 => Some((0x01000010, -1.0, 1.0, 0.0)),
            17 | 18 | 22 | 23 => Some((0x01000010, 0.0, 1.0, 0.0)),
            _ => None,
        }
    } else {
        if !matches!(axis, 0 | 1) {
            return None;
        }
        let source = if requested_source != 0 {
            requested_source
        } else if sources & 0x2002 == 0x2002 {
            0x2002
        } else {
            0x1002
        };
        if sources & source != source || source & 2 == 0 {
            return None;
        }
        let size = rudroid_common::display_dimensions::RenderDimensions::from_host(
            super::HOST.get().unwrap(),
        )
        .ok()?;
        Some((
            source,
            0.0,
            if axis == 0 {
                size.width as f32
            } else {
                size.height as f32
            },
            0.0,
        ))
    }
}
extern "C" fn motion_range_source(env: JniEnv, object: JObject, axis: i32, source: i32) -> JObject {
    let Some((source, min, max, flat)) = range_spec(env, object, axis, source) else {
        return std::ptr::null_mut();
    };
    let fields = RANGE_FIELDS.get().unwrap();
    let range = allocate(env, RANGE);
    env.set_int_field(range, fields.axis as JObject, axis);
    env.set_int_field(range, fields.source as JObject, source);
    for (field, value) in [(fields.min, min), (fields.max, max), (fields.flat, flat)] {
        env.set_float_field(range, field as JObject, value);
    }
    range
}
extern "C" fn motion_range(env: JniEnv, object: JObject, axis: i32) -> JObject {
    motion_range_source(env, object, axis, 0)
}
extern "C" fn motion_ranges(env: JniEnv, object: JObject) -> JObject {
    let axes = [0, 1, 11, 14, 15, 16, 17, 18, 22, 23];
    let ranges: Vec<_> = axes
        .into_iter()
        .map(|axis| motion_range(env, object, axis))
        .filter(|r| !r.is_null())
        .collect();
    let class = env.find_class(RANGE);
    let array = env.new_object_array(ranges.len() as i32, class, std::ptr::null_mut());
    env.delete_local_ref(class);
    for (i, range) in ranges.into_iter().enumerate() {
        env.set_object_array_element(array, i as i32, range);
        env.delete_local_ref(range);
    }
    let list = super::collections::list(env, array);
    env.delete_local_ref(array);
    list
}
extern "C" fn range_axis(env: JniEnv, o: JObject) -> i32 {
    int(env, o, RANGE_FIELDS.get().unwrap().axis)
}
extern "C" fn range_source(env: JniEnv, o: JObject) -> i32 {
    int(env, o, RANGE_FIELDS.get().unwrap().source)
}
extern "C" fn range_min(env: JniEnv, o: JObject) -> f32 {
    float(env, o, RANGE_FIELDS.get().unwrap().min)
}
extern "C" fn range_max(env: JniEnv, o: JObject) -> f32 {
    float(env, o, RANGE_FIELDS.get().unwrap().max)
}
extern "C" fn range_flat(env: JniEnv, o: JObject) -> f32 {
    float(env, o, RANGE_FIELDS.get().unwrap().flat)
}
extern "C" fn range_span(env: JniEnv, o: JObject) -> f32 {
    range_max(env, o) - range_min(env, o)
}
extern "C" fn register_device_listener(
    env: JniEnv,
    _manager: JObject,
    listener: JObject,
    handler: JObject,
) {
    if listener.is_null() || LISTENERS_STOPPED.load(std::sync::atomic::Ordering::Acquire) {
        super::preferences::error(env, "input device listener must not be null");
        return;
    }
    let root = env.new_global_ref(listener);
    let handler_root = env.new_global_ref(handler);
    if root.is_null() || (!handler.is_null() && handler_root.is_null()) {
        env.delete_global_ref(root);
        env.delete_global_ref(handler_root);
        return;
    }
    let (inserted, full) = {
        let mut listeners = LISTENERS.lock().unwrap();
        let duplicate = listeners
            .iter()
            .any(|l| env.is_same_object(l.listener as JObject, listener));
        let full = !duplicate && listeners.len() >= 32;
        if !duplicate && !full && !LISTENERS_STOPPED.load(std::sync::atomic::Ordering::Acquire) {
            listeners.push(Listener {
                listener: root as usize,
                handler: handler_root as usize,
            });
            (true, false)
        } else {
            (false, full)
        }
    };
    if !inserted {
        env.delete_global_ref(root);
        env.delete_global_ref(handler_root);
    }
    if full {
        super::preferences::error(env, "input device listener limit exceeded");
    }
}
extern "C" fn unregister_device_listener(env: JniEnv, _manager: JObject, listener: JObject) {
    let removed = {
        let mut listeners = LISTENERS.lock().unwrap();
        listeners
            .iter()
            .position(|l| env.is_same_object(l.listener as JObject, listener))
            .map(|at| listeners.remove(at))
    };
    if let Some(l) = removed {
        env.delete_global_ref(l.listener as JObject);
        env.delete_global_ref(l.handler as JObject);
    }
}
extern "C" fn character_map(env: JniEnv, _class: JObject, _device: i32) -> JObject {
    let mut root = CHARACTER_MAP.lock().unwrap();
    if *root == 0 {
        let object = allocate(env, c"android/view/KeyCharacterMap");
        if object.is_null() {
            return object;
        }
        *root = env.new_global_ref(object) as usize;
        env.delete_local_ref(object);
    }
    env.new_local_ref(*root as *mut c_void)
}
fn character(code: i32, meta: i32) -> i32 {
    match code {
        29..=54 => {
            let letter = b'a' as i32 + code - 29;
            if (meta & 1 != 0) ^ (meta & 0x100000 != 0) {
                letter - 32
            } else {
                letter
            }
        }
        7..=16 => b'0' as i32 + code - 7,
        62 => 32,
        66 => 10,
        61 => 9,
        67 => 8,
        _ => 0,
    }
}
extern "C" fn character_get(_env: JniEnv, _map: JObject, code: i32, meta: i32) -> i32 {
    character(code, meta)
}
fn int(env: JniEnv, event: *mut c_void, field: usize) -> i32 {
    env.get_int_field(event, field as *mut c_void)
}
fn long(env: JniEnv, event: *mut c_void, field: usize) -> i64 {
    env.get_long_field(event, field as *mut c_void)
}
fn float(env: JniEnv, event: *mut c_void, field: usize) -> f32 {
    env.get_float_field(event, field as *mut c_void)
}
extern "C" fn action(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().action)
}
extern "C" fn source(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().source)
}
extern "C" fn code(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().code)
}
extern "C" fn scan(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().scan)
}
extern "C" fn repeat(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().repeat)
}
extern "C" fn time(e: JniEnv, o: JObject) -> i64 {
    long(e, o, FIELDS.get().unwrap().time)
}
extern "C" fn down(e: JniEnv, o: JObject) -> i64 {
    long(e, o, FIELDS.get().unwrap().down)
}
extern "C" fn x(e: JniEnv, o: JObject, _index: i32) -> f32 {
    float(e, o, FIELDS.get().unwrap().x)
}
extern "C" fn y(e: JniEnv, o: JObject, _index: i32) -> f32 {
    float(e, o, FIELDS.get().unwrap().y)
}
extern "C" fn x0(e: JniEnv, o: JObject) -> f32 {
    x(e, o, 0)
}
extern "C" fn y0(e: JniEnv, o: JObject) -> f32 {
    y(e, o, 0)
}
extern "C" fn axis(e: JniEnv, o: JObject, axis: i32, _index: i32) -> f32 {
    match axis {
        0 => x(e, o, 0),
        1 => y(e, o, 0),
        2 => pressure(e, o, 0),
        11 | 14 | 15 | 16 | 17 | 18 | 23 | 22 => {
            let index = match axis {
                11 => 0,
                14 => 1,
                15 => 2,
                16 => 3,
                17 => 4,
                18 => 5,
                23 => 6,
                _ => 7,
            };
            float(e, o, FIELDS.get().unwrap().axes[index])
        }
        _ => 0.0,
    }
}
extern "C" fn axis0(e: JniEnv, o: JObject, a: i32) -> f32 {
    axis(e, o, a, 0)
}
extern "C" fn button_state(e: JniEnv, o: JObject) -> i32 {
    int(e, o, FIELDS.get().unwrap().buttons)
}
extern "C" fn pressure(e: JniEnv, o: JObject, _index: i32) -> f32 {
    if action(e, o) == 1 {
        0.0
    } else {
        1.0
    }
}
extern "C" fn pressure0(e: JniEnv, o: JObject) -> f32 {
    pressure(e, o, 0)
}
extern "C" fn unit_float(_e: JniEnv, _o: JObject) -> f32 {
    1.0
}
extern "C" fn zero_float(_e: JniEnv, _o: JObject) -> f32 {
    0.0
}
extern "C" fn unit_index(_e: JniEnv, _o: JObject, _i: i32) -> f32 {
    1.0
}
extern "C" fn zero_index(_e: JniEnv, _o: JObject, _i: i32) -> f32 {
    0.0
}
extern "C" fn one(_e: JniEnv, _o: JObject) -> i32 {
    1
}
extern "C" fn zero(_e: JniEnv, _o: JObject) -> i32 {
    0
}
extern "C" fn index(_e: JniEnv, _o: JObject, _i: i32) -> i32 {
    0
}
extern "C" fn tool(_e: JniEnv, _o: JObject, _i: i32) -> i32 {
    1
}
extern "C" fn empty(_e: JniEnv, _o: JObject) {}
extern "C" fn absent(_e: JniEnv, _o: JObject) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn no_quit(_e: JniEnv, _o: JObject) -> u8 {
    0
}
fn allocate(env: JniEnv, name: &CStr) -> *mut c_void {
    let class = env.find_class(name);
    let event = env.alloc_object(class);
    env.delete_local_ref(class);
    event
}
extern "C" fn key(env: JniEnv, _class: JObject) -> JObject {
    allocate(env, c"android/view/KeyEvent")
}
extern "C" fn motion(env: JniEnv, _class: JObject) -> JObject {
    allocate(env, c"android/view/MotionEvent")
}
extern "C" fn obtain(env: JniEnv, _class: JObject, event: JObject) -> JObject {
    if event.is_null() {
        return std::ptr::null_mut();
    }
    let copy = allocate(env, c"android/view/MotionEvent");
    if copy.is_null() {
        return copy;
    }
    let fields = FIELDS.get().unwrap();
    for field in [
        fields.action,
        fields.source,
        fields.code,
        fields.scan,
        fields.repeat,
        fields.device,
    ] {
        env.set_int_field(copy, field as *mut c_void, int(env, event, field));
    }
    for field in [fields.time, fields.down] {
        env.set_long_field(copy, field as *mut c_void, long(env, event, field));
    }
    for field in [fields.x, fields.y] {
        env.set_float_field(copy, field as *mut c_void, float(env, event, field));
    }
    for field in fields.axes {
        env.set_float_field(copy, field as JObject, float(env, event, field));
    }
    env.set_int_field(
        copy,
        fields.buttons as JObject,
        int(env, event, fields.buttons),
    );
    copy
}
pub(super) fn register(env: JniEnv) -> i32 {
    if declare_superclass(env, NOTIFY, c"java/lang/Runnable") != 0 {
        return -1;
    }
    let mut notification = [0; 3];
    for (i, (name, sig)) in [
        (c"listener", c"Ljava/lang/Object;"),
        (c"kind", c"I"),
        (c"device", c"I"),
    ]
    .iter()
    .enumerate()
    {
        if declare_field(env, NOTIFY, name, sig, false) != 0 {
            return -1;
        }
        let class = env.find_class(NOTIFY);
        notification[i] = env.get_field_id(class, name, sig) as usize;
        env.delete_local_ref(class);
    }
    if notification.contains(&0) {
        return -1;
    }
    let _ = NOTIFY_FIELDS.set(notification);
    if env.register_class(NOTIFY, &[Native::new0(c"run", c"()V", notify_run)]) != 0 {
        return -1;
    }
    if env.register_class(
        c"org/rudroid/runtime/InputEvents",
        &[
            Native::new1(c"deviceAdded", c"(I)V", added),
            Native::new1(c"deviceRemoved", c"(I)V", removed),
            Native::new1(c"deviceChanged", c"(I)V", changed),
        ],
    ) != 0
    {
        return -1;
    }
    let mut fields = [0; 5];
    for (i, (name, sig)) in [
        (c"axis", c"I"),
        (c"source", c"I"),
        (c"minimum", c"F"),
        (c"maximum", c"F"),
        (c"flat", c"F"),
    ]
    .iter()
    .enumerate()
    {
        if declare_field(env, RANGE, name, sig, false) != 0 {
            return -1;
        }
        let class = env.find_class(RANGE);
        fields[i] = env.get_field_id(class, name, sig) as usize;
        env.delete_local_ref(class);
    }
    if fields.contains(&0) {
        return -1;
    }
    let _ = RANGE_FIELDS.set(RangeFields {
        axis: fields[0],
        source: fields[1],
        min: fields[2],
        max: fields[3],
        flat: fields[4],
    });
    if env.register_class(
        RANGE,
        &[
            Native::new0(c"getAxis", c"()I", range_axis),
            Native::new0(c"getSource", c"()I", range_source),
            Native::new0(c"getMin", c"()F", range_min),
            Native::new0(c"getMax", c"()F", range_max),
            Native::new0(c"getFlat", c"()F", range_flat),
            Native::new0(c"getRange", c"()F", range_span),
            Native::new0(c"getFuzz", c"()F", zero_float),
            Native::new0(c"getResolution", c"()F", zero_float),
        ],
    ) != 0
    {
        return -1;
    }
    for &(name, value) in super::keycodes::VALUES {
        let class = c"android/view/KeyEvent";
        if declare_field(env, class, name, c"I", true) != 0 {
            return -1;
        }
        let class = env.find_class(class);
        env.set_static_int_field(class, env.get_static_field_id(class, name, c"I"), value);
        env.delete_local_ref(class);
    }
    for (class, parent) in [
        (c"android/view/InputEvent", DATA),
        (c"android/view/KeyEvent", c"android/view/InputEvent"),
        (c"android/view/MotionEvent", c"android/view/InputEvent"),
        (
            c"android/view/InputDevice",
            c"org/rudroid/runtime/InputDeviceData",
        ),
    ] {
        if declare_superclass(env, class, parent) != 0 {
            return -1;
        }
    }
    let specs = [
        (c"action", c"I"),
        (c"source", c"I"),
        (c"keyCode", c"I"),
        (c"scanCode", c"I"),
        (c"repeatCount", c"I"),
        (c"eventTime", c"J"),
        (c"downTime", c"J"),
        (c"x", c"F"),
        (c"y", c"F"),
        (c"deviceId", c"I"),
    ];
    let mut ids = [0; 10];
    for (i, (name, sig)) in specs.iter().enumerate() {
        if declare_field(env, DATA, name, sig, false) != 0 {
            return -1;
        }
        let class = env.find_class(DATA);
        ids[i] = env.get_field_id(class, name, sig) as usize;
        env.delete_local_ref(class);
        if ids[i] == 0 {
            return -1;
        }
    }
    let class = env.find_class(DATA);
    let mut axes = [0; 8];
    for (i, name) in [
        c"z",
        c"rz",
        c"hatX",
        c"hatY",
        c"leftTrigger",
        c"rightTrigger",
        c"brake",
        c"gas",
    ]
    .iter()
    .enumerate()
    {
        if declare_field(env, DATA, name, c"F", false) != 0 {
            return -1;
        }
        axes[i] = env.get_field_id(class, name, c"F") as usize;
    }
    if declare_field(env, DATA, c"buttonState", c"I", false) != 0 {
        return -1;
    }
    let buttons = env.get_field_id(class, c"buttonState", c"I") as usize;
    env.delete_local_ref(class);
    if axes.contains(&0) || buttons == 0 {
        return -1;
    }
    FIELDS.get_or_init(|| Fields {
        action: ids[0],
        source: ids[1],
        code: ids[2],
        scan: ids[3],
        repeat: ids[4],
        time: ids[5],
        down: ids[6],
        x: ids[7],
        y: ids[8],
        device: ids[9],
        axes,
        buttons,
    });
    let data = c"org/rudroid/runtime/InputDeviceData";
    let mut device_fields = [0; 6];
    for (i, (name, sig)) in [
        (c"id", c"I"),
        (c"sources", c"I"),
        (c"name", c"Ljava/lang/String;"),
        (c"hasHatAxes", c"Z"),
        (c"vendorId", c"I"),
        (c"productId", c"I"),
    ]
    .iter()
    .enumerate()
    {
        if declare_field(env, data, name, sig, false) != 0 {
            return -1;
        }
        let class = env.find_class(data);
        device_fields[i] = env.get_field_id(class, name, sig) as usize;
        env.delete_local_ref(class);
    }
    if device_fields.contains(&0) {
        return -1;
    }
    DEVICE_FIELDS.get_or_init(|| device_fields);
    for (class, methods) in [
        (c"android/hardware/input/InputManager", vec![
            Native::new1(c"getInputDevice", c"(I)Landroid/view/InputDevice;", device),
            Native::new0(c"getInputDeviceIds", c"()[I", device_ids),
            Native::new2(c"registerInputDeviceListener", c"(Landroid/hardware/input/InputManager$InputDeviceListener;Landroid/os/Handler;)V", register_device_listener),
            Native::new1(c"unregisterInputDeviceListener", c"(Landroid/hardware/input/InputManager$InputDeviceListener;)V", unregister_device_listener),
        ]),
        (
            c"android/view/KeyCharacterMap",
            vec![
                Native::new1(c"load", c"(I)Landroid/view/KeyCharacterMap;", character_map),
                Native::new2(c"get", c"(II)I", character_get),
            ],
        ),
        (
            c"org/rudroid/runtime/InputEvents",
            vec![
                Native::new0(c"key", c"()Ljava/lang/Object;", key),
                Native::new0(c"motion", c"()Ljava/lang/Object;", motion),
                Native::new0(c"device", c"()Ljava/lang/Object;", device_factory),
            ],
        ),
        (
            c"org/rudroid/runtime/InputSource",
            vec![
                Native::new0(c"poll", c"()Ljava/lang/Object;", absent),
                Native::new0(c"quitRequested", c"()Z", no_quit),
                Native::new1(c"getDevice", c"(I)Ljava/lang/Object;", absent_device),
                Native::new0(c"getDeviceIds", c"()[I", empty_ids),
            ],
        ),
        (
            c"android/view/InputDevice",
            vec![
                Native::new1(c"getDevice", c"(I)Landroid/view/InputDevice;", device),
                Native::new0(c"getDeviceIds", c"()[I", device_ids),
                Native::new0(c"getId", c"()I", device_id),
                Native::new0(c"getSources", c"()I", device_sources),
                Native::new0(c"getName", c"()Ljava/lang/String;", device_name),
                Native::new0(c"getDescriptor", c"()Ljava/lang/String;", device_name),
                Native::new0(c"getVendorId", c"()I", device_vendor),
                Native::new0(c"getProductId", c"()I", device_product),
                Native::new0(c"getKeyboardType", c"()I", keyboard_type),
                Native::new0(c"isVirtual", c"()Z", virtual_device),
                Native::new1(c"supportsSource", c"(I)Z", device_supports),
                Native::new0(c"getMotionRanges", c"()Ljava/util/List;", motion_ranges),
                Native::new2(c"getMotionRange",c"(II)Landroid/view/InputDevice$MotionRange;",motion_range_source),
                Native::new1(
                    c"getMotionRange",
                    c"(I)Landroid/view/InputDevice$MotionRange;",
                    motion_range
                ),
            ],
        ),
        (
            c"android/view/InputEvent",
            vec![
                Native::new0(c"getEventTime", c"()J", time),
                Native::new0(c"getSource", c"()I", source),
                Native::new0(c"getDeviceId", c"()I", event_device_id),
                Native::new0(c"getDevice", c"()Landroid/view/InputDevice;", event_device),
            ],
        ),
        (
            c"android/view/KeyEvent",
            vec![
                Native::new0(c"getAction", c"()I", action),
                Native::new0(c"getKeyCode", c"()I", code),
                Native::new0(c"getScanCode", c"()I", scan),
                Native::new0(c"getRepeatCount", c"()I", repeat),
                Native::new0(c"getDownTime", c"()J", down),
                Native::new0(c"getFlags", c"()I", zero),
                Native::new0(c"getMetaState", c"()I", zero),
            ],
        ),
        (
            c"android/view/MotionEvent",
            vec![
                Native::new1(
                    c"obtain",
                    c"(Landroid/view/MotionEvent;)Landroid/view/MotionEvent;",
                    obtain
                ),
                Native::new0(c"getAction", c"()I", action),
                Native::new0(c"getActionMasked", c"()I", action),
                Native::new0(c"getPointerCount", c"()I", one),
                Native::new0(c"getHistorySize", c"()I", zero),
                Native::new1(c"getPointerId", c"(I)I", index),
                Native::new1(c"getToolType", c"(I)I", tool),
                Native::new0(c"getButtonState", c"()I", button_state),
                Native::new0(c"getMetaState", c"()I", zero),
                Native::new0(c"getDownTime", c"()J", down),
                Native::new1(c"getX", c"(I)F", x),
                Native::new1(c"getY", c"(I)F", y),
                Native::new0(c"getX", c"()F", x0),
                Native::new0(c"getY", c"()F", y0),
                Native::new1(c"getPressure", c"(I)F", pressure),
                Native::new0(c"getPressure", c"()F", pressure0),
                Native::new0(c"getTouchMajor", c"()F", unit_float),
                Native::new0(c"getTouchMinor", c"()F", unit_float),
                Native::new1(c"getTouchMajor", c"(I)F", unit_index),
                Native::new1(c"getTouchMinor", c"(I)F", unit_index),
                Native::new0(c"getSize", c"()F", unit_float),
                Native::new1(c"getSize", c"(I)F", unit_index),
                Native::new0(c"getOrientation", c"()F", zero_float),
                Native::new1(c"getOrientation", c"(I)F", zero_index),
                Native::new0(c"getRawX", c"()F", x0),
                Native::new0(c"getRawY", c"()F", y0),
                Native::new2(c"getAxisValue", c"(II)F", axis),
                Native::new1(c"getAxisValue", c"(I)F", axis0),
                Native::new0(c"recycle", c"()V", empty),
            ],
        ),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    let source = env.find_class(c"org/rudroid/runtime/InputSource");
    let device = env.static_method(source, c"getDevice", c"(I)Ljava/lang/Object;");
    let ids = env.static_method(source, c"getDeviceIds", c"()[I");
    let root = env.new_global_ref(source) as usize;
    env.delete_local_ref(source);
    let (false, Some(device), Some(ids)) = (root == 0, device, ids) else {
        env.delete_global_ref(root as *mut c_void);
        return -1;
    };
    if DEVICE_SOURCE
        .set(DeviceSource {
            class: root,
            device,
            ids,
        })
        .is_err()
    {
        env.delete_global_ref(root as *mut c_void);
    }
    0
}
