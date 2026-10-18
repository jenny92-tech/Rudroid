//! INPUT:  private items of `src/jni_input.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/jni_input.rs`; declared there with `#[path]`
use super::*;
#[test]
fn held_keys_repeats_and_touch_keep_independent_down_times() {
    let mut times = InputTimes {
        keys: [None; 512],
        touch: 0,
    };
    let mut touch = Event {
        motion: true,
        action: 0,
        a: 1,
        b: 1,
        repeat: 0,
        device: 0,
        gamepad: false,
        ..Event::default()
    };
    assert_eq!(times.stamp(&mut touch, 10), 10);
    let mut a = Event {
        motion: false,
        action: 0,
        a: 29,
        b: 0,
        repeat: 0,
        device: 0,
        gamepad: false,
        ..Event::default()
    };
    assert_eq!(times.stamp(&mut a, 20), 20);
    let mut b = Event { a: 30, ..a };
    assert_eq!(times.stamp(&mut b, 30), 30);
    a.repeat = 1;
    assert_eq!(times.stamp(&mut a, 40), 20);
    assert_eq!(a.repeat, 1);
    assert_eq!(times.stamp(&mut a, 50), 20);
    assert_eq!(a.repeat, 2);
    touch.action = 1;
    assert_eq!(times.stamp(&mut touch, 60), 10);
    a.action = 1;
    assert_eq!(times.stamp(&mut a, 70), 20);
    assert!(times.keys[29].is_none());
    b.action = 1;
    assert_eq!(times.stamp(&mut b, 80), 30);
}

#[test]
fn controller_events_keep_device_ids_remaps_and_exit_chords_separate() {
    use sdl2::controller::Button as B;
    let mut codes = DEFAULT_CONTROLLER_CODES;
    codes[B::A as usize] = 96;
    let mut state = ControllerState::new(HashMap::from([(42, 1), (43, 2)]), codes);
    let (event, quit) = state.event(42, B::A, true).unwrap();
    assert_eq!(
        (event.device, event.a, event.action, event.gamepad, quit),
        (1, 96, 0, true, false)
    );
    assert_eq!(state.event(42, B::A, false).unwrap().0.action, 1);
    assert_eq!(state.event(42, B::DPadLeft, true).unwrap().0.a, 21);
    assert!(!state.event(42, B::Start, true).unwrap().1);
    assert!(!state.event(43, B::Back, true).unwrap().1);
    assert!(state.event(42, B::Back, true).unwrap().1);
    assert!(!state.event(42, B::Back, false).unwrap().1);
    assert!(state.event(99, B::A, true).is_none());
}

#[test]
fn explicit_gamepad_timelines_use_the_gamepad_device() {
    let events = script("10:gamepad:0:96:0;20:gamepad:1:96:0").unwrap();
    assert!(events
        .iter()
        .all(|(_, event)| event.device == 1 && event.gamepad && !event.motion));
    assert!(script("10:gamepad:2:96:0").is_err());
}
#[test]
fn timelines_are_bounded_ordered_and_explicit() {
    assert_eq!(
        script("10:touch:0:160:180;20:touch:1:160:180")
            .unwrap()
            .len(),
        2
    );
    for bad in [
        "20:key:0:66:0;10:key:1:66:0",
        "1:unknown:0:0:0",
        "1:touch:9:0:0",
        "1:touch:0:0",
    ] {
        assert!(script(bad).is_err());
    }
    assert!(script(&vec!["1:key:0:66:0"; 1025].join(";")).is_err());
}

#[test]
fn calibrated_axes_keep_raw_values_and_release_on_removal() {
    use sdl2::controller::Axis as A;
    let mut state = ControllerState::new(HashMap::from([(42, 1)]), DEFAULT_CONTROLLER_CODES);
    let axis = state.axis(42, A::LeftX, i16::MIN);
    assert_eq!(axis[0].axes.unwrap()[0], -1.0);
    let down = state.axis(42, A::TriggerLeft, i16::MAX);
    assert_eq!(down[0].axes.unwrap()[6], 1.0);
    assert_eq!(down[0].axes.unwrap()[8], 1.0);
    assert_eq!((down[1].a, down[1].action), (104, 0));
    assert_eq!(
        state.axis(42, A::TriggerLeft, 15000).len(),
        1,
        "hysteresis keeps the trigger held"
    );
    let released = state.remove(42);
    assert!(released.iter().any(|e| e.a == 104 && e.action == 1));
    assert_eq!(released.back().unwrap().axes, Some([0.0; 10]));
    assert!(state.axis(42, A::LeftX, 100).is_empty());
}

