# Rudroid

Rust compatibility runtime for ARM64 Android Unity libraries on ARM64 Linux.

## Role

Implementation lives in `crates/`; offline resource preparation lives in `tools/`.
The sibling Bogodroid C++ tree is a read-only behavior reference, not a dependency.
Read the [documentation index](docs/README.md), then the relevant module's AGENTS.
Current acceptance covers five bounded MiniLoong title paths, not ARM32 or Mono.

## Design

A thin loader dynamically selects Android/Linux/Unity capabilities. Shared safe
boundaries live in common; runtime authority belongs to the loader/shared VM.
Follow [DEVELOPMENT](DEVELOPMENT.md) and [the documentation protocol](FRACTAL-DOCS.md).

## Constraints

- Rust build/check/test uses only `bash crates/scripts/docker-rust.sh` with image
  `rudroid-rust-gles:20260917`, container `rudroid-rust`, existing caches, 2 CPU / 4 GiB.
  No ad hoc environments, toolchain installation or global prune; see [build rules](BUILD-DOCKER.md).
- Never edit workspace sources, manifests or Cargo.lock during a Docker run:
  all members share the kit fingerprint. Deploy a complete audited production kit;
  test/training libraries cannot ship.
- Business code uses common's safe tools. Preserve native safety contracts,
  exact unsafe budgets, verified Unity ABI gates and ownership through teardown.
- New titles/devices follow [the porting guide](crates/docs/HANDHELD-PORTING-WORKFLOW.md).
  Reuse existing capabilities; verify loading, engine, display and gameplay separately.
  Within authorized scope, inspect, modify, build, test and read logs autonomously.
- Games, saves, caches and measurements are local inputs, never source. Optional
  real-game Docker inputs are read-only. Preserve third-party licenses/copyright.
- First-party code uses the root noncommercial source-sharing [LICENSE](LICENSE):
  commercial use is prohibited; distribution/network access requires public
  corresponding source, with the same license for derivatives. Third-party
  exceptions and older GPL copies are described in [NOTICE](NOTICE).

## Contents

| Path | Responsibility |
|------|----------------|
| `crates/` | Capabilities, loader, tests, native boundaries and build/deploy scripts |
| `tools/` | One-shot offline resource preparation and developer audits |
| `configs/`, `work/config.silksong.device.toml` | Config contract and regression samples |
| `docs/README.md` | Current guides and reference sources |
| `DEVELOPMENT.md`, `BUILD-DOCKER.md` | Development constraints, necessary pitfalls and build/deploy workflow |
| `CONTEXT.md`, `FRACTAL-DOCS.md` | Terms, source headers and navigation rules |
| `LICENSE`, `NOTICE`, `CONTRIBUTING`, `THIRD-PARTY-NOTICES`, `crates/LICENSE.md`, `crates/AUTHORS` | License scope, locked dependency notices and attribution |
| `.gitignore`, `.gitattributes` | Exclude local artifacts and preserve verbatim license/vendor bytes |
