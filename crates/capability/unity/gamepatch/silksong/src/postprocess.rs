//! INPUT:  checked CustomSceneManager.Start, feature snapshot and player frame/stop events
//! OUTPUT:  scene-delayed postprocess and static Sprite disabling through common's rooted adapter
//! POS:    owns scene pruning schedules; retains metadata and counters, never managed objects
#![deny(unsafe_op_in_unsafe_fn)]

use super::{lock, InstallEvent, SilksongFeatures};
use rudroid_common::arm64_detour::Original;
use rudroid_common::gamepatch_support::install_detour;
use rudroid_common::host_api::HostApiV0;
use rudroid_common::il2cpp::{Class, Il2cpp, TYPE_VOID};
use rudroid_common::unity_objects::{BehaviourPruner, StaticSpritePruner};
use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;

type Start = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void);
static ORIGINAL: Original<Start> = Original::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static STOPPED: AtomicBool = AtomicBool::new(false);
static HOOKED: AtomicBool = AtomicBool::new(false);

pub(super) fn arm() {
    GENERATION.store(0, Ordering::Release);
    STOPPED.store(false, Ordering::Release);
    HOOKED.store(false, Ordering::Release);
}

unsafe extern "C-unwind" fn start(receiver: *mut c_void, method: *mut c_void) {
    if let Some(original) = ORIGINAL.get() {
        // SAFETY: validated instance void() hook; live receiver/MethodInfo forwarded on the guest stack.
        unsafe { original(receiver, method) };
        if !STOPPED.load(Ordering::Acquire) {
            GENERATION.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// # Safety
/// Same mapped-code and quiescent installation contract as the title's other hooks.
pub(super) unsafe fn install(api: Il2cpp, enabled: bool) -> Vec<InstallEvent> {
    if !enabled {
        return Vec::new();
    }
    let target = "CustomSceneManager.Start";
    let Some(method) = api
        .find_class_named(c"CustomSceneManager")
        .and_then(|class| api.method(class, c"Start", 0))
        .filter(|method| api.method_signature(*method, false, &[], TYPE_VOID))
    else {
        return vec![InstallEvent::TargetMissing {
            behavior: "postprocess component pruning",
            target: target.into(),
        }];
    };
    let Some(entry) = api.method_entry(method) else {
        return vec![InstallEvent::TargetMissing {
            behavior: "postprocess component pruning",
            target: target.into(),
        }];
    };
    // SAFETY: exact checked instance ABI; caller keeps target idle until original publication.
    match unsafe {
        install_detour(
            "postprocess component pruning",
            target,
            entry,
            &ORIGINAL,
            start,
        )
    } {
        Ok(result) => {
            HOOKED.store(true, Ordering::Release);
            vec![InstallEvent::Installed {
                behavior: "postprocess component pruning",
                target: target.into(),
                shape: result.shape,
            }]
        }
        Err(event) => vec![event],
    }
}

struct Engine {
    pruner: BehaviourPruner,
    sprites: Option<StaticSpritePruner>,
    classes: Vec<(&'static CStr, Class)>,
    generation: u64,
    remaining: u32,
}

thread_local! {
    static ENGINE: RefCell<Option<(usize, Option<Engine>)>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        BUSY.with(|b| b.set(false));
    }
}

pub(super) struct Runtime {
    host: HostApiV0,
    names: Vec<&'static CStr>,
    sprite_percent: u32,
    binding: Mutex<Option<Il2cpp>>,
    owner: Mutex<Option<ThreadId>>,
}

impl Runtime {
    pub(super) fn new(host: HostApiV0, features: &SilksongFeatures) -> Self {
        let names = [
            (features.disable_bloom, c"BloomOptimized"),
            (features.disable_camera_blur_plane, c"CameraBlurPlane"),
            (features.disable_light_blur, c"LightBlur"),
            (
                features.disable_light_blurred_background,
                c"LightBlurredBackground",
            ),
            (features.disable_uber_postprocess, c"UberPostprocess"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        Self {
            host,
            names,
            sprite_percent: features.decorative_sprite_percent as u32,
            binding: Mutex::new(None),
            owner: Mutex::new(None),
        }
    }
    fn log(&self, message: &str) {
        self.host.log_line(c"SILK", message);
    }
    pub(super) fn ready(&self, api: Il2cpp) {
        *lock(&self.binding) = Some(api);
    }
    pub(super) fn stop(&self, identity: usize) {
        // Cancellation precedes recursive-call protection; cleanup follows the in-flight call.
        STOPPED.store(true, Ordering::Release);
        *lock(&self.binding) = None;
        ENGINE.with(|slot| {
            if slot
                .borrow()
                .as_ref()
                .is_some_and(|(id, _)| *id == identity)
            {
                slot.replace(Some((identity, None)));
            }
        });
    }
    pub(super) fn frame(&self, identity: usize) {
        if STOPPED.load(Ordering::Acquire) || !HOOKED.load(Ordering::Acquire) {
            return;
        }
        let generation = GENERATION.load(Ordering::Acquire);
        if generation == 0 {
            return;
        }
        // Once this thread has consumed the generation (or disabled itself),
        // skip binding/owner mutexes, thread-id lookup and engine moves. A new
        // Start generation still re-arms the existing 30-frame delay. Other
        // threads have separate TLS and continue through the owner check.
        let idle = ENGINE.with(|slot| {
            slot.borrow().as_ref().is_some_and(|(id, state)| {
                *id == identity
                    && state
                        .as_ref()
                        .is_none_or(|state| state.generation == generation && state.remaining == 0)
            })
        });
        if idle {
            return;
        }
        let Some(api) = *lock(&self.binding) else {
            return;
        };
        {
            let current = std::thread::current().id();
            let mut owner = lock(&self.owner);
            if owner.is_some_and(|id| id != current) {
                return;
            }
            *owner = Some(current);
        }
        if BUSY.with(|b| b.replace(true)) {
            return;
        }
        let _guard = Guard;
        // Finished sentinel prevents reentry/unwind from reinitializing or repeating a prune.
        let previous = ENGINE.with(|slot| slot.replace(Some((identity, None))));
        let mut engine = match previous {
            Some((id, engine)) if id == identity => engine,
            _ => match BehaviourPruner::discover(api) {
                Ok(pruner) => {
                    let mut classes = Vec::new();
                    for &name in &self.names {
                        if let Some(class) = api.find_class_named(name) {
                            classes.push((name, class));
                        } else {
                            self.log(&format!(
                                "postprocess class unavailable: {}",
                                name.to_string_lossy()
                            ));
                        }
                    }
                    let sprites = if self.sprite_percent < 100 {
                        match StaticSpritePruner::from_behaviour(pruner.clone()) {
                            Ok(pruner) => Some(pruner),
                            Err(reason) => {
                                self.log(&format!("static Sprite runtime unavailable: {reason}"));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    self.log(
                        "postprocess runtime ready: scene-start delay=30 frames; rooted objects",
                    );
                    Some(Engine {
                        pruner,
                        sprites,
                        classes,
                        generation: 0,
                        remaining: 0,
                    })
                }
                Err(reason) => {
                    self.log(&format!("postprocess runtime unavailable: {reason}"));
                    None
                }
            },
        };
        if let Some(state) = engine.as_mut() {
            if state.generation != generation {
                state.generation = generation;
                state.remaining = 30;
            }
            if state.remaining > 0 {
                state.remaining -= 1;
                if state.remaining == 0 {
                    let cancelled = || {
                        STOPPED.load(Ordering::Acquire)
                            || GENERATION.load(Ordering::Acquire) != generation
                    };
                    let mut failed = false;
                    if let Some(sprites) = &state.sprites {
                        match sprites.prune(self.sprite_percent,is_static_decorative_name,cancelled) {
                            Ok((matched,disabled))=>self.log(&format!("static decorative sprites matched={matched} disabled={disabled} percent={} generation={generation}",self.sprite_percent)),
                            Err("cancelled")=>{},
                            Err(reason)=>{self.log(&format!("static Sprite runtime failed: {reason}; further Sprite pruning disabled"));state.sprites=None;}
                        }
                    }
                    for &(name, class) in &state.classes {
                        match state.pruner.disable(class,cancelled) {
                            Ok(count) => self.log(&format!("disabled postprocess class {} count={count} generation={generation}",name.to_string_lossy())),
                            Err("cancelled") => break,
                            Err(reason) => { self.log(&format!("postprocess runtime failed: {}: {reason}; further pruning disabled",name.to_string_lossy())); failed=true;break; }
                        }
                    }
                    if failed {
                        engine = None;
                    }
                }
            }
        }
        if !STOPPED.load(Ordering::Acquire) {
            ENGINE.with(|slot| slot.replace(Some((identity, engine))));
        }
    }
}

fn is_static_decorative_name(name: &str) -> bool {
    [
        "moss_",
        "Moss_",
        "Moss ",
        "bone_",
        "Bone_",
        "boneforest_",
        "Bonechurch_",
        "clump",
        "Clump",
        "simple_grass",
        "Simple Grass",
        "grass_",
        "green_grass",
        "haze",
        "fog",
        "temp fog",
        "silkcatcher_",
        "black_fader",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
#[path = "../tests/unit/postprocess.rs"]
mod tests;