#[test]
fn analog_only_triggers_preserve_both_axis_aliases_and_neutral_without_keys() {
    use sdl2::controller::Axis as A;
    let mut policy = super::super::input_policy::InputPolicy::default();
    policy.axes[8..].fill(None);
    let mut state = ControllerState::new(HashMap::from([(42, 1)]), policy.buttons);
    for (axis, slot) in [(A::TriggerLeft, 6), (A::TriggerRight, 7)] {
        for (raw, expected) in [(i16::MAX, 1.0), (16000, 16000.0/32767.0), (0, 0.0)] {
            let events = state.axis_with_policy(42, axis, raw, &policy);
            assert_eq!(events.len(), 1, "no synthetic or unknown-key event");
            assert!(events[0].motion);
            let axes = events[0].axes.unwrap();
            assert_eq!(axes[slot], expected);
            assert_eq!(axes[slot+2], expected);
        }
    }
    assert_eq!(state.axis_pressed.get(&42).copied().unwrap_or(0), 0);
    let released = state.remove(42);
    assert!(released.iter().all(|event| event.motion));
    assert_eq!(released.back().unwrap().axes, Some([0.0; 10]));
}
#[test]
fn opposing_hat_buttons_and_shared_key_mappings_do_not_stick() {
    use sdl2::controller::Button as B;
    let mut codes = DEFAULT_CONTROLLER_CODES;
    codes[B::A as usize] = 104;
    let mut state = ControllerState::new(HashMap::from([(42, 1)]), codes);
    state.event(42, B::DPadLeft, true);
    assert_eq!(state.hat(42).unwrap().axes.unwrap()[4], -1.0);
    state.event(42, B::DPadRight, true);
    assert_eq!(state.hat(42).unwrap().axes.unwrap()[4], 0.0);
    state.event(42, B::A, true).unwrap();
    assert_eq!(
        state
            .axis(42, sdl2::controller::Axis::TriggerLeft, i16::MAX)
            .len(),
        1,
        "same logical key is already down"
    );
    assert!(
        state.event(42, B::A, false).is_none(),
        "trigger still owns the logical key"
    );
    let released = state.axis(42, sdl2::controller::Axis::TriggerLeft, 0);
    assert_eq!((released[1].a, released[1].action), (104, 1));
}
#[test]
fn mouse_touch_and_accurate_modes_keep_distinct_sequences() {
    use sdl2::mouse::MouseButton as B;
    let mut mouse = MouseState::default();
    let mut policy = super::super::input_policy::InputPolicy::default();
    assert_eq!(mouse.motion(10, 20, &policy).unwrap().action, 7);
    policy.touch_mode = true;
    assert!(mouse.motion(10, 20, &policy).is_none());
    assert_eq!(
        mouse.button(B::Right, true, 10, 20, &policy)[0].source,
        0x1002
    );
    assert!(mouse.button(B::Left, true, 10, 20, &policy).is_empty());
    assert!(mouse.button(B::Right, false, 10, 20, &policy).is_empty());
    assert_eq!(mouse.button(B::Left, false, 10, 20, &policy)[0].action, 1);
    policy.touch_mode = false;
    policy.accurate_mouse = true;
    let down = mouse.button(B::Left, true, 10, 20, &policy);
    assert_eq!(
        down.iter().map(|e| e.action).collect::<Vec<_>>(),
        [11, 10, 0]
    );
    assert!(down
        .iter()
        .all(|e| e.button_state == 1 && e.source == 0x2002));
    assert_eq!(
        mouse
            .button(B::Left, false, 10, 20, &policy)
            .iter()
            .map(|e| e.action)
            .collect::<Vec<_>>(),
        [12, 9, 1]
    );
}
