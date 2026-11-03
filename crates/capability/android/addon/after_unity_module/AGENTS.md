# After-Unity guest module chain

> The post-Unity guest-library mapping logic shared by the Burst, Unity Helpers and XLua addons, named after the C++ `AfterUnityGuestModule`.

## Role
Plain library, no descriptor; package `rudroid-android-addon-after-unity-module`. [`unity_burst`](../unity_burst/), [`unity_helpers`](../unity_helpers/) and [`xlua`](../xlua/) each link their own copy (statics are per addon); it is never deployed alone.

## Design
- An addon describes itself with a `GuestModule`: `library` (path under `paths.game_files`, also its event name), `log_tag`, `ModuleKind` and rejection-log `label`.
- `plugin_init_raw`/`plugin_init` keep a process-lifetime `AddonState` in a `HandleTable` (the subscription `user` is its handle) and subscribe to the Unity event and the addon's own event (null table -1, ABI mismatch -2, rejected subscription -3).
- First Unity event checks the file: absent gives `SkippedMissing`, present waits for the own event; unset `paths.game_files` stays `Pending`.
- First own event sets `Mapped` only if `base != 0` and its name equals `library`, else `Failed`; it overrides an earlier `SkippedMissing`. Repeats are ignored.
- The crate neither loads nor maps files: the loader maps through the `android_elf_load` provider and fires the events.

## Constraints
- Keep it a plain library (no descriptor, no cdylib); addon-specific data belongs in the thin addon crates.
- Only observed loader events or real file checks change state; never derive `Mapped` from an argument.
- A present but broken file is the ELF provider's failure to report; the addon stays `Pending`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library; depends on `rudroid-common` |
| Mapping chain | `src/lib.rs` | `GuestModule`, `MappingOutcome`, event handlers, `plugin_init`, test reset |
| Unit tests | `tests/unit/` | Stub Host API: missing file, load identity, idempotency, init |
