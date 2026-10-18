# NDK extracted assets

> Android asset files and directory iteration over the loader-owned resource service.

## Role
Capability `android_assets`, runtime requiring `android_elf_load`. The loader
registers `rudroid_assets_symbol` before native guest mapping.

## Design
Native queries use the shared resource authority. A process-pinned borrowed
manager issues private, never-reused asset/directory identities. Each asset
owns one File and offset; read uses positional I/O without a whole-pack copy.
getBuffer lazily creates one read-only private file map. Directory names stay
stable in the handle owner; returned fds are independent owned duplicates.

## Constraints
`ffi.rs` is the only unsafe module. Native callers supply valid buffers/C
strings, end borrowed map/name use before close and keep mapped backing files
unmodified. File/map/handle ownership releases on close; invalid/stale handles
fail without pointer dereference. No decoder, decompressor, downloader or
managed-owner pointer is implemented. Production capabilities stay pinned.

## Contents

| Name | Path | Responsibility |
|---|---|---|
| Manifest | `Cargo.toml` | cdylib/rlib, common and native libc |
| Plugin | `src/lib.rs` | Descriptor and validated service join |
| Native boundary | `src/ffi.rs` | Typed asset APIs, opaque handles, File/maps and directory borrows |
