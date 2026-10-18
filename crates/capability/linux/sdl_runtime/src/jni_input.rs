//! INPUT:  creator-thread SDL keyboard/touch/controllers, Host Config and JNI factories
//! OUTPUT:  VM-owned events with device identity and Select+Start exit requests
//! POS:    owns creator-thread input, calibrated controller events and controller handle lifetimes
#![forbid(unsafe_code)]
use rudroid_common::{
    host_api::HostApiV0,
    jni_env::{Arg, JObject, JavaVm, JniEnv, Method, Native, JNI_VERSION_1_6},
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{c_void, CString},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Instant,
};
struct Bindings {
    factory: usize,
    key: Method,
    motion: Method,
    device: Method,
    fields: [usize; 10],
    device_fields: [usize; 6],
    extra_fields: [usize; 9],
    notifications: [Method; 3],
    vm: JavaVm,
}
static BINDINGS: OnceLock<Bindings> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static DEVICES: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
fn device_roots() -> &'static Mutex<HashMap<i32, usize>> {
    DEVICES.get_or_init(|| Mutex::new(HashMap::new()))
}
static INVENTORY: Mutex<Vec<(i32, String)>> = Mutex::new(Vec::new());
static CONTROLLER_CODES: OnceLock<Option<[i32; 15]>> = OnceLock::new();
static POLICY: OnceLock<super::input_policy::InputPolicy> = OnceLock::new();
pub(super) const DEFAULT_CONTROLLER_CODES: [i32; 15] = [
    97, 96, 100, 99, 109, 110, 108, 106, 107, 102, 103, 19, 20, 21, 22,
];
#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct Event {
    motion: bool,
    action: i32,
    a: i32,
    b: i32,
    repeat: i32,
    device: i32,
    gamepad: bool,
    axes: Option<[f32; 10]>,
    source: i32,
    button_state: i32,
}
struct InputTimes {
    keys: [Option<(i64, i32)>; 512],
    touch: i64,
}
impl InputTimes {
    fn stamp(&mut self, event: &mut Event, ms: i64) -> i64 {
        if event.motion {
            if event.action == 0 {
                self.touch = ms;
            }
            return self.touch;
        }
        let Some(key) = usize::try_from(event.a)
            .ok()
            .and_then(|i| self.keys.get_mut(i))
        else {
            return ms;
        };
        if event.action == 1 {
            let down = key.take().map(|v| v.0).unwrap_or(ms);
            event.repeat = 0;
            return down;
        }
        let entry = key.get_or_insert((ms, 0));
        if event.repeat != 0 {
            entry.1 = entry.1.saturating_add(1);
        } else {
            *entry = (ms, 0);
        }
        event.repeat = entry.1;
        entry.0
    }
}
struct Input {
    pump: sdl2::EventPump,
    start: Instant,
    script: VecDeque<(u64, Event)>,
    width: u32,
    height: u32,
    render: rudroid_common::display_dimensions::RenderDimensions,
    times: HashMap<i32, InputTimes>,
    quit: bool,
    controllers: ControllerState,
    pending: VecDeque<Event>,
    next_device: i32,
    retired: Vec<i32>,
    notifications: Vec<(usize, i32)>,
    mouse: MouseState,
    keyboard_chord: u8,
    // SDL subsystem and controller handles are creator-thread owners.
    _gamepads: Vec<sdl2::controller::GameController>,
    _controller_subsystem: Option<sdl2::GameControllerSubsystem>,
}
#[derive(Default)]
struct MouseState {
    buttons: i32,
}
impl MouseState {
    fn button(
        &mut self,
        button: sdl2::mouse::MouseButton,
        down: bool,
        x: i32,
        y: i32,
        policy: &super::input_policy::InputPolicy,
    ) -> VecDeque<Event> {
        use sdl2::mouse::MouseButton as B;
        let bit = match button {
            B::Left => 1,
            B::Right => 2,
            B::Middle => 4,
            B::X1 => 8,
            B::X2 => 16,
            _ => return VecDeque::new(),
        };
        let before = self.buttons;
        if down {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
        if self.buttons == before {
            return VecDeque::new();
        }
        let mut events = VecDeque::new();
        if policy.touch_mode {
            if (before == 0) != (self.buttons == 0) {
                let mut event = self.event(if down { 0 } else { 1 }, x, y, true);
                event.button_state = i32::from(down);
                events.push_back(event);
            }
        } else {
            if policy.accurate_mouse {
                events.push_back(self.event(if down { 11 } else { 12 }, x, y, false));
            }
            if before == 0 || self.buttons == 0 {
                if policy.accurate_mouse {
                    events.push_back(self.event(if down { 10 } else { 9 }, x, y, false));
                }
            }
            events.push_back(self.event(if down { 0 } else { 1 }, x, y, false));
        }
        events
    }
    fn motion(&self, x: i32, y: i32, policy: &super::input_policy::InputPolicy) -> Option<Event> {
        if policy.touch_mode && self.buttons == 0 {
            return None;
        }
        Some(self.event(
            if self.buttons == 0 { 7 } else { 2 },
            x,
            y,
            policy.touch_mode,
        ))
    }
    fn event(&self, action: i32, x: i32, y: i32, touch: bool) -> Event {
        Event {
            motion: true,
            action,
            a: x,
            b: y,
            source: if touch { 0x1002 } else { 0x2002 },
            button_state: if touch { 0 } else { self.buttons },
            ..Event::default()
        }
    }
}
struct ControllerState {
    ids: HashMap<u32, i32>,
    pressed: HashMap<u32, u32>,
    codes: [i32; 15],
    axes: HashMap<u32, [f32; 10]>,
    axis_pressed: HashMap<u32, u16>,
}
impl ControllerState {
    fn new(ids: HashMap<u32, i32>, codes: [i32; 15]) -> Self {
        Self {
            ids,
            pressed: HashMap::new(),
            codes,
            axes: HashMap::new(),
            axis_pressed: HashMap::new(),
        }
    }
    fn code(&self, index: usize) -> Option<i32> {
        let p = POLICY.get();
        let index = if (11..15).contains(&index) {
            11 + p.map_or(index - 11, |p| p.dpad[index - 11])
        } else {
            index
        };
        self.codes.get(index).copied()
    }
    fn held(&self, which: u32, code: i32) -> bool {
        let physical = self.pressed.get(&which).copied().unwrap_or(0);
        if (0..15).any(|index| physical & (1 << index) != 0 && self.code(index) == Some(code)) {
            return true;
        }
        let axes = self.axis_pressed.get(&which).copied().unwrap_or(0);
        let defaults = super::input_policy::InputPolicy::default();
        let p = POLICY.get().unwrap_or(&defaults);
        (0..10).any(|index| axes & (1 << index) != 0 && p.axes[index] == Some(code))
    }
    fn event(
        &mut self,
        which: u32,
        button: sdl2::controller::Button,
        down: bool,
    ) -> Option<(Event, bool)> {
        let device = *self.ids.get(&which)?;
        let index = button as usize;
        let code = self.code(index)?;
        let before = self.held(which, code);
        let mask = self.pressed.entry(which).or_default();
        if down {
            *mask |= 1 << index;
        } else {
            *mask &= !(1 << index);
        }
        let chord = (1 << sdl2::controller::Button::Back as usize)
            | (1 << sdl2::controller::Button::Start as usize);
        let quit = *mask & chord == chord && POLICY.get().is_none_or(|p| p.exit_chord);
        let after = self.held(which, code);
        if before == after && !quit {
            return None;
        }
        Some((
            Event {
                motion: false,
                action: i32::from(!after),
                a: code,
                b: 0,
                repeat: 0,
                device,
                gamepad: true,
                ..Event::default()
            },
            quit,
        ))
    }
    fn motion(&self, which: u32) -> Option<Event> {
        Some(Event {
            motion: true,
            action: 2,
            device: *self.ids.get(&which)?,
            gamepad: true,
            axes: Some(*self.axes.get(&which)?),
            ..Event::default()
        })
    }
    fn hat(&mut self, which: u32) -> Option<Event> {
        if !POLICY.get().is_none_or(|p| p.hats) {
            return None;
        }
        let mut dpad = [false; 4];
        let pressed = self.pressed.get(&which).copied().unwrap_or(0);
        for i in 0..4 {
            let target = POLICY.get().map_or(i, |p| p.dpad[i]);
            dpad[target] |= pressed & (1 << (11 + i)) != 0;
        }
        let axes = self.axes.entry(which).or_insert([0.0; 10]);
        axes[4] = f32::from(dpad[3] as u8) - f32::from(dpad[2] as u8);
        axes[5] = f32::from(dpad[1] as u8) - f32::from(dpad[0] as u8);
        self.motion(which)
    }
    fn axis(&mut self, which: u32, axis: sdl2::controller::Axis, value: i16) -> VecDeque<Event> {
        let defaults = super::input_policy::InputPolicy::default();
        self.axis_with_policy(which, axis, value, POLICY.get().unwrap_or(&defaults))
    }
    fn axis_with_policy(
        &mut self,
        which: u32,
        axis: sdl2::controller::Axis,
        value: i16,
        p: &super::input_policy::InputPolicy,
    ) -> VecDeque<Event> {
        use sdl2::controller::Axis as A;
        let Some(&device) = self.ids.get(&which) else {
            return VecDeque::new();
        };
        let (slot, directions, trigger) = match axis {
            A::LeftX => (0, [0, 1], false),
            A::LeftY => (1, [2, 3], false),
            A::RightX => (2, [4, 5], false),
            A::RightY => (3, [6, 7], false),
            A::TriggerLeft => (6, [8, 8], true),
            A::TriggerRight => (7, [9, 9], true),
        };
        let raw = super::input_policy::normalized(value, trigger);
        let axes = self.axes.entry(which).or_insert([0.0; 10]);
        axes[slot] = raw;
        if trigger {
            axes[slot + 2] = raw;
        }
        let mut events = VecDeque::new();
        if let Some(motion) = self.motion(which) {
            events.push_back(motion);
        }
        for (index, direction) in directions.into_iter().enumerate() {
            if trigger && index == 0 {
                continue;
            }
            let Some(code) = p.axes[direction] else {
                continue;
            };
            let before = self.held(which, code);
            let mask = self.axis_pressed.entry(which).or_default();
            let was = *mask & (1 << direction) != 0;
            let down = super::input_policy::direction_down(raw, trigger, index == 1, was);
            if down {
                *mask |= 1 << direction;
            } else {
                *mask &= !(1 << direction);
            }
            let after = self.held(which, code);
            if before != after {
                events.push_back(Event {
                    action: i32::from(!after),
                    a: code,
                    device,
                    gamepad: true,
                    ..Event::default()
                });
            }
        }
        events
    }
    fn remove(&mut self, which: u32) -> VecDeque<Event> {
        let Some(device) = self.ids.get(&which).copied() else {
            return VecDeque::new();
        };
        let defaults = super::input_policy::InputPolicy::default();
        let p = POLICY.get().unwrap_or(&defaults);
        let mut codes = self.codes.to_vec();
        codes.extend(p.axes.iter().flatten().copied());
        codes.sort_unstable();
        codes.dedup();
        let mut events: VecDeque<_> = codes
            .into_iter()
            .filter(|code| self.held(which, *code))
            .map(|code| Event {
                action: 1,
                a: code,
                device,
                gamepad: true,
                ..Event::default()
            })
            .collect();
        if self.axes.contains_key(&which) {
            events.push_back(Event {
                motion: true,
                action: 2,
                device,
                gamepad: true,
                axes: Some([0.0; 10]),
                ..Event::default()
            });
        }
        self.ids.remove(&which);
        self.pressed.remove(&which);
        self.axes.remove(&which);
        self.axis_pressed.remove(&which);
        events
    }
}
thread_local! {static INPUT:RefCell<Option<Input>>=const{RefCell::new(None)};}
fn script(value: &str) -> Result<VecDeque<(u64, Event)>, String> {
    let mut result = VecDeque::new();
    let mut previous = 0;
    for entry in value.split(';').filter(|entry| !entry.is_empty()) {
        if result.len() >= 1024 {
            return Err("input script exceeds 1024 events".into());
        }
        let parts: Vec<_> = entry.split(':').collect();
        if parts.len() != 5 {
            return Err("input script requires ms:key|touch:action:a:b".into());
        }
        let ms = parts[0].parse::<u64>().map_err(|_| "invalid input time")?;
        let action = parts[2]
            .parse::<i32>()
            .map_err(|_| "invalid input action")?;
        let a = parts[3]
            .parse::<i32>()
            .map_err(|_| "invalid input coordinate/key")?;
        let b = parts[4]
            .parse::<i32>()
            .map_err(|_| "invalid input coordinate/scan")?;
        if ms < previous
            || ms > 3_600_000
            || !(0..=2).contains(&action)
            || !matches!(parts[1], "key" | "touch" | "gamepad")
            || (parts[1] != "touch" && (!(0..512).contains(&a) || action == 2))
        {
            return Err("invalid or unsorted input event".into());
        }
        previous = ms;
        result.push_back((
            ms,
            Event {
                motion: parts[1] == "touch",
                action,
                a,
                b,
                repeat: 0,
                device: i32::from(parts[1] == "gamepad"),
                gamepad: parts[1] == "gamepad",
                ..Event::default()
            },
        ));
    }
    Ok(result)
}
pub(super) fn activate(
    sdl: &sdl2::Sdl,
    render: rudroid_common::display_dimensions::RenderDimensions,
) -> Result<(), String> {
    let mut script = script(&std::env::var("RUDROID_INPUT_SCRIPT").unwrap_or_default())?;
    let pump = sdl.event_pump()?;
    let codes = CONTROLLER_CODES
        .get()
        .copied()
        .unwrap_or(Some(DEFAULT_CONTROLLER_CODES));
    let mut gamepads = Vec::new();
    let mut inventory = Vec::new();
    let mut ids = HashMap::new();
    let controller_subsystem = if codes.is_some() {
        let subsystem = sdl.game_controller()?;
        for index in 0..subsystem.num_joysticks()? {
            if gamepads.len() >= 8 {
                break;
            }
            if !subsystem.is_game_controller(index) {
                continue;
            }
            let pad = subsystem.open(index).map_err(|error| error.to_string())?;
            let id = (gamepads.len() + 1) as i32;
            eprintln!(
                "[SDL] gamepad opened: device={id} instance={} name={}",
                pad.instance_id(),
                pad.name()
            );
            ids.insert(pad.instance_id(), id);
            inventory.push((id, pad.name()));
            gamepads.push(pad);
        }
        Some(subsystem)
    } else {
        None
    };
    if script.iter().any(|(_, event)| event.gamepad) {
        if codes.is_none() {
            return Err("gamepad test input requires input.controller=true".into());
        }
        if inventory.is_empty() {
            inventory.push((1, "Explicit scripted gamepad".into()));
        }
    }
    if let Some(policy) = POLICY.get() {
        for &(at, code) in &policy.startup {
            let device = if (96..=110).contains(&code) {
                inventory.first().map_or(0, |p| p.0)
            } else {
                0
            };
            script.push_back((
                at,
                Event {
                    a: code,
                    device,
                    gamepad: device != 0,
                    ..Event::default()
                },
            ));
            script.push_back((
                at + 20,
                Event {
                    action: 1,
                    a: code,
                    device,
                    gamepad: device != 0,
                    ..Event::default()
                },
            ));
        }
    }
    script.make_contiguous().sort_by_key(|(at, _)| *at);
    let next_device = inventory.iter().map(|(id, _)| *id).max().unwrap_or(0) + 1;
    INPUT.with(|input| -> Result<(), String> {
        let mut input = input.borrow_mut();
        if input.is_some() {
            return Err("SDL input already active on this thread".into());
        }
        *input = Some(Input {
            pump,
            start: Instant::now(),
            script,
            width: render.output_width as u32,
            height: render.output_height as u32,
            render,
            times: HashMap::new(),
            quit: false,
            controllers: ControllerState::new(ids, codes.unwrap_or(DEFAULT_CONTROLLER_CODES)),
            pending: VecDeque::new(),
            next_device,
            retired: vec![],
            notifications: vec![],
            mouse: MouseState::default(),
            keyboard_chord: 0,
            _gamepads: gamepads,
            _controller_subsystem: controller_subsystem,
        });
        Ok(())
    })?;
    *INVENTORY.lock().unwrap() = inventory;
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}
pub(super) fn deactivate() {
    ACTIVE.store(false, Ordering::Release);
    INPUT.with(|input| {
        input.borrow_mut().take();
    });
    INVENTORY.lock().unwrap().clear();
    if let Some(bindings) = BINDINGS.get() {
        if let Some(env) = bindings
            .vm
            .get_env(JNI_VERSION_1_6)
            .or_else(|| bindings.vm.attach_current_thread())
        {
            let roots = std::mem::take(&mut *device_roots().lock().unwrap());
            for (_, root) in roots {
                env.delete_global_ref(root as JObject);
            }
        }
    }
}
fn keycode(key: sdl2::keyboard::Keycode) -> Option<i32> {
    use sdl2::keyboard::Keycode as K;
    match key {
        K::Return => Some(66),
        K::Escape => Some(4),
        K::Space => Some(62),
        K::Up => Some(19),
        K::Down => Some(20),
        K::Left => Some(21),
        K::Right => Some(22),
        K::Tab => Some(61),
        K::Backspace => Some(67),
        _ => {
            let raw = key.into_i32();
            if (97..=122).contains(&raw) {
                Some(29 + raw - 97)
            } else {
                None
            }
        }
    }
}
extern "C" fn poll(env: JniEnv, _class: JObject) -> JObject {
    let retired = INPUT.with(|input| {
        let mut input = input.borrow_mut();
        let Some(input) = input.as_mut() else {
            return vec![];
        };
        if input.pending.is_empty() {
            std::mem::take(&mut input.retired)
        } else {
            vec![]
        }
    });
    for id in retired {
        INVENTORY
            .lock()
            .unwrap()
            .retain(|(device, _)| *device != id);
        let global = device_roots().lock().unwrap().remove(&id);
        if let Some(global) = global {
            env.delete_global_ref(global as JObject);
        }
        INPUT.with(|input| {
            if let Some(input) = input.borrow_mut().as_mut() {
                input.times.remove(&id);
            }
        });
        let bindings = BINDINGS.get().unwrap();
        env.call_static_void(
            bindings.factory as JObject,
            bindings.notifications[1],
            &[Arg::int(id)],
        );
    }
    let event = INPUT.with(|input| {
        let mut input = input.borrow_mut();
        let input = input.as_mut()?;
        let ms = input.start.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let mut event = if let Some(event) = input.pending.pop_front() {
            Some(event)
        } else if input.script.front().is_some_and(|(at, _)| *at <= ms as u64) {
            let (_, event) = input.script.pop_front().unwrap();
            eprintln!("[SDL] scripted input: ms={ms} event={event:?}");
            Some(event)
        } else {
            None
        };
        if event.is_none() {
            for _ in 0..128 {
                let Some(native) = input.pump.poll_event() else {
                    break;
                };
                use sdl2::event::Event as S;
                event = match native {
                    S::Quit { .. } => {
                        input.quit = true;
                        None
                    }
                    S::ControllerButtonDown { which, button, .. }
                    | S::ControllerButtonUp { which, button, .. } => {
                        let event = input
                            .controllers
                            .event(
                                which,
                                button,
                                matches!(native, S::ControllerButtonDown { .. }),
                            )
                            .map(|(event, chord)| {
                                input.quit |= chord;
                                event
                            });
                        if (11..15).contains(&(button as usize)) {
                            if let Some(hat) = input.controllers.hat(which) {
                                input.pending.push_back(hat);
                            }
                        }
                        event
                    }
                    S::ControllerAxisMotion {
                        which, axis, value, ..
                    } => {
                        input
                            .pending
                            .extend(input.controllers.axis(which, axis, value));
                        input.pending.pop_front()
                    }
                    S::ControllerDeviceAdded { which, .. } => {
                        if input._gamepads.len() < 8 {
                            if let Some(subsystem) = input._controller_subsystem.as_ref() {
                                if subsystem.is_game_controller(which) {
                                    if let Ok(pad) = subsystem.open(which) {
                                        let instance = pad.instance_id();
                                        if !input.controllers.ids.contains_key(&instance) {
                                            if let Some(next) = input.next_device.checked_add(1) {
                                                let id = input.next_device;
                                                input.next_device = next;
                                                let name = pad.name();
                                                input.controllers.ids.insert(instance, id);
                                                INVENTORY.lock().unwrap().push((id, name));
                                                input._gamepads.push(pad);
                                                input.notifications.push((0, id));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None
                    }
                    S::ControllerDeviceRemoved { which, .. } => {
                        let id = input.controllers.ids.get(&which).copied();
                        input.pending.extend(input.controllers.remove(which));
                        input._gamepads.retain(|pad| pad.instance_id() != which);
                        if let Some(id) = id {
                            input.retired.push(id);
                        }
                        input.pending.pop_front()
                    }
                    S::ControllerDeviceRemapped { which, .. } => {
                        if let Some(id) = input.controllers.ids.get(&which) {
                            input.notifications.push((2, *id));
                        }
                        None
                    }
                    S::KeyDown {
                        keycode: Some(key),
                        scancode,
                        repeat,
                        ..
                    }
                    | S::KeyUp {
                        keycode: Some(key),
                        scancode,
                        repeat,
                        ..
                    } => {
                        let down = !matches!(native, S::KeyUp { .. });
                        let bit = match key {
                            sdl2::keyboard::Keycode::Escape => 1,
                            sdl2::keyboard::Keycode::Return => 2,
                            _ => 0,
                        };
                        if down {
                            input.keyboard_chord |= bit;
                        } else {
                            input.keyboard_chord &= !bit;
                        }
                        input.quit |=
                            input.keyboard_chord == 3 && POLICY.get().is_none_or(|p| p.exit_chord);
                        POLICY
                            .get()
                            .and_then(|p| p.keyboard.get(&key).copied())
                            .or_else(|| keycode(key))
                            .map(|code| Event {
                                motion: false,
                                action: if matches!(native, S::KeyUp { .. }) {
                                    1
                                } else {
                                    0
                                },
                                a: code,
                                b: scancode.map(|s| s as i32).unwrap_or(0),
                                repeat: i32::from(repeat),
                                device: 0,
                                gamepad: false,
                                ..Event::default()
                            })
                    }
                    S::MouseButtonDown {
                        x,
                        y,
                        mouse_btn,
                        which,
                        ..
                    }
                    | S::MouseButtonUp {
                        x,
                        y,
                        mouse_btn,
                        which,
                        ..
                    } if which != u32::MAX => {
                        let defaults = super::input_policy::InputPolicy::default();
                        let policy = POLICY.get().unwrap_or(&defaults);
                        input.pending.extend(input.mouse.button(
                            mouse_btn,
                            matches!(native, S::MouseButtonDown { .. }),
                            x,
                            y,
                            policy,
                        ));
                        input.pending.pop_front()
                    }
                    S::MouseMotion { x, y, which, .. } if which != u32::MAX => {
                        let defaults = super::input_policy::InputPolicy::default();
                        input.mouse.motion(x, y, POLICY.get().unwrap_or(&defaults))
                    }
                    S::FingerDown { x, y, .. }
                    | S::FingerUp { x, y, .. }
                    | S::FingerMotion { x, y, .. } => Some(Event {
                        motion: true,
                        action: match native {
                            S::FingerDown { .. } => 0,
                            S::FingerUp { .. } => 1,
                            _ => 2,
                        },
                        a: (x * input.width as f32) as i32,
                        b: (y * input.height as f32) as i32,
                        repeat: 0,
                        device: 0,
                        gamepad: false,
                        ..Event::default()
                    }),
                    _ => None,
                };
                if event.is_some() {
                    break;
                }
            }
        }
        let mut event = event?;
        // Android input and Unity's native clock must share CLOCK_MONOTONIC.
        // Keep the private Instant only for relative test-script scheduling.
        let timestamp_ms = rudroid_common::clock::monotonic_nanos() / 1_000_000;
        if event.motion && event.axes.is_none() {
            (event.a, event.b) = input.render.point(event.a, event.b);
        }
        let down = input
            .times
            .entry(event.device)
            .or_insert(InputTimes {
                keys: [None; 512],
                touch: 0,
            })
            .stamp(&mut event, timestamp_ms);
        Some((event, timestamp_ms, down))
    });
    let notices = INPUT.with(|input| {
        input
            .borrow_mut()
            .as_mut()
            .map(|input| std::mem::take(&mut input.notifications))
            .unwrap_or_default()
    });
    let bindings = BINDINGS.get().unwrap();
    for (kind, id) in notices {
        env.call_static_void(
            bindings.factory as JObject,
            bindings.notifications[kind],
            &[Arg::int(id)],
        );
    }
    let Some((event, ms, down)) = event else {
        return std::ptr::null_mut();
    };
    let bindings = BINDINGS.get().unwrap();
    let factory = if event.motion {
        bindings.motion
    } else {
        bindings.key
    };
    let object = env.call_static_object(bindings.factory as JObject, factory, &[]);
    if object.is_null() {
        return object;
    }
    for (i, value) in [
        (0, event.action),
        (
            1,
            if event.source != 0 {
                event.source
            } else if event.axes.is_some() {
                0x01000010
            } else if event.gamepad {
                0x401
            } else if event.motion {
                0x1002
            } else {
                0x101
            },
        ),
        (2, if event.motion { 0 } else { event.a }),
        (3, if event.motion { 0 } else { event.b }),
        (4, event.repeat),
        (9, event.device),
    ] {
        env.set_int_field(object, bindings.fields[i] as *mut c_void, value);
    }
    env.set_long_field(object, bindings.fields[5] as *mut c_void, ms);
    env.set_long_field(object, bindings.fields[6] as *mut c_void, down);
    if event.motion {
        let axes = event.axes.unwrap_or([
            event.a as f32,
            event.b as f32,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ]);
        env.set_float_field(object, bindings.fields[7] as *mut c_void, axes[0]);
        env.set_float_field(object, bindings.fields[8] as *mut c_void, axes[1]);
        for (i, value) in axes[2..].iter().enumerate() {
            env.set_float_field(object, bindings.extra_fields[i] as *mut c_void, *value);
        }
        env.set_int_field(
            object,
            bindings.extra_fields[8] as *mut c_void,
            event.button_state,
        );
    }
    object
}
extern "C" fn quit(_env: JniEnv, _class: JObject) -> u8 {
    INPUT.with(|input| u8::from(input.borrow().as_ref().is_some_and(|input| input.quit)))
}
fn cached_device(env: JniEnv, id: i32) -> Option<JObject> {
    // Promote while the cache owns its global; removal takes this lock before deleting it.
    let roots = device_roots().lock().unwrap();
    roots
        .get(&id)
        .map(|root| env.new_local_ref(*root as JObject))
}
extern "C" fn device(env: JniEnv, _class: JObject, id: i32) -> JObject {
    if !ACTIVE.load(Ordering::Acquire) {
        return std::ptr::null_mut();
    }
    let name = if id == 0 {
        if let Some(local) = cached_device(env, id) {
            return local;
        }
        "Rudroid SDL virtual keyboard/touch".to_owned()
    } else {
        let inventory = INVENTORY.lock().unwrap();
        let Some((_, name)) = inventory.iter().find(|(device, _)| *device == id) else {
            return std::ptr::null_mut();
        };
        // Keep the inventory lease through promotion: a retired device cannot be returned
        // in the interval before removal also retires its cached global. Lock order matches
        // cold publication below (inventory, then roots). Copy the name only on a miss.
        if let Some(local) = cached_device(env, id) {
            return local;
        }
        name.clone()
    };
    {
        let bindings = BINDINGS.get().unwrap();
        let object = env.call_static_object(bindings.factory as JObject, bindings.device, &[]);
        if object.is_null() {
            return object;
        }
        env.set_int_field(object, bindings.device_fields[0] as *mut c_void, id);
        // Controller axes and hats have calibrated MotionRanges in the shared framework.
        env.set_int_field(
            object,
            bindings.device_fields[1] as *mut c_void,
            if id == 0 {
                0x101 | 0x1002 | 0x2002
            } else {
                0x401 | 0x201 | 0x01000010
            },
        );
        env.set_boolean_field(
            object, bindings.device_fields[3] as JObject,
            u8::from(id != 0 && POLICY.get().is_none_or(|policy| policy.hats)),
        );
        let defaults = super::input_policy::InputPolicy::default();
        let policy = POLICY.get().unwrap_or(&defaults);
        let (name, vendor, product) = if id == 0 {
            (name.as_str(), 0, 0)
        } else {
            policy.gamepad_identity(&name)
        };
        env.set_int_field(object, bindings.device_fields[4] as JObject, vendor);
        env.set_int_field(object, bindings.device_fields[5] as JObject, product);
        let name = env.new_string(&name.encode_utf16().collect::<Vec<_>>());
        env.set_object_field(object, bindings.device_fields[2] as *mut c_void, name);
        env.delete_local_ref(name);
        let new_root = env.new_global_ref(object) as usize;
        env.delete_local_ref(object);
        if new_root == 0 {
            return std::ptr::null_mut();
        }
        let (result, keep) = {
            let inventory = INVENTORY.lock().unwrap();
            let mut roots = device_roots().lock().unwrap();
            if ACTIVE.load(Ordering::Acquire)
                && (id == 0 || inventory.iter().any(|(device, _)| *device == id))
            {
                let root = *roots.entry(id).or_insert(new_root);
                (env.new_local_ref(root as JObject), root == new_root)
            } else {
                (std::ptr::null_mut(), false)
            }
        };
        if !keep {
            env.delete_global_ref(new_root as JObject);
        }
        result
    }
}
extern "C" fn device_ids(env: JniEnv, _class: JObject) -> JObject {
    let ids: Vec<i32> = if ACTIVE.load(Ordering::Acquire) {
        std::iter::once(0)
            .chain(INVENTORY.lock().unwrap().iter().map(|(id, _)| *id))
            .collect()
    } else {
        Vec::new()
    };
    let array = env.new_int_array(ids.len() as i32);
    if !ids.is_empty() && !array.is_null() {
        env.set_int_array_region(array, 0, &ids);
    }
    array
}
pub(super) fn register(api: &HostApiV0) -> i32 {
    let Some(vm) = api.java_vm() else {
        return 0;
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return -1;
    };
    let class = env.find_class(c"android/view/KeyEvent");
    let policy = super::input_policy::InputPolicy::read(api, |value| {
        let name = CString::new(if value.starts_with("KEYCODE_") {
            value.to_owned()
        } else {
            format!("KEYCODE_{value}")
        })
        .map_err(|_| "invalid input keycode".to_owned())?;
        let field = env.get_static_field_id(class, &name, c"I");
        if field.is_null() || env.exception_check() {
            return Err(format!("unknown input keycode {value}"));
        }
        Ok(env.get_static_int_field(class, field))
    });
    env.delete_local_ref(class);
    let policy = match policy {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[SDL] {e}");
            return -1;
        }
    };
    let codes = api
        .config_bool(c"input.controller", true)
        .then_some(policy.buttons);
    let _ = POLICY.set(policy);
    CONTROLLER_CODES.get_or_init(|| codes);
    let factory = env.find_class(c"org/rudroid/runtime/InputEvents");
    let key = env.static_method(factory, c"key", c"()Ljava/lang/Object;");
    let motion = env.static_method(factory, c"motion", c"()Ljava/lang/Object;");
    let device_method = env.static_method(factory, c"device", c"()Ljava/lang/Object;");
    let notifications = [c"deviceAdded", c"deviceRemoved", c"deviceChanged"]
        .map(|name| env.static_method(factory, name, c"(I)V"));
    let [Some(added), Some(removed), Some(changed)] = notifications else {
        env.delete_local_ref(factory);
        return -1;
    };
    let data = env.find_class(c"org/rudroid/runtime/InputDeviceData");
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
        device_fields[i] = env.get_field_id(data, name, sig) as usize;
    }
    env.delete_local_ref(data);
    let data = env.find_class(c"org/rudroid/runtime/InputEventData");
    let mut fields = [0; 10];
    for (i, (name, sig)) in [
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
    ]
    .iter()
    .enumerate()
    {
        fields[i] = env.get_field_id(data, name, sig) as usize;
    }
    env.delete_local_ref(data);
    let data = env.find_class(c"org/rudroid/runtime/InputEventData");
    let mut extra_fields = [0; 9];
    for (i, (name, sig)) in [
        (c"z", c"F"),
        (c"rz", c"F"),
        (c"hatX", c"F"),
        (c"hatY", c"F"),
        (c"leftTrigger", c"F"),
        (c"rightTrigger", c"F"),
        (c"brake", c"F"),
        (c"gas", c"F"),
        (c"buttonState", c"I"),
    ]
    .iter()
    .enumerate()
    {
        extra_fields[i] = env.get_field_id(data, name, sig) as usize;
    }
    env.delete_local_ref(data);
    if extra_fields.contains(&0) {
        return -1;
    }
    let root = env.new_global_ref(factory) as usize;
    env.delete_local_ref(factory);
    let (Some(key), Some(motion), Some(device_method), false, false, false) = (
        key,
        motion,
        device_method,
        root == 0,
        fields.contains(&0),
        device_fields.contains(&0),
    ) else {
        env.delete_global_ref(root as *mut c_void);
        return -1;
    };
    if BINDINGS
        .set(Bindings {
            factory: root,
            key,
            motion,
            device: device_method,
            fields,
            device_fields,
            extra_fields,
            notifications: [added, removed, changed],
            vm,
        })
        .is_err()
    {
        env.delete_global_ref(root as *mut c_void);
    }
    env.register_class(
        c"org/rudroid/runtime/InputSource",
        &[
            Native::new1(c"getDevice", c"(I)Ljava/lang/Object;", device),
            Native::new0(c"getDeviceIds", c"()[I", device_ids),
            Native::new0(c"poll", c"()Ljava/lang/Object;", poll),
            Native::new0(c"quitRequested", c"()Z", quit),
        ],
    )
}
#[cfg(test)]
#[path = "../tests/unit/jni_input.rs"]
mod tests;
