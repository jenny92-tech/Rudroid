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
  the same layout-gated adapters, including open64/openat64 and positioned I/O.
  rand/srand share native state; float/double min/max use the attested libm ABI.
  Locale/termios/ioctl retain Android ABI conversion. The legacy `_ctype_`
  data symbol is an AOSP-shaped 257-byte table pointer. Seven libstdc++.so
  C++ runtime imports use an exact LIBC_O gate and a C-only malloc/free,
  futex guard and fatal pure-virtual boundary; no C++ exception runtime is used.
- Image finalization drains destructors, atfork entries and synchronization
  payloads before unmapping. It requires quiescent guest threads.
- Independent Android 15 ARM64/API 35 export-map snapshots audit symbol/version
  coverage offline. The strict full-ABI gate is opt-in due to known missing APIs;
  ordinary behavior tests do not claim complete Bionic or Android API support.
  Scope and how to measure gaps: [Bionic compatibility](../../../docs/BIONIC-COMPATIBILITY.md).

## Constraints
- Each export lives in exactly one provider table, and names outside base `LIBC` also go into their registry version table (`VERSIONED` for libc, `LIBSTDCXX_VERSIONED` for libstdc++); never pass symbols through to `dlsym`.
- Never hand a host `FILE*` to the guest; native `free`/`realloc` accept only native-heap pointers, and Rust objects never live on the C heap.
- C layouts are static-asserted against native headers; C/assembly builds only through `native-build`; unmapping requires quiescent guest threads.
- Explicit diagnostics may report at most 64 SIGABRT/SIGBUS signal-interface
  operations per process (TID, handler address, flags, native result). This is
  received/returned interface metadata, never fault-PC/cause attribution; the
  facade retains native dispositions/results/errno and reads no guest code or
  stack. Default runs are quiet; signal-context formatting has no heap or lock.
  The raw syscall entry shares that budget for ARM64 rt_sigaction6/7 only,
  reporting scalar operation/TID/result without dereferencing its kernel action.
  It forwards the kernel ABI unchanged; other syscalls retain the native tail call.
- With both diagnostics and RUDROID_DIAGNOSTIC_SYNC=1, a bounded observer records
  in-flight condition-wait calls and successful notification interfaces.
  Native payload lifetimes have anonymous generation ids; hot sampling is
  best-effort and never spans a native call. Completion survives a missed end
  sample. Cold lifetime registration precedes publication and retires before
  free. Diagnostic ARM64 wait entries pass the caller return address as one
  received scalar; snapshots may derive its aligned preceding call PC. There
  are no guest memory/code reads, register-set capture or stack walks; an
  address alone does not establish the waiting condition or its producer.
  Default providers keep their original direct Rust entries. Errno/results
  remain native. The optional private SyncDiagnostics.snapshot JNI service is
  polled by the existing separate-player creator loop, with no observer worker.
  At most128 wait rows and16 status rows report drops/budget exhaustion. A
  notification is not proof of a returned wait; no mutex ownership is inferred.
  Semaphores stay on their original path, including async-signal-safe sem_post.

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
| Private wait observation | `src/sync_entry.S`, `src/sync_observation.rs`, `tests/unit/sync_observation.rs` | Opt-in scalar caller address, anonymous lifetime/notification metadata and explicit sampling limits |
| Destructors | `src/destructors.rs`, `src/atfork.c` | `__cxa_*` registries, `pthread_atfork` ledger, range finalizer |
| C++ runtime ABI | `src/libstdcxx.c` | LIBC_O-gated allocation, LP64 futex guard and pure-virtual fatal entries |
| stdio | `src/stdio.rs`, `src/stdio_variadic.c` | `FILE` facade, `__sF`, printf/scanf families |
| Asset paths | `src/paths.rs` | Host API resource queries for extracted assets/fonts/cache/analytics; safe pre-init URI fallback and traversal rejection |
| Locale and UTF-8 | `src/locale.c`, `src/multibyte.rs` | Locale tokens, bounded byte/wide classification, `mbstate_t` conversions |
| Jump buffers | `src/jump.S`, `src/jump.c` | `setjmp`/`longjmp` with a process cookie |
| Math | `src/math.c` | Native libm addresses with ABI gates |
| Network and vectored I/O | `src/network.c` | Socket/iovec layout gates, native `writev` and `addrinfo` translation |
| Signals, syscall and ioctl | `src/signals.c`, `src/syscall.S`, `src/ioctl.S` | Mask/`sigaction` conversion, unchanged raw syscall forwarding with bounded signal metadata, 32-bit ioctl opcode widening |
| liblog | `src/logging.c` | Priority-preserving liblog bridge: ERROR/FATAL by default, lower priorities only with explicit host diagnostics; assertions always survive filtering |
| POSIX | `src/posix.c`, `src/posix.rs` | `stat`/`dirent`/`tm` gates, `sysconf`, `strerror_r`, native descriptor `fsync`, owned `openlog` tag |
| Unit tests | `tests/unit/` | Version gate, registry uniqueness, sync races, threads, destructors |
| Integration tests | `tests/diagnostics.rs`, `tests/stdio_cost.rs`, `tests/legacy_libc.rs`, `tests/libstdcxx.rs` | Fatal paths, allocation/resources, legacy locale/FORTIFY/ctype bounds, C++ runtime guard/race and private-PTY ABI |
| ABI completeness | `tests/unit/completeness.rs`, `tests/fixtures/android-15/` | Independent AOSP symbol/version reference, complete missing-export report and opt-in strict gate |
| Behavior conformance | `tests/conformance.rs` | Signal masks, pthread errno, numeric IPv4/IPv6 DNS and all Android pathconf selectors |

`src/posix_layout.h` checks native record and flag compatibility when compiling `posix.c`.
