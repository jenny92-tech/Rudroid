# stderr log sink

> Deployable host log capability that writes `[tag] message` lines to stderr.

## Role
Capability `platform_log_stderr`, kind `logging`, no `requires`; deployed for Silksong. It is the deployable counterpart of the reference kit capability; the loader's Host API `log` already writes to stderr by default.

## Design
- `rudroid_log_stderr(tag, message)` writes one line with `eprintln!`; null renders as `<null>`, invalid UTF-8 as `<utf8>`.
- `rudroid_plugin_init` announces the sink through the Host API `log` and returns -1 for a null table.

## Constraints
- No SDL and no Android assumptions.
- Null or invalid UTF-8 input must never crash the host.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `platform_log_stderr`; depends on `rudroid-common` |
| Sink | `src/lib.rs` | Descriptor, `rudroid_log_stderr`, `rudroid_plugin_init` |
| Unit tests | `tests/unit/` | Direct writes, including null and invalid UTF-8 input |
