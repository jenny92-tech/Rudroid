# Offline resource preparation

## Role

`unity_assets.py SOURCE OUTPUT [--target astc-1g|astc-2g|astc-4g]` prepares a
complete resource directory once.
Default to 1 GiB; external callers can override the target in the CLI or `prepare()`.
`unity_port.py` runs the pinned PortMaster-Unity CLI for desktop Unity port authoring
and host preparation. Its `prepare` command composes upstream conversion and ARM64 Mono data packaging on a fresh working copy. Launcher Pack owns title-specific recipes and delivery.

Before adding another CLI or audit script, read the repo-wide registry
[TOOLING](../docs/TOOLING.md): reuse or extend entries here; do not invent a
parallel converter. Update TOOLING when this Contents table changes.

## Design

For `unity_assets.py`, the parent detects UnityFS by content and scans font roots/material edges in
short-lived workers before conversion. It retains only stable reference keys,
closes cross-bundle references, copies the full layout and starts one conversion
worker per bundle. The worker tries blocks from coarse to fine against the
original pixels, retains quality-passing bytes and generates their mip tail once.
Internal streamed textures stay streamed: append pixels while retaining each serialized
header and object offset. Inline/external layouts remain unchanged. Reopen and check
the exact layout, original stream prefixes and ownership of appended bytes before publication.
There are no caches, plan files, generation links or public intermediate commands.

`unity_port.py` verifies the Git HEAD or archive checksum and dispatches to upstream
`pmunity` with caller cwd and arguments intact. A local extension may use a
modified checkout or extracted tree, with its verification status reported.


## Constraints

- Read [usage](README.md). ASTC targets require verified ASTC support; RAM does not
  establish GPU support. This is resource preparation, not a general game porter.
- Keep original desktop data read-only by using a separate overlay or a complete
  working copy with `unity_port.py convert`. Keep upstream step and packaging
  behavior in PortMaster-Unity. Use only trusted local source, plugins and configs.
- Keep input bytes, dimensions, mip counts, stream owners, fonts, CPU-readable,
  unknown-readability, data and HDR textures intact. Never coarsen repeatedly
  from rejected candidates. Preserve unknown texture layouts byte-for-byte and report them
  after reliable font-reference scanning; uncheckable font/material metadata aborts
  the tree without publishing or modifying the input.
- Keep original streams and all unselected objects unchanged. Compare serialized
  payload hashes with the exact quality-checked bytes; verify full mip lengths.
- Publish only a complete directory. No input/output overlap, symlinks, prepared
  inputs or replacement of existing outputs. Detect source changes during work.
- File payload totals do not prove live RAM/FPS or gameplay acceptance. Do not
  enable runtime caps, audio rewrites or game-specific art changes here.
- First-party tools use the root noncommercial source-sharing LICENSE; preserve
  third-party licenses and copyright notices. Private game inputs and measurements
  belong under ignored target directories, not in repository documentation.

## Contents

| Path | Responsibility |
|------|----------------|
| `unity_assets.py` | single public CLI, complete-tree copying, isolation and publication |
| `unity_port.py` | pinned source verification, upstream CLI dispatch and the host preparation entry |
| `pmunity_rudroid/check.py` | read-only ARM64 assessment from the converter survey, with title references independent of route selection |
| `pmunity_rudroid/prepare.py` | working-copy conversion, validation and publication of ARM64 Mono game data |
| `patches/pmunity-mono64-no-thread-local-alloc.patch` | optional upstream ARM64 Boehm thread exit A/B configuration |
| `unity_astc/profiles.py` | shared 1 GiB default and ASTC target/memory candidate lists |
| `unity_astc/texture_policy.py` | sensitive/color format gates and native-size/alpha quality |
| `unity_astc/astc_retier.py` | internal automatic conversion, font and stream checks |
| `unity_astc/resource_refs.py` | Unity-compatible basename resolution with ambiguous-owner and invalid-path rejection |
| `unity_astc/image_encode.py` | preserve full mip chains without re-encoding the tested base |
| `unity_astc/validate_output.py` | independently reopen and verify serialized output |
| `unity_astc/audio_resource_validate.py` | internal external audio range/header checks |
| `unity_astc/repair_mips.py` | developer-only exact-pair repair, preserving every existing byte |
| `unity_astc/requirements.txt` | tested dependency pins |
| `audit_game_assets.py`, `audit_pck.py` | separate read-only developer audits |
| `tests/`, `unity_astc/tests/` | publication, format, quality, real codec and repair regressions |
| `README.md` | sole user guide |

Run both suites with the dependency-equipped Python:
`python -m unittest discover -s tools/tests` and
`python -m unittest discover -s tools/unity_astc/tests`. No Rust build or device deployment.
