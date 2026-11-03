# Unity 2020.2.2f1 native teardown order

This is an exact native-image lifetime repair, not a generic Unity offset table
or a title-specific workaround. The original image remains a local input.

| Identity | Value |
|---|---|
| ELF | AArch64 little-endian Android `libunity.so` |
| Player version | 2020.2.2f1 |
| Local source SHA-256 | `19083b858d9dc6ae8dc323a28eec7de29dc6177343170e0d7facc890103406a7` |
| GNU build ID | `ca5aa9eee1c7ee809c61997ce20f318f72361dff` |
| Export RVAs | `JNI_OnLoad=0x695274`, `JNI_OnUnload=0x695448`, `UnitySendMessage=0x41901c` |
| Parent RVA | `0x67e478`, complete 48-byte body guarded |
| Old calls | `0x67e488 → 0x530c40`, `0x67e48c → 0x67664c` |
| New calls | `0x67e488 → 0x67664c`, `0x67e48c → 0x530c40` |
| Callee guards | complete observed bodies: `0x530c40` / 176 bytes and `0x67664c` / 88 bytes |
| Player close guards | complete adjacent `nativeDone` / `nativePause` bodies at `0x69473c` / 216 bytes |

Disassembly of these exact functions shows no incoming argument use and ignored
return values at the two call sites. The first old call destroys engine memory
management. The second releases native module/unwind metadata through shared
ownership destructors. Nearby AOSP libunwindstack source-path strings and the
module-path/ELF-reference/mutex object layout support that cache interpretation;
stripped disassembly labels are not treated as recovered function symbols.

On the MiniLoong RK3566 ARM64 Linux device, hk26's syscall trace observed a
successful allocator `munmap` through RVA `0xe0b390`. The unmapped 256MiB virtual
arena covered the shared-ownership control block later decremented at
`0x3b5b54`, after the cache cleanup at `0x67664c`. This is evidence about virtual
mapping lifetime, not a claim of 256MiB resident memory savings.

Fresh private processes hk30 and hk32, with the two calls swapped only in
process memory, both completed nativeDone, worker cleanup and preference flush
with exit 0. One-shot entry probes observed cache cleanup before allocator
cleanup in both. The matched unmodified hk31 control entered allocator cleanup
first and exited 139. No step was skipped, no forced shutdown was used, and all
tracer patches/attachments and private mounts were cleaned up. The original
games, libraries and saves were untouched. Host evidence is under
`crates/target/device-rollout/20261015-122457/`, archive
`completed-hk30-32-cleanup-order.tar.gz` SHA-256
`f4d0c546c5b81f8dd6645d5264cda0a6a5183eac920ba6c78645320097faf3d4`.

The maintained implementation requires all listed anchors and word guards,
checks mapping coverage, and delegates the two-word write to common's existing
quiescent transaction with cache maintenance and failed-seal rollback. It never
patches an unknown build, invokes native cleanup from Rust, retains a native
object, bypasses nativeDone, or changes the allocator during gameplay. Exact
guard, real two-call order and rollback tests cover the native boundary. These
checks do not prove every native lifetime or a performance improvement.

Production runs hk33 and hk34 completed nativeDone with this allocator/cache
repair; hk34 also matched the original C King's Pass world geometry and complete
HUD after the missing camera post-callback correction was implemented. Those
runs still lacked complete streamed music. Once the Android/native pthread-stack
minimum translation restored music and the FMOD file worker, hk35 and hk36
exposed a separate earlier wait in streamed Sound release. Both are failed,
watchdog-forced exits, not successful native cleanup.

Read-only thread snapshots showed nativeDone waiting at `0x1195dd0`. The actual
nativePause called Android AudioDevice.stop before that destruction started;
its mixer had already joined. Keeping only the final Lifecycle hook late was
therefore insufficient. On this image, `nativeDone` owns complete destruction
through `0x67e3f8` and `0x67e478`, including its own AudioDevice.stop-before-free
callback. Native output stop at `0x11f6d28` clears its active flag under its own
mutex, releases that mutex before Java stop at `0x11f6dbc`, then continues native
cleanup after the producer has joined.

The hk37 control changed only the final nativePause entry in one private process
to return false; actual nativeDone and both repaired cleanup calls were left
intact. It completed owner stop/join, nativeDone, final worker cleanup and
preference flush, exit 0 with no forced signals or survivors. This diagnostic is
not production acceptance. The hk36/hk37 archive is
`completed-hk36-37-pause-causality.tar.gz`, SHA-256
`f422a0f76f485f8a168b694d84b41fe09eb2070137950db041faf8696c6b284b`.

An initial maintained profile selected native-owned terminal destruction after
the image guards and repair succeeded. Later Unity 2021.3.56f2 evidence exposed
the same shared orchestration error. Terminal destruction now belongs to common
without injecting a suspend; the temporary per-version selector is removed.
NativePause/nativeDone code and ordinary suspend/resume are unchanged. See
[the shared terminal ownership evidence](terminal-lifecycle.md).
A regression with a nativePause that really stops the producer first reproduces
the failure, then covers live streamed drain, synchronous owner join before free,
zero callback after free, both hook-error phases/first-exception identity and
a separately invoked ordinary pause in five isolated child cases.
New complete-production-kit gameplay/relaunch/music/exit validation follows
separately in the dated acceptance record.

The same exact image also needs the C-compatible host-presentation facade to
select its working Android audio path. That behavior is not a universal Unity
default: on Unity 2021.3.14f1c1, hsh9's presentation-triggered audio restart
closed the initial 24000Hz output and failed both replacement attempts. An
early private-process control, hsh11, returned no presentation route before
native startup; it retained the 24000Hz output, produced continuous title and
forest PCM and completed nativeDone/exit 0 without forced cleanup. hsh10 patched
the query too late and is not used as causal negative evidence.

Framework now defaults to no virtual presentation route. This verified profile
alone enables the bounded cached facade through the shared VM at module load.
It creates no second window or physical display and does not add SDK/title-name
branches to the framework. Both-profile JNI regressions cover default absence,
explicit enable/disable, queries on a retained disabled route, and repeated
identity reuse. Real production audio for every affected title remains a
separate acceptance step.
