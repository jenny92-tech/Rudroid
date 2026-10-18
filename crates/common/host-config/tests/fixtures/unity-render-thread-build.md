# Render-thread startup compatibility evidence

Observed on 2026-10-16, MiniLoong Pocket One / RK3566 / Mali-G52 / ARM64 Linux,
Vampire Survivors 1.14.111, Unity runtime reporting `6000.0.61f1 (74a0adb02c31)`,
branch `6000.0/staging`, Release, IL2CPP, stripping enabled.

- Input: trusted, extracted Android `lib/arm64-v8a/libunity.so`.
- ELF64, little endian, ET_DYN, AArch64.
- Complete file length: **23,298,160 bytes**.
- GNU build ID: `309e284d38a68f71` (8 bytes).
- SHA256: `ae2af629e1426b5cd8d64f93280c55f12d208b7856942f4d2d6ebe766c92b229`.
- PT_NOTE file offset 624, length 176: an Android note followed by the GNU note.

```text
080000008400000001000000416e64726f696400170000007232376300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000031323437393031380000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000800000003000000474e5500309e284d38a68f71
```

The runtime uses a bounded header/program/note read and compares build ID and
complete file length. The SHA was collected independently from the real device;
this is a trusted-input compatibility gate, not an authenticity guarantee.
No binary, game data or private native offsets are included here.

The first candidate mistakenly used 13,154,416 as the file length. Its real-device
preflight rejected the opt-in before publication; the original player and boot
file were unchanged. That length remains a negative regression case. The current
integration fixture is sparse identity-only data, not executable Unity code, and
must not be confused with native-game acceptance.

Only the three existing rendering keys are projected; jobs, GC, FPS and geometry
remain independent. Unknown builds keep engine defaults unless an explicit
unsupported override is requested, which rejects before guest loading. A nearby
Unity version is insufficient evidence. Performance/cost acceptance is recorded
separately in [the performance audit](../../../../docs/PERFORMANCE-AUDIT.md).
