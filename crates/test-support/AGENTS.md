# Test support

> Dev-only shared test library: serial lock, log capture, module-subscription fake, and the single path for nested cargo builds and guest C fixtures.

## Role
Package `rudroid-test-support`, a plain library with no descriptor or binary that never ships. It is only a `[dev-dependencies]` entry of `project/unity_loader`, `capability/unity/il2cpp_hook` and the `silksong`/`hollow_knight` gamepatches, and depends only on `rudroid-common`.

## Design
- `serial_lock()` (poison-tolerant per-binary mutex) and `LogCapture::shared()` (process-global `"tag: message"` buffer) cover process-global test state.
- `SubscriptionFake` mirrors the host subscription registry: several registrations per kind, `fire` calls targets outside the lock, mid-fire subscriptions apply next time.
- Fake Host API tables start from `HostApiV0::stubbed()` in `rudroid-common` and override only the entries a test exercises; this crate keeps no slot fillers.
- `cargo` resolves target dir, optional `CARGO_BUILD_TARGET` subdirectory and profile like cargo (including fixture/variant paths), runs `cargo build --locked`, replays `RUDROID_KIT_FEATURES`, and builds workspace cdylibs, out-of-workspace fixtures and `variants/<name>/`.
- `compile_c_fixture` compiles a guest C source with `gcc`/`cc` (`-shared -fPIC -nostdlib`) once per name into a per-process temp dir.

## Constraints
- Dev-dependency only; never add a descriptor, binary or production dependency edge.
- Never link a capability rlib: each exports `rudroid_plugin_descriptor`, so two in one test binary collide; SDL state comes from the caller.
- Every nested cargo build and guest C compile goes through this crate; no prebuilt artifacts via env vars, and variants never overwrite `<target>/<profile>`.
- Fixture `Cargo.lock` files must stay current (`--locked`); keep `RUDROID_KIT_FEATURES` parsing in sync with `docker-rust.sh` and `build-kit.sh`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library, no `[[bin]]`; depends on `rudroid-common` |
| Shared fakes | `src/lib.rs` | `serial_lock`, `LogCapture`, `SubscriptionFake` |
| Nested builds | `src/cargo.rs` | Target/profile resolution, kit features, workspace/fixture/variant builds, C fixtures |
| Unit tests | `tests/unit/` | Subscription fire semantics, kit feature parsing |
