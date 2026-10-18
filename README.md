# Rudroid

Rudroid runs ARM64 Android Unity native libraries on ARM64 Linux through a thin
Rust loader and optional capabilities for ELF, Bionic, JNI, graphics, audio and input.
The implementation lives in [crates/](crates/AGENTS.md); the separate Bogodroid C++
repository is a read-only behavioral reference.

Rudroid's first-party code is available for **noncommercial use**. Distributing
source or binaries, or providing network access to a modified version, requires
public corresponding source and the same license for derivatives. See
[LICENSE](LICENSE) and [NOTICE](NOTICE).

## Current status

Five titles have bounded gameplay, input, audio, save and exit checks on
MiniLoong / RK3566 / Mali-G52 / about 4 GiB, and TrimUI / Allwinner A523 /
Mali-G57 / about 1 GiB: Hollow Knight, HSH pixel edition, Terraria, Silksong
and Vampire Survivors. Silksong's language selection persists across processes.

GLES is the default. MiniLoong also has bounded Silksong and Vampire Survivors
Vulkan checks. TrimUI's KMSDRM firmware uses GLES: its GPU supports Vulkan,
but Rudroid's current Vulkan presentation requires Wayland. Unsupported backends
and controls cannot be selected in the launcher.

The 1 GiB device still shows substantial swapping and loading stalls in Silksong
and Vampire Survivors. These checks establish short playable paths, not a frame-rate
improvement or long-term stability claim. See [acceptance](crates/docs/ACCEPTANCE.md)
for exact kits, firmware and remaining limits. ARM32, Unity Mono and R36S remain
unverified.

## Documentation

Start with the [documentation index](docs/README.md).

- [Build, test and deploy](BUILD-DOCKER.md): fixed Docker environment and production-kit checks.
- [Development rules](DEVELOPMENT.md): module boundaries, memory safety and lifecycle review.
- [Port a handheld or game](crates/docs/HANDHELD-PORTING-WORKFLOW.md): inspect, reuse, back up and validate each stage.
- [Runtime configuration](crates/docs/RUNTIME-POLICY.md): memory modes, presentation, threads and optional reclamation.
- [Capabilities and limits](crates/docs/MIGRATION-MAP.md): implemented consumers and remaining differences.
- [Performance decisions](crates/docs/PERFORMANCE-AUDIT.md): measured tradeoffs and conservative defaults.
- [Offline resource preparation](tools/README.md): one-command resource preparation for ASTC-capable 1/2/4 GiB targets.

Run from this repository root using the existing fixed Docker environment:

```sh
bash crates/scripts/docker-rust.sh check --release -- --all-targets
bash crates/scripts/docker-rust.sh test --release --sdl
bash crates/scripts/docker-rust.sh build --release --sdl
```

## Acknowledgements

Thanks to binarycounter and the [Bogodroid](https://github.com/binarycounter/Bogodroid)
contributors for laying the foundation for Rudroid.

## License

The first-party Rust runtime, native glue, tests and offline tools use the
[Rudroid Noncommercial Source-Share License 1.0](LICENSE). Commercial use is
prohibited, including paid products/services and internal business use.
This is source-available software, not OSI-approved open-source software.

Third-party components retain their original licenses and notices; this license
does not restrict their independent use. Previously granted GPL permissions for
older copies are not revoked. See [NOTICE](NOTICE), [authors](crates/AUTHORS),
[dependency notices](THIRD-PARTY-NOTICES) and [CONTRIBUTING](CONTRIBUTING).
