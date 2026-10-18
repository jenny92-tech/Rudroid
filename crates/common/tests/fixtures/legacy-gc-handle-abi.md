# Stripped Unity metadata and the legacy GC-handle ABI

The local original ARM64 IL2CPP library used by 黑神话像素版 has SHA-256
`27bb8e235b1b34e940112941fbc9727e660e877e56360e6bb8e273824dc17360`
and GNU build ID `33a6d7eea7cd2b04ccf9f9f229cbb4e5aa073029`.
Its paired Unity image reports `2021.3.14f1c1`; metadata version29 contains
no `get_unityVersion` string. The optional managed getter cannot establish
this library's ABI. Game binaries are private inputs, not repository files.

Native `llvm-readelf`/`llvm-objdump` inspection on2026-10-16 established:

- `il2cpp_gchandle_new` at0x8443c4 masks the bool inw1 and branches to0x7edcd0,
  which enters the allocator at0x7edcfc. Its complete return at0x7ee108–0x7ee12c
  forms `kind | (index << 3)` inw0 and restores the saved registers before ret.
- `il2cpp_gchandle_get_target` at0x844404 branches to0x7ee20c. It masks the kind
  fromw0 and shiftsw0 right3 for the slot, then returns the object pointer.
- `il2cpp_gchandle_free` at0x84444c branches to0x7ee3c0 and decodes the same
  32-bit kind/index. These are uint32 handles, not Unity6 slot pointers.

The existing typed u32 adapter is reused; no function pointer is called until
the native identity or existing exact version gate succeeds. Selection checks
the GNU note at0x200, all three resolved-export offsets, their branch words,
allocator return and both consumer prefixes. It does not recognize a package
name, infer adjacent versions, guess object offsets or fabricate a version string.
The binding's trusted, pinned, stable-image contract still applies; these guards
do not authenticate arbitrary modified third-party libraries.

Only a missing managed version getter may take this native path. Invalid/byref,
foreign-module or failing getters and unknown returned version strings reject.
The cold signature reads use bounded safe `FileExt::read_exact_at` on
`/proc/self/mem`; file/read failure or any mismatch rejects without touching
guest code, changing page permissions, retaining objects or adding frame work.
Callbacks retain their existing local roots, owner-thread/stop checks and complete
method/field signature validation. Unity memory-pressure actions keep their
separate evidence gate; this change does not enable forced legacy unloading/GC.

Unit coverage mutates every guarded word and each export anchor, checks absent
and overflowing memory, preserves getter errors and exercises both existing
typed root adapters' exact target/free ownership. Real-title HUD/configured-hook
installation and normal teardown must also be recorded after a complete
production-kit rebuild; native inspection alone is not gameplay acceptance.

Raw private evidence is in `target/device-rollout/20261015-122457/` under
`heishenhua-analysis/`: source identities, metadata absence check, exported
symbols, full allocator disassembly and `gc-handle-identity-guards.json`.
