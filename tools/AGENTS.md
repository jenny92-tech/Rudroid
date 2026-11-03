# Offline resource preparation

## Role

One public operation: `unity_assets.py SOURCE OUTPUT [--target astc-1g|astc-2g|astc-4g]`.
Default to 1 GiB; external callers can override the target in the CLI or `prepare()`.
Prepare a complete resource directory once; no sibling repository or runtime work.

## Design

The parent detects UnityFS by content, copies the full input layout and starts
one short-lived worker per bundle. The worker tries blocks from coarse to fine against the
original pixels, retains quality-passing bytes and generates their mip tail once.
Internal streamed textures stay streamed: append pixels while retaining each serialized
header and object offset. Inline/external layouts remain unchanged. Reopen and check
the exact layout, original stream prefixes and ownership of appended bytes before publication.
There are no caches, plan files, generation links or public intermediate commands.

## Constraints

- Read [usage](README.md). Targets require verified ASTC support; RAM does not
  establish GPU support. This is resource preparation, not a general game porter.
- Keep input bytes, dimensions, mip counts, stream owners, fonts, CPU-readable,
  unknown-readability, data and HDR textures intact. Never coarsen repeatedly
  from rejected candidates. Preserve unknown layouts byte-for-byte and report them.
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
| `unity_astc/profiles.py` | shared 1 GiB default and ASTC target/memory candidate lists |
| `unity_astc/texture_policy.py` | sensitive/color format gates and native-size/alpha quality |
| `unity_astc/astc_retier.py` | internal automatic conversion, font and stream checks |
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
