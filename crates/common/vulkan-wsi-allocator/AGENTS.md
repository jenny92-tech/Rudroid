# Vulkan WSI allocator

## Role
Plain Rust static library implementing upstream Arm wsialloc ABI version 3.
First-party library under the root noncommercial LICENSE, linked into the optional
upstream MIT Wayland WSI layer, never a capability or a
direct dependency of the loader. Uses the target's existing GBM driver.

## Design
- The allocator owns one GBM device and DRM File. A process-pinned library owns
  every native function pointer. Buffer objects borrow that device synchronously.
- GBM chooses the exact requested DRM modifier. Read its stride/offset/plane
  count before exporting one owned DMA-BUF fd; close the BO after the export.
  WSI owns the returned fd, then Vulkan owns it after successful import.
- The small upstream patch allocates the first real buffer during layout
  discovery and reuses it during binding. No temporary layout allocation or
  guessed AFBC arithmetic; no per-frame allocation, mapping or CPU copy.

## Constraints
- Native C callers supply valid unique allocator ownership, readable bounded
  input arrays and a writable result. Delete requires quiescent allocation calls.
- Only single-plane 32-bit RGBA/RGBX formats are implemented. Exact modifier,
  dimensions, stride, offset zero and one plane must all match. Reject protected,
  unknown and no-memory-query flags rather than advertising unsupported behavior.
- Each swapchain allocator records its first successful layout and rejects later
  stride, geometry, format or modifier changes before exporting another fd.
- On any failure, close private fds/BOs. Transfer fd ownership only with success.
  Library/code remain pinned after allocator destruction; GBM buffers use native
  driver lifetimes, not Rust borrowed pointers stored in a device handle.
- Build/test only via the root fixed Docker entry. Real AFBC import/presentation
  requires target-device evidence; pure tests do not establish driver support.

## Contents

| Path | Responsibility |
|------|----------------|
| `src/lib.rs` | Native wsialloc/GBM ABI, immutable function table and RAII ownership |
| `tests/unit/lib.rs` | ABI layout, rejected flags/geometry and partial-failure ownership contracts |
