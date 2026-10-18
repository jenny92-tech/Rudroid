# Guest ELF loader

> `android_elf_load` parses, relocates and maps guest AArch64 shared objects and supplies their imports.

## Role
Capability `android_elf_load`, `Kind::Runtime`, artifact `libandroid_elf_load.so` (cdylib + rlib), deployed in every guest kit. The Unity loader resolves its C API by name, registers the native ABI sources (`android_bionic`, `android_media_fallback`, `android_native_events`, `android_zlib`) and maps each guest library once. Feature `test-fixtures` exposes synthetic ELF builders plus a test build marker, so such a build is refused as a kit member.

## Design

- The buffer backend is non-executable. Linux reserves/maps PT_LOAD privately,
  applies checked relocations, W^X and required cache maintenance, then constructors.
  noexec uses validated anonymous copies without changing source or mount flags.
- RELR/APS2/RELA decode with bounded streaming work and shared target coverage/
  symbol checks. Untouched isolated file-backed code stays demand paged; copies,
  text writes and shared-page layouts retain explicit instruction-cache maintenance.
- noexec loading advises redundant clean source pages once before publication;
  metadata mappings remain owned/readable and may refault. Runtime image/resource
  pages are never reclaimed by a frame callback or periodic scan.
- Source/image indexes are Arc snapshots; external lookups and finalizers run
  outside locks. Exact import/version/export rules remain below; C errors are TLS.
- Java load requests acknowledge only already-initialized guest modules or real
  registered Android providers, balancing temporary refs without replaying init.
  Unknown libraries/OpenSL fail, allowing FMOD to select AudioTrack.
- Synthetic `libvulkan.so` is available only with a real registered Vulkan
  proc-address provider. Ordinary GLES runs do not advertise Vulkan support.
- Only two approved Steamworks host companions may load NOW|LOCAL from the pinned
  game-local directory. Code stays pinned after guest handles close.
  Optional libdl tracing is bounded and disabled by default.

## Constraints
- Exports are only defined, non-local, default/protected dynamic symbols; `STT_TLS` symbols are refused, and a versioned import never falls back to a bare name.
- Every relocation write must lie inside the image and merged PT_LOAD coverage; no RWX range may remain, and each mapping is unmapped exactly once on failure.
- Source callbacks and finalizers stay valid for the process (even after revoke), accept reentrant calls and never retain borrowed strings.
- `ExecutableModule::map` is unsafe: constructors/imports require trusted code and exact ABIs. Destroying an image needs quiescent guest threads; production guest images live for the process, and backing files must stay unmodified.
- Guest-file DT_NEEDED discovery, TLS imports, arbitrary new guest-file dlopen, RTLD_NEXT and guest-local namespaces remain unsupported. Two approved native Steamworks basenames are distinct host companions loaded NOW|LOCAL.
- Init pins `paths.native_companion_dir` (relative to game_files; default current game directory). Reject paths supplied by guest, escaped symlinks and non-ELF64 LE AArch64 files. Constructors execute outside locks; recursive/concurrent in-progress loads fail instead of waiting. Native code remains process-pinned after independent guest handles close, preserving returned function pointers. Synthetic Android service handles resolve only existing runtime sources and never host driver DSOs.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_elf_load`; `test-fixtures` feature |
| Crate root | `src/lib.rs` | Descriptor, buffer backend, export rules, C error channel, load stats |
| ELF parser | `src/elf.rs` | Header, program header, dynamic table, version and `.symtab` parsing |
| Hash lookup | `src/hash.rs` | Bounds-checked SYSV and GNU hash lookup |
| Relocations | `src/relocations.rs` | Shared unsafe writable-image RELR/APS2/RELA boundary, coverage checks, symbol binding and conservative successful-write interval |
| Packed Android relocations | `src/packed.rs` | Safe streaming APS2/SLEB128 groups, checked offsets/addends/flags and work bound |
| Exec backend | `src/exec.rs` | File-backed W^X mapping, source registry, image chain, exec C API |
| Java native requests | `src/java.rs` | Safe shared-VM System.load/loadLibrary callbacks over the existing mapped-image authority |
| libdl | `src/libdl.rs` | Guest-visible libdl functions over the image chain |
| Native companions | `src/native_companions.rs` | Exact two-library allowlist, approved directory, process code ownership and revocable serial handles; isolated real libm-copy device probe |
| Import inventory | `src/imports.rs` | Metadata-only `rudroid_elf_visit_imports` visitor |
| Unit tests | `tests/unit/` | Parser, hash, RELR decode, libdl, reentrant lookup and lifetimes |
| Integration tests | `tests/*.rs` | C error channel, exports, bounds, mapping, relocations, init_array, TLS, versions; `file_backed_residency.rs` checks cold code, partial tails and text/data relocation maintenance; `noexec_residency.rs` measures copied-file residency and retained metadata/code lifetime on a real noexec mount |
| Fixtures | `tests/fixtures/` | Guest C probes compiled once per process by `tests/cc_fixture/` |
