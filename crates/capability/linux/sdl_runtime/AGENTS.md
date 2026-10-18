# SDL Linux runtime

> SDL backend that owns the window/GLES session and serves guest EGL/GLES/ANativeWindow, audio and input from it.

## Role
Capability `platform_sdl_runtime`, kind `platform`, no `requires`; deployed for Silksong. The loader dlsyms its C API (names in `c_api::symbols`, mirrored by loader constants under a parity test). The real backend needs the optional `sdl` feature, enabled by the device build through the kit features.

## Design

- Startup selects and verifies a real GLES context, reporting measured drawable
  dimensions. One verification frame draws Jenny92's half-body purple character and uppercase RUDROID;
  startup textures/VBO/shaders are released before guest activation. No animation
  or forced delay. Builds without `sdl` export explicit failure stubs.
- Optional Wayland native buffers preserve exact guest size and require linear
  presentation; failure releases partial state before the loader retries internal
  presentation. Use a named initialized C display-mode value until SDL copies it;
  SDL2 0.37's temporary-pointer wrapper produced corrupt native dimensions.
  Query pixel attributes only with a current context; reset requests on every exit.
- EGL/ANativeWindow and scaled GL share one geometry/context authority. Driver
  tables are pinned, GL strings initialized once, version/capability policies
  immutable. Advertising a version does not create driver support.
- Primary window binding and presentation go through SDL, including KMSDRM's
  GBM/DRM page flip; never add a second raw EGL swap. Worker pbuffers use native
  EGL with SDL thread-local bindings cleared, and failed binds restore the
  previous owner. Guest surface identity survives SDL native-surface replacement.
- Shader/texture plans hold bounded context/share-group metadata. Preserve linked
  executables through compile failure/deferred deletion, validate CPU/PBO spans,
  restore guest bindings/errors and call native GL outside state locks. Scaling
  retains per-call snapshots rather than a persistent guessed GL-state cache.
- `gpu.renderScaleHalfPrecision` defaults false and selects sharp RGB arithmetic
  once at program creation. UVs, the center sample and alpha stay highp; the
  five taps, strength, buffers and state restoration are unchanged. Device/game
  A/B evidence is required before enabling it in a deployment profile.
- The loader publishes audio policy before producers start. SDL owns the callback
  and bounded SPSC ring; JNI converts PCM and uses callback-period backpressure.
  A one-second queue timeout rejects a block with zero accepted frames and one
  diagnostic per output; it retains the device for recovery. Invalid buffers,
  closed outputs and submission failures remain errors; the queue never grows during a stall.
  Input stays on the SDL creator thread, with calibrated axes, HAT/remaps,
  hotplug release, single touch and Select+Start exit handling.
- Device identity is a cold opt-in policy. Promote device roots under the owner
  lock; removal releases keys/axes before retiring inventory and globals.
- Capture/trace/statistics are opt-in. Ordinary shader/texture evidence is bounded;
  no diagnostic worker or periodic gameplay I/O is added.
- Optional `vulkan` selects a separate Wayland preflight and typed Android surface
  bridge. GLES remains default. Native WSI/Vulkan/SDL query libraries stay pinned;
  preflight reports zero game frames. Pixel-size queries are resolved optionally,
  preserving ordinary GLES loading on older SDL. The candidate requires SDL 2.26+.
- Unity's unconditional EGL imports share the existing checked facade even in
  Vulkan mode. Without a GLES session they report uninitialized; never forward
  Android window tokens to native Wayland EGL functions.

## Constraints
- One guest graphics session per process. Create, activate and destroy on the SDL creating thread, only after guest threads stop calling and release their EGL bindings.
- Render geometry is configured before activation; output stays SDL-owned while EGL/NDK/Java report shared guest dimensions. GLES 3 uses a private framebuffer plus sharp or linear presentation with guest state restoration. Input maps output coordinates to render coordinates. Scaling requires GLES 3; 100% remains available on GLES 2. Mixed primary-window/pbuffer read/draw bindings are rejected while scaling is active.
- Optional compositor presentation uses an exact guest-sized native drawable, so the existing unscaled EGL/NDK/GL path and SDL-transformed input coordinates apply directly. It cannot implement the internal sharp filter; the loader's typed configuration requires an explicit linear choice. Default presentation and user FPS/resolution choices stay independent; Wayland mode support and visual quality require device acceptance.
- `gpu.disableFramebufferInvalidate` is an immutable opt-in policy suppressing the three reference invalidation/discard hints before scaled/native resolution, including canonical glad aliases. These safe entries never access or retain guest attachments; disabling the policy preserves existing native resolution. It does not unload resources. Program-binary suppression, extension filtering and capability limits now have real consumers; shader policies now have consumers in a separate native boundary. Copied TOML does not prove C behavior or equal rendering workloads.
- The host owns the primary display/context/surface; guest destroy never frees them. Callbacks and returned symbol addresses stay valid for the process.
- Vulkan uses internal presentation and one live guest instance/device with the
  explicit private WSI library path. Exact/75%/50% rendering uses a verified
  window-local Wayland native mode and linear compositor scaling before guest
  activation; pointer coordinates are already SDL-transformed. The immutable
  creation report retains the physical output size. Vulkan render setup is
  one-shot, including failures: retry/change modes by recreating the session.
  A failed attempt clears its render contract before activation. Validate the
  physical buffer, not SDL's pre-first-commit closest logical desktop mode.
  GLES's sharp setting is retained.
  Vulkan children and GPU work must end before creator-thread window destruction.
  GLES shader/texture transformations do not apply to native Vulkan commands.
