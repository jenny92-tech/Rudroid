# Android 15 ARM64 ABI reference

These are verbatim AOSP export maps from `android-15.0.0_r1`, used as test
data. No upstream implementation is compiled or linked. Function names and
ABI version nodes are reference facts; upstream ownership remains with AOSP.

| Snapshot | Source | SHA256 of decoded original bytes |
|---|---|---|
| `libc.map.txt` | [platform/bionic libc](https://android.googlesource.com/platform/bionic/+/android-15.0.0_r1/libc/libc.map.txt) | `cc1b205339a2e568922a2e2bf0aaa4bcedf6588e20bebb5279b6f335bea6bddb` |
| `libm.map.txt` | [platform/bionic libm](https://android.googlesource.com/platform/bionic/+/android-15.0.0_r1/libm/libm.map.txt) | `253aa26941f7cd37517da06c24c215e55475506d5de98b4373739812eb660949` |
| `liblog.map.txt` | [platform/system/logging liblog](https://android.googlesource.com/platform/system/logging/+/android-15.0.0_r1/liblog/liblog.map.txt) | `7477e071de023751db40660ecfde5ec65ce446fe53f53111eeab418c36550b64` |

The test selects ARM64/API 35 NDK-mode exports: it excludes private/platform
versions, other architectures, future APIs and APEX/LLNDK/system-only tags.
Filtering follows the [Soong symbol-file contract](https://android.googlesource.com/platform/build/soong/+/android-15.0.0_r1/cc/symbolfile/__init__.py).
The snapshots never come from Rudroid's provider tables. Tests run offline.

This reference covers these three library export surfaces only. It is not an
Android SDK/CTS suite, a prototype database or proof of behavioral compatibility.
