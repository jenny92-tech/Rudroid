# Guest ELF loader

> `android_elf_load` parses, relocates and maps guest AArch64 shared objects and supplies their imports.

## Role
Capability `android_elf_load`, `Kind::Runtime`, artifact `libandroid_elf_load.so` (cdylib + rlib), deployed in every guest kit. Host-core resolves the ELF C API by name, registers the native ABI sources (`android_bionic`, `android_media_fallback`, `android_native_events`, `android_zlib`) and maps each guest library once. Feature `test-fixtures` exposes synthetic ELF builders plus a test build marker, so such a build is refused as a kit member.

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
- The optional same-kit native alias table accepts exact path spellings and
  issues revocable handles with a pinned capability-owned symbol callback.
  Registration does not discover files or load a new guest image.
- Synthetic `libvulkan.so` is available only with a real registered Vulkan
  proc-address provider. Ordinary GLES runs do not advertise Vulkan support.
- Only two approved Steamworks host companions may load NOW|LOCAL from the pinned
  game-local directory. Code stays pinned after guest handles close.
  Optional libdl tracing is bounded and disabled by default. `RUDROID_TRACE_LIBDL`
  enables trace output. Without `RUDROID_TRACE_LIBDL_SYMBOLS`, live diagnostics
  remain miss-only for the first lookup of each missed name. The comma-separated
  filter selects up to 16 case-sensitive exact names (trimmed, at most 128 bytes
  each) and diagnoses both misses and successful file-hit controls. Only the
  first lookup of each selected name performs work.
  Each name checks at most 8 modules; one process-wide budget allows at most 32
  module reports. `[ELF-LIVE] ... file=... live=...` is evidence only and never
  changes dlsym binding or errors.

- Unversioned runtime lookup preserves successful file exports and hard TLS
  rejection. A file miss may consult bounded owned live dynamic tables only
  after that image's genuine initializers return (release/acquire gate; images
  are published earlier for constructor reentrancy). The strict fallback
  accepts defined global/weak default/protected NOTYPE/OBJECT/FUNC only;
  targets must be inside owned LOADs and functions inside executable LOADs.
  Conflicting tags, version metadata, TLS/IFUNC/absolute/reserved symbols,
  malformed tables and out-of-range targets cannot supply an address. It is
  independent of trace flags/budgets and does not change versioned resolution,
  provider order, mappings or guest bytes. Raw diagnostics remain file-only
  on their `file=` side, even when normal lookup can bind a live export.

## Constraints
- Exports are only defined, non-local, default/protected dynamic symbols; `STT_TLS` symbols are refused, and a versioned import never falls back to a bare name. Guest version matching is per provider DSO: SYSV-chain undefined/non-export entries are skipped before definition-only version validation; no VERSYM accepts any requested version, an absent requested VERDEF node selects GLOBAL index 1, hidden VERSYM bits are masked only after node selection, and VERNEED `vn_file` never scopes interposition. Malformed version tables and versioned TLS definitions fail the name instead of falling through to an older provider.
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
| Exec backend | `src/exec.rs`, `src/exec_exports.rs` | File-backed W^X mapping, source registry, image chain, exec C API |
| Java native requests | `src/java.rs` | Safe shared-VM System.load/loadLibrary callbacks over the existing mapped-image authority |
| libdl | `src/libdl.rs` | Guest-visible libdl over the image chain; publishes process-pinned `rudroid.elf.guest_libdl.v1` |
| Live symbols | `src/live_symbols.rs` | Bounded live dynamic-table evidence and strict unversioned export fallback after initializers |
| Native aliases | `src/native_alias.rs` | Exact path-spelling native alias table and revocable symbol callbacks for same-kit capabilities |
| Native companions | `src/native_companions.rs` | Exact two-library allowlist, approved directory, process code ownership and revocable serial handles; isolated real libm-copy device probe; guest_libdl service init |
| Import inventory | `src/imports.rs` | Metadata-only `rudroid_elf_visit_imports` visitor |
| Unit tests | `tests/unit/` | Parser, hash, RELR decode, libdl, reentrant lookup and lifetimes |
| Integration tests | `tests/*.rs` | C error channel, exports, bounds, mapping, relocations, init_array, TLS, versions; `file_backed_residency.rs` checks cold code, partial tails and text/data relocation maintenance; `noexec_residency.rs` measures copied-file residency and retained metadata/code lifetime on a real noexec mount |
| Synthetic ELF generator | `src/fixture.rs` | Cross-crate test images behind the test-fixtures feature |
| Fixtures | `tests/fixtures/` | Guest C probes compiled once per process by `tests/cc_fixture/` |