- The cold graphics-features export reports compiled support, without SDL init,
  hardware probes or guest execution. Driver and title support remain separate.
- Use GL tables only while their context is current and null-check every address. C report strings borrow the session until unsafe `session_destroy`, whose caller must establish guest quiescence.
- The audio callback never allocates, does I/O, logs or holds a lock across external calls. One producer per handle; JNI fields store integer ids only.
- Current RK3566 five-title movement, process-audio and exit evidence is bounded in [acceptance](../../../docs/ACCEPTANCE.md). Bridge probes separately cover axes and synthetic hotplug; physical unplugging, other controllers, speaker quality and every gameplay action still need their own evidence.
- Not a full EGL/Android window (no config selection, resize, `toSurface`, buffer lock). No multi-touch. Axis/hotplug behavior is implemented but physical unplugging and title-specific analog controls require separate acceptance. Xvfb or dummy-audio evidence does not prove device playability.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `platform_sdl_runtime`; optional `sdl` feature |
| Crate root | `src/lib.rs` | Descriptor, window config and GLES ladder, JNI registration, C API |
| Window session | `src/window.rs` | SDL window, GLES context verification, session report |
| Startup brand | `src/startup_brand.rs`, `assets/` | Safe one-shot RUDROID layout, approved empty-handed Jenny92 half-body character and embedded RGBA8 pixels |
| Guest graphics | `src/guest_graphics.rs` | EGL facade, GL address cache, ANativeWindow, swap, frame capture |
| Vulkan bridge | `src/vulkan.rs` | Optional typed native Vulkan dispatch, Android-to-Wayland surface, actual presentation notifications and process-pinned tables |
| Render scaling | `src/render_scale.rs` | Typed native GL objects, scaled default framebuffer, sharp/linear presentation and quiescent cleanup |
| Scaled GL facade | `src/scaled_gl.rs` | Default-buffer binds/queries, attachment translation and private framebuffer ownership |
| GLES identity | `src/gles_identity.rs` | Safe immutable version policy; native string/integer forwarding stays in guest_graphics |
| GLES capabilities | `src/gles_policy.rs` | Safe immutable binary/cache/extension/limit policy, process-pinned driver snapshots |
| Shader transformations | `src/shader_policy.rs` | Safe opt-in GLSL/refraction/screen-sprite transformations |
| Shader lifecycle | `src/shader_state.rs` | Safe bounded executable metadata and unique native context/share-group identity |
| Native shader facade | `src/shader_gl.rs` | Pinned GLES prototypes, source/compile/link/binary/deferred-delete bridges and four reference core draw wrappers |
| Texture policy | `src/texture_policy.rs` | Safe caps, exclusions, checked unpack spans, mip/LOD rebasing and CPU filters |
| Texture lifecycle | `src/texture_state.rs` | Bounded reserved metadata keyed by unique share group and texture name |
| Native texture facade | `src/texture_gl.rs` | Typed uploads, PBO mapping leases, failure/error preservation, parameter mapping and deletion |
| Invalidation policy | `src/invalidate_policy.rs` | Immutable opt-in safe no-op framebuffer hints, native fallback and bounded call evidence |
| Audio policy pump | `src/audio.rs` | Policy validation and dummy/file sinks |
| Audio device | `src/real_audio.rs` | SDL callback device, SPSC ring, stats and spec report |
| JNI audio | `src/jni_audio.rs` | `AudioOutput` natives and PCM16 → f32 conversion |
| JNI input | `src/jni_input.rs` | Creator-thread calibrated axes/buttons, hotplug release/inventory/root ownership, mouse modes, exit chord and input script |
| Input policy | `src/input_policy.rs` | Checked reference aliases, dpad/keyboard mappings, axis-key hysteresis and bounded opt-in startup keys |
| Unit tests | `tests/unit/` | Config ladder, Xvfb ES session, EGL facade, audio ring/pump and physical callback probe, input |
| Integration tests | `tests/*.rs` | C API via dlopen, dummy/disk devices, open failure, buffer A/B, report cache |
