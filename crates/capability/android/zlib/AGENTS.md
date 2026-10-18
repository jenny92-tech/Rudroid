# Guest zlib ABI source

> `android_zlib` supplies the host's native zlib compression and checksum functions to guests through the ELF symbol-source chain.

## Role
Capability `android_zlib`, `Kind::Runtime`, `requires = android_elf_load`, artifact `libandroid_zlib.so` (cdylib + rlib). Optional and needs the firmware's native `libz.so.1`; the Unity loader registers `rudroid_zlib_symbol` with `android_elf_load` before guest mapping and revokes it if bring-up fails. Guest end-to-end coverage lives in `project/unity_loader/tests/zlib_guest.rs`.

## Design
- The public source accepts only bare queries and forwards them to the private
  `rudroid_native_zlib_symbol`; keep these linker names distinct. Library/version
  queries and unknown names return 0.
- `zlib.c` returns 28 native zlib addresses (inflate/deflate, `compress`/`uncompress`, checksums, `zlibVersion`, `zError`, `get_crc_table`) with no marshalling.
- Compile-time gates check the LP64 `z_stream` field offsets, 112-byte size, scalar widths and allocator prototypes against the native header.

## Constraints
- Linux ARM64 GNU only; no generic native `dlsym`, foreign-version fallback, second stream, state registry, cache or allocator.
- The guest owns streams, buffers and allocator callbacks and must `End` every stream before guest unmap; native zlib owns internal state until then.
- Audit the real libz symbol versions against the target firmware; never replace firmware libraries.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_zlib`; `common` only |
| Build script | `build.rs` | Private archive `rudroid_zlib` from `src/zlib.c`; links native `z` |
| Source | `src/lib.rs` | Descriptor and bare-name-only source |
| zlib ABI | `src/zlib.c` | `z_stream` layout gates and the 28-entry address table |
| Unit tests | `tests/unit/lib.rs` | Explicit names only; no foreign-version fallback |
