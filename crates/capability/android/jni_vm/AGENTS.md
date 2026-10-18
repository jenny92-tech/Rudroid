# Shared JNI VM

> `android_jni_vm` is the single process authority for JNI classes, methods, objects, references, fields, arrays and native registration.

## Role
Capability `android_jni_vm`, `Kind::Runtime`, artifact `libandroid_jni_vm.so` (cdylib + rlib), depending on `common` and `reply_catalog/format`. Its `rudroid_plugin_early_init` creates the process session and registers the `JavaVM*` in the Host API shared-VM slot (first write wins; a foreign VM fails the phase). The guest `JNI_OnLoad`, `android_base`, `reply_catalog` and Addons reach it only through that slot and the JNI tables.

## Design

- `abi.rs` publishes complete JNI tables; unsupported slots report a pending
  exception. Native dispatch checks full descriptors and exact ARM64 prototypes;
  the C adapter alone interprets normal/V varargs. Unsupported shapes reject.
- `session.rs` owns process-lifetime classes, ids, objects, threads and references.
  Fields use exact declaration indexes with inherited lookup; diagnostics preserve
  pending exceptions. Assignability includes interfaces and array covariance.
- Every native call owns a guarded local frame and promotes returned references
  before releasing nested locals. No lock spans guest code; attach/detach and
  external native-thread local cleanup remain explicit.
- All eight primitive arrays share typed storage and checked copy leases;
  COMMIT retains the copy/root, final release/ABORT retire both. Critical access
  remains subject to JNI restrictions. Strings own lazy alternate encodings and
  keep immutable backing storage alive through the final borrow/pin.
- Asset streams use bounded shared read-ahead and release File/budget on close
  or last owner. Scanner accepts UTF-8 whole-stream `\A`/`\z`, not arbitrary regexes.
  Integer parsing borrows UTF-16 without temporary strings.

## Constraints
- Reference promotion atomically retains under the source-handle lock; temporary roots protect direct-buffer copies and object/array field publication against concurrent last-global deletion.
- Object/array collection traces an allocation-free intrusive work list under the liveness and both heap locks, expanding each reachable node once. Scratch links never escape these locks; collection timing, roots, leaf pins and rootless-cycle reclamation remain immediate.
- Incoming graph edges are counted under the liveness/heap locks. A node with no outgoing object/array edges cannot keep other owners alive: a zero-root such node is retained by incoming edges or reclaimed directly at zero incoming edges. Rootless nodes with incoming edges and graph children require tracing; sweep releases every dead owner's incoming-edge contribution before removing any payload.
- Zero-root/zero-incoming nodes and their newly unowned descendants are peeled iteratively with the same scratch links, without allocation or recursion. A residual zero-root child with graph children triggers full tracing before returning, so an acyclic owner of a dead cycle cannot leave the cycle behind. Every removed owner's leaf/graph edges are released once.
- The session is the only authority for objects, fields, arrays and natives; never store a Rust owner pointer in a JNI field, guess an FFI shape, or free the session while guests hold handles.
- Every non-reserved slot is non-null, and any entry whose zero default reads as `JNI_OK` must be implemented.
- `varargs.c` holds no Java objects, state or allocation, and Rust never interprets a platform `va_list`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_jni_vm`; `common` and `reply_catalog/format` |
| Build script | `build.rs` | Private archive `rudroid_jni_varargs` from `src/varargs.c` |
| Crate root | `src/lib.rs` | Descriptor, early-init session and shared-VM registration, `Resolver` |
| JNI tables | `src/abi.rs` | Slot indices, VM/env handles, tables, named failure thunks |
| Session | `src/session.rs` | Classes, methods, references, frames, exceptions, threads, monitors, builtins |
| Fields | `src/fields.rs` | Exact declaration index, field IDs, typed values and object reference edges |
| Object arrays | `src/objects.rs` | Checked object arrays retaining referenced entities |
| Primitive arrays | `src/primitive_arrays.rs`, `src/bytes.rs` | All eight typed POD arrays, shared rooted copy/critical leases and two byte-view adapters |
| Strings | `src/strings.rs` | UTF-16 and (modified) UTF-8 conversion, paired borrows |
| Integer parsing | `src/integers.rs` | Safe decimal conversion over borrowed UTF-16 with signed bounds and BMP decimal digit handling |
| Java streams | `src/streams.rs` | `FileInputStream` and `Scanner` natives; stream opens consult the shared Host API resource resolver |
| Stream buffer | `src/stream_buffer.rs` | Asset read-ahead and the shared `BufferBudget` |
| Direct buffers | `src/direct_buffers.rs` | Direct `ByteBuffer` storage; synchronous owned-only PCM zeroing keeps addresses stable and rejects borrowed native memory |
| Vararg bindings | `src/varargs.rs` | Binds variadic and `V` table slots to the C adapter |
| Vararg adapter | `src/varargs.c` | `va_list` to `jvalue` promotion with ARM64 static asserts |
| Scalar calls | `src/scalar_calls.rs` | Safe descriptor classification and typed byte/char/short/double call boundaries |
| Nonvirtual calls | `src/nonvirtual_calls.rs` | Typed nonvirtual A entries over the shared checked dispatch and explicit typed DefineClass failure |
| Native frames | `src/native_frames.rs` | Safe automatic native scope, independent input locals, return promotion, guarded boundary and Drop cleanup |
| Unit tests | `tests/unit/` | Resolver, tables, field-index collision/shadowing/concurrency, session semantics, frames, threads, stream buffer |
| Integration tests | `tests/cached_calls.rs` | Allocation and provider-query cost regressions |
| Fixtures | `tests/fixtures/` | `rudroid52_probe.c` checks JNI semantics, framework URI/orientation and real system CA export (including explicit rejection and independent byte copies) through table calls; step 21 checks real float defaults through both A and promoted variadic calls; vendored Apache-2.0 AOSP `jni.h`; no sibling-repository include; run by `project/unity_loader/tests/jni_on_load_fixture.rs` |
