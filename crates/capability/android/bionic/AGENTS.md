# Bionic native ABI source

> `android_bionic` answers version-checked Android libc/libm/liblog imports with host glibc services and manages guest destructors.

## Role
Capability `android_bionic`, `Kind::Runtime`, `requires = android_elf_load`, artifact `libandroid_bionic.so` (cdylib + rlib); optional deploy member. The Unity loader registers `rudroid_bionic_symbol` with `rudroid_elf_symbol_source_register_finalized` (finalizer `rudroid_bionic_finalize_range`) before guest mapping. Implementations compile only for Linux ARM64 GNU; elsewhere every query returns 0.

## Design

- `registry.rs` owns provider order and exact library/version gates; weak or
  versioned requests never become arbitrary host dlsym passthrough.
- C heap/errno, checked FORTIFY and layout-asserted FILE/synchronization/thread
  facades reuse host services. Guest FILE pointers never escape to host stdio.
  Only native-owned payloads enter the shared finalizer ledger.
- Small Android stacks adapt to the host minimum without extending borrowed
  ranges; valid larger borrowed stacks keep their bounds. Blocking synchronization
  waits hold no Rust lock; process-shared heap-handle objects reject.
- open/stat/stdio/directory paths share resource/font/cache resolution. Native
  C-string copies are freed after I/O with errno preserved; 64-bit aliases reuse
  the same layout-gated adapters. Locale/termios/ioctl retain Android ABI conversion.
- Image finalization drains destructors, atfork entries and synchronization
  payloads before unmapping. It requires quiescent guest threads.

## Constraints
- Each export lives in exactly one provider table, and names outside base `LIBC` also go into `registry::VERSIONED`; never pass symbols through to `dlsym`.
- Never hand a host `FILE*` to the guest; native `free`/`realloc` accept only native-heap pointers, and Rust objects never live on the C heap.
- C layouts are static-asserted against native headers; C/assembly builds only through `native-build`; unmapping requires quiescent guest threads.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_bionic`; `common` and `libc` |
| Build script | `build.rs` | Private archive `rudroid_bionic_c` from C/assembly; links `m`, `dl` |
| ABI source | `src/lib.rs` | Descriptor, library/version gate, liblog install at init |
| Symbol registry | `src/registry.rs`, `src/symbol_table.h` | Provider order, `VERSIONED` nodes, C table shape |
| Basic C services | `src/basic.rs` | Memory/string/number/process addresses, errno, `gettid`, `strlcpy` |
| Fatal diagnostics | `src/diagnostics.rs` | Abort message, `abort`, `__assert2`, `__stack_chk_fail` |
| Sync and threads | `src/sync.rs`, `src/threads.rs` | Mutex/cond/rwlock/sem/barrier/spin facades; `pthread_attr_t` conversion, keys, once |
| Destructors | `src/destructors.rs`, `src/atfork.c` | `__cxa_*` registries, `pthread_atfork` ledger, range finalizer |
| stdio | `src/stdio.rs`, `src/stdio_variadic.c` | `FILE` facade, `__sF`, printf/scanf families |
| Asset paths | `src/paths.rs` | Host API resource queries for extracted assets/fonts/cache/analytics; safe pre-init URI fallback and traversal rejection |
| Locale and UTF-8 | `src/locale.c`, `src/multibyte.rs` | Locale tokens, bounded byte/wide classification, `mbstate_t` conversions |
| Jump buffers | `src/jump.S`, `src/jump.c` | `setjmp`/`longjmp` with a process cookie |
| Math | `src/math.c` | Native libm addresses with ABI gates |
| Network and vectored I/O | `src/network.c` | Socket/iovec layout gates, native `writev` and `addrinfo` translation |
| Signals and ioctl | `src/signals.c`, `src/ioctl.S` | Mask and `sigaction` conversion; 32-bit ioctl opcode widening |
| liblog | `src/logging.c` | liblog facade forwarding to the Host API log |
| POSIX | `src/posix.c`, `src/posix.rs` | `stat`/`dirent`/`tm` gates, `sysconf`, `strerror_r`, native descriptor `fsync`, owned `openlog` tag |
| Unit tests | `tests/unit/` | Version gate, registry uniqueness, sync races, threads, destructors |
| Integration tests | `tests/diagnostics.rs`, `tests/stdio_cost.rs`, `tests/legacy_libc.rs` | Fatal paths, allocation/resources, legacy locale/FORTIFY bounds and private-PTY ABI |
