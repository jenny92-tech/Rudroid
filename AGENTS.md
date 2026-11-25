# Rudroid

Handheld porting toolkit with a Rust runtime for ARM64 Android Unity libraries
on ARM64 Linux. The native loader runs Android IL2CPP titles and prepared Mono titles.

## Role

Implementation lives in `crates/`; offline resource preparation lives in `tools/`.
Bogodroid is the knowledge base; historical C++ behavior references live in its
Git history, not a required sibling source tree.
Read the [documentation index](docs/README.md), then the relevant module's AGENTS.
Current acceptance covers five bounded MiniLoong and TrimUI/A523 title paths.
All five validated launchers default to GLES and 75% rendering. Vulkan remains
a manual option for Silksong and Vampire Survivors after native preflight;
TrimUI/A523 Silksong uses full-size Vulkan compositor output to avoid scaled-scanout
stripes; older full-effects failures remain outside this bounded fix. TrimUI uses
an optional private Weston session and retains its 1 GiB limits. One-shot render
thread placement is limited to measured TrimUI/A523 Silksong topology and helps
both backends. Optional FMOD mixer placement is enabled only for this title's
measured A523/Vulkan path; Vulkan has no general frame-rate guarantee. ARM32
remains unverified. Prepared ARM64 Mono titles can select the [native runtime](crates/docs/UNITY-MONO.md).

## Design

`rudroid-host-core` orchestrates discovery, Host API and guest bring-up; the
thin Project CLI (`rudroid-unity-loader`) launches it. Capabilities cover
Android / Linux / Unity / optional engine profiles. Shared safe boundaries live
in common; runtime authority belongs to host-core and the shared VM.
Follow [DEVELOPMENT](DEVELOPMENT.md) and [the documentation protocol](FRACTAL-DOCS.md).
Build scripts, offline CLIs and debug switches are registered in
[TOOLING](docs/TOOLING.md); extend those entries instead of inventing parallel tools.

## Constraints

- Production device delivery is Release only, with positive per-image Release
  markers and no Debug/test/training members. Normal runs emit errors only;
  diagnostics require explicit private investigation (`RUDROID_DIAGNOSTICS=1`).
  After acceptance, rebuild and audit a complete quiet production kit before delivery.
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

## Workspace artifacts

Rudroid and Launcher Pack own implementation artifacts and their separate build environments.
Bogodroid only stores knowledge and project routing.

| Content | Location and retention |
|---------|------------------------|
| Maintained source | Declared source, tests, scripts, configs and durable technical documentation |
| Private original inputs | `gamefiles/<title>/` or an explicit external read-only input; never copies hidden in experiment caches |
| Fixed build cache | Only `crates/target/docker-linux-arm64/{target,cargo}`; reuse the fixed Docker entry and cache |
| Current experiment | `.scratch/<component>/`, one active task per component; no dated/ticket/Agent target trees |
| Accepted delivery | `dist/`, one accepted version per product/platform plus checksums and required source/licenses |

- Inventory relevant directories, sizes and active work before cleanup. Check tracked files, unique original inputs, saves and accepted packages before removing anything; a directory name alone is not proof of disposable data.
- Keep originals read-only. Use one task output, not successive resource clones, final/final2 trees or copied kit inventories. Put build identity in artifact filenames/manifests.
- Rebuildable caches have a soft limit of 10 GiB per project. Explain necessary excess and remove obsolete copies instead of creating another cache. Reuse an unfinished task slot; clear task outputs on completion.
- After delivery, retain one accepted package per target. Remove duplicate extracted kits, staging, device exports, screenshots and raw per-run measurements; consolidate durable findings into the owning document.
- Keep all test outputs inside the current task or fixed build cache, not beside source. Optional real-input tests use explicit input variables, not historical private-cache discovery.
- Self-cleaning OS/container temporary fixtures are allowed; do not retain copies as project output or enlarge environment quotas for them.
- Keep the current delivery inventory, sizes and file/tree checksums in `dist/ARTIFACTS.json`. Reuse documented images/containers/volumes, never global-prune or follow managed cache symlinks into other projects. Report package count/size and before/after project size; distinguish moving to Trash from freeing disk space. Do not empty system Trash automatically or deploy/run games for housekeeping.

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
