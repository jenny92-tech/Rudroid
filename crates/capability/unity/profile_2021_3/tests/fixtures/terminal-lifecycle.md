# Terminal native player ownership

Terminal close stops player input/managed maintenance first, quiesces independent
Java services, calls nativeDone once, then joins final services and core workers.
It does not inject nativePause. Native-owned audio must remain able to process
streaming releases until its owner calls AudioDevice.stop/close and joins before
free. Ordinary suspend/resume remains a separate lifecycle operation.

This is a shared lifecycle contract, not a title-name branch, allocator patch,
sound-release timeout, force exit or offset-based bypass. Exact native-image
instruction repairs and presentation-route compatibility stay version-owned.

| Device evidence | Result and scope |
|---|---|
| Unity 2020.2.2f1, SHA `19083b858d9dc6ae8dc323a28eec7de29dc6177343170e0d7facc890103406a7` | hk36's nativePause joined the mixer before nativeDone waited in streamed Sound release at `0x1195dd0`. hk37's private-process terminal-pause control kept real nativeDone intact and completed owner stop, all cleanup and exit 0. Production hk38/39/40 subsequently completed that stop order with music, save reload and no forced cleanup. The allocator/cache repair remains a separate exact-image fix. |
| Unity 2021.3.56f2, SHA `6456fbddbe10addc6c7c2253f8ad6ff589a01309945d1e1194d2890a21709574` | terra15 restored music and completed/validated its game save, but injected nativePause stopped the mixer before nativeDone waited in native release (`0x7b0678` on the sampled stack). A read-only 37-thread snapshot detached all threads. The failed run was then explicitly aborted: exit143, forced child signals15/9, no survivors. It is not accepted as a normal exit. |
| Same 2021.3.56f2 image, terra17 private control | Only the final nativePause entry returned false; original files, nativeDone and cleanup bodies remained intact. Menu music was continuous (game PCM peak14494/RMS2337.39). NativeDone itself called owner stop/join, final cleanup/preferences completed, exit0 in57.73s, no forced signals or survivors. This diagnostic is not production acceptance. |

An initial native-owner policy was restricted to the 2020 image. The later 2021
evidence showed that the injected suspend was a shared orchestration error.
The maintained correction removes that extra call and the temporary enum, Spec
selector and per-profile flag rather than adding more private offsets.

The FMOD regression uses the real VM/framework/SDK DSOs and a nativePause that
actually stops its producer. Default close must provide additional mixer
callbacks, synchronously join at owner stop, make no call after native free,
preserve the first Throwable through both hook-error phases, reject new starts
and tolerate repeated final cleanup. A separate explicitly invoked pause also
remains tested. Both profiles' spy fixtures require nativeDone once and no
terminal nativePause, including failure/partial-init coverage.

No native callback, VM reference or guest mapping may outlive its owner. New
engines/SDKs must verify the stop-before-free contract and real device exit;
these results do not prove all native lifetimes or a performance improvement.

The final complete production kit `rust-kit-3f9f77f90607a974` then completed
real nativeDone and owner stop/join with no forced cleanup on all five device
trials: hk41 (112.61s), hsh13 (102.34s), terra18 (98.41s), silk2 (517.23s),
and vs5 (547.05s). Each entered a real scene and produced nonzero game-only
PCM; Terraria reloaded its independently committed save and VS wrote
LifetimeSurvived410→424 before exit. These bounded trials are not full-game,
all-SDK, low-memory or performance acceptance.
