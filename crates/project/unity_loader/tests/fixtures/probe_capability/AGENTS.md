# Loader phase probe fixture

> Native android_base-kind fixture for CLI config, C-string and cwd checks; never deployed in a production kit.

## Role
Out-of-workspace test crate built through `test-support::cargo::build_loader_fixture`.

## Design
Early init announces phase order. Init reads the installed Host Config and
walks synthetic environment strings as a C consumer. When
`RUDROID_EXPECT_GAME_CWD` is supplied, it verifies both the process cwd and
the installed absolute game-root leaf before returning success.

## Constraints
Only synthetic fixture inputs; no game data or JNI services. Keep the
descriptor/test fixture out of production Deploy Selections.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Standalone fixture, depends on common |
| Native probe | `src/lib.rs` | Phase/config/string checks and optional game cwd assertion |
