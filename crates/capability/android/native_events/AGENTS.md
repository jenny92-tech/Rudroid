# Native NDK fd events

> `android_native_events` supplies ALooper fd polling and reports sensors as unavailable.

## Role
Capability `android_native_events`, `Kind::Runtime`, `requires = android_elf_load`, artifact `libandroid_native_events.so` (cdylib + rlib). Optional: the Unity loader registers `rudroid_native_events_symbol` with `android_elf_load` before guest mapping, and `rudroid_plugin_init` prepares the host/UI thread's Looper before Unity `JNI_OnLoad`. Guest end-to-end coverage lives in `project/unity_loader/tests/native_events.rs`.

## Design
- The source answers only bare `ALooper_*` and `ASensor*` names; sensor queries expose no devices or event queues.
- One pthread TLS key holds each thread's Rust-owned Looper; `OwnedFd` manages its epoll and eventfd.
- Registrations live in a fallibly grown slot vector addressed by index plus a never-reused generation token.
- A poll classifies at most 16 kernel events under one lease, then revalidates and runs callbacks outside all locks; the poll path allocates nothing.
- `abi_gate.c` only checks epoll/pthread/time layouts at compile time (`archive: None`).

## Constraints
- `prepare`/`forThread` return a borrowed reference; use `acquire`/`release` across threads. Only the owning thread polls.
- The caller owns monitored fds, callbacks and userdata; release frees only registrations and internal fds. Never mix C and Rust allocators.
- Generation tokens stop a reused fd or stale callback from removing a newer registration; exhaustion reports `EOVERFLOW`.
- Removal cannot cancel a running callback; quiesce callbacks and polls before unmapping guest code.
- No poll-path compaction, second token registry or cache; Choreographer, hardware-buffer and physical sensor APIs are unsupported.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_native_events`; `common` and `libc` |
| Build script | `build.rs` | Compile-only layout gate via `native-build` (`-pthread`) |
| Source | `src/lib.rs` | Descriptor, host/UI Looper preparation, bare-name-only source |
| Event owner | `src/events.rs` | Looper/TLS owner, slots, generation tokens, allocation-free polling |
| ABI gate | `src/abi_gate.c` | Compile-time `epoll_event`, `pthread_key_t`, `timespec` checks |
| Unit tests | `tests/unit/` | Bare names, `prepare` rollback, tokens, refcounts, growth, allocation failures |
