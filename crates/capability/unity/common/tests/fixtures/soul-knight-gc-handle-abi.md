# Unity 2022.3.62f2 ARM64 GC handles

The original Soul Knight 8.7.0 ARM64 IL2CPP library has SHA-256
`f8a4c527ec910c341189f2697e543a285e2b6a7873d475a456c24dba137afcc3`.
The paired Unity build is `2022.3.62f2`. Game binaries remain private inputs.

On 2026-11-15, short native API probes in the original Pixel 4 / Android 13
process checked all three public exports and the actual allocator/consumers:

| Evidence | RVA | Observed instruction or ABI |
|---|---|---|
| `il2cpp_gchandle_new` | `0x5e3e874` | `(object*, bool) -> uint32` |
| `il2cpp_gchandle_get_target` | `0x5e3e8b4` | `(uint32) -> object*` |
| `il2cpp_gchandle_free` | `0x5e3e8fc` | `(uint32) -> void` |
| Allocator handle formation | `0x5e91b28` | `orr w0, w8, w22, lsl #3` |
| Target kind extraction | `0x5e91c4c` | `and w8, w0, #7` |
| Free slot extraction | `0x5e91e0c` | `lsr w24, w0, #3` |

The `w0` return and both 32-bit consumers establish kind/index handles, rather
than Unity 6 slot pointers. Real managed objects and completed Tasks were
rooted, resolved back to the same objects and released using those prototypes.
The local-data/time candidates retained engine roots across agent detachment;
the original process survived the measured observation and reached its login
selection. Temporary root release and failure ownership were also reviewed.
Private probe code and consolidated evidence are under
`.scratch/soul-knight/pixel4/`; no account/token/save values were needed.

The exact version selector reuses the existing typed u32 adapter. Unknown and
adjacent versions remain rejected; a missing managed version getter does not
gain a new native fallback. Full method signatures, owner thread, pinned code
and managed-object lifetime requirements remain the caller's responsibility.
This evidence supports the GC-handle ABI only. It does not establish offline
login, save/reload, handheld first frame or gameplay acceptance, and it does
not enable any title patch or change default deployment configuration.
