# Unity loader

> Thin Unity Project CLI over `rudroid-host-core`: discovers nothing itself; re-exports the host kernel for tests and launches the registered engine profile lifecycle.

## Role
Package `rudroid-unity-loader`: binaries `rudroid-unity-loader` (`src/main.rs`) and `rudroid-loader` (`src/bin/rudroid-loader.rs`) share `cli::run_cli`, plus a `#![forbid(unsafe_code)]` library that re-exports host-core. Not a capability, no descriptor. Deploy Selection copies it with `--include-loader` (prefers `rudroid-unity-loader`, accepts `rudroid-loader`).

## Design
- Host kernel (discovery, Host API, guest plan, registry, SDL session, engine prepare/bootstrap/run) lives in `crates/host/`.
- This crate owns CLI argument parsing, the `unityloader <toml>` launcher alias, and orchestration that calls host-core.
- `--check` validates kit topology. With `--config` it also checks generic HostConfig render/memory. It does not run phases, register-JNI, or `EngineProfile::validate_config`. Engine-specific Unity/Mono checks run only in run mode after register-JNI.
- Integration tests stay here and depend on the re-export surface so paths/fixtures under `tests/` remain stable. `rudroid-unity-common` is a `dev-dependency` only.

## Constraints
- Library `src/lib.rs` forbids unsafe. CLI bodies (`cli.rs`, thin bins) use `deny(unsafe_op_in_unsafe_fn)` and call `unsafe { load_deploy_dir(...) }` / `unsafe { guest::bring_up_observed(...) }` with `// SAFETY:` at the operator trust boundary.
- Do not reintroduce host kernel modules or a production `rudroid-unity-common` dependency here.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Dual bins; prod deps host-core / common / host-config; unity-common dev-only |
| Library | `src/lib.rs` | Re-exports of host-core for CLI and integration tests |
| Shared CLI | `src/cli.rs` | Argument parsing and run/check orchestration |
| Binaries | `src/main.rs`, `src/bin/rudroid-loader.rs` | Thin entries exporting release markers |
| Integration tests | `tests/*.rs` | Guest, JNI, profile lifecycle, native probes |
| Fixtures | `tests/fixtures/` | Guest C probes and out-of-workspace fixture crates |
