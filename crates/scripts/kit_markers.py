#!/usr/bin/env python3
# INPUT:  kit member file bytes (ELF lib*.so / Mach-O lib*.dylib)
# OUTPUT:  TEST_BUILD_MARKERS and test_build_marker(): identify test/training artifacts
# POS:    Test-build marker check shared by deploy-select.py and kit-manifest.py, same table as the loader's TEST_BUILD_MARKERS
"""Non-production markers shared by deploy-select.py and kit-manifest.py.

`cargo test` unifies dev-dependency features (`android_elf_load/test-fixtures`)
into the shared target's cdylibs without changing the kit id, so a deploy
from that dir would otherwise be accepted. Members exporting any of these
symbols are refused. LLVM PGO training images additionally export their
collector; optimized feedback images do not. Keep the list identical to `TEST_BUILD_MARKERS` in
`project/unity_loader/src/lib.rs` (test_deploy_select.py checks it).

Only real exports count: ELF files are checked through their `.dynsym`
defined symbols, so the same name in rodata (the loader binary carries the
list as strings) or as an undefined import never matches. Mach-O symbol
names carry a leading `_`; they are matched as whole `\\0_<name>\\0` strings.
"""

from __future__ import annotations

import struct
from pathlib import Path

TEST_BUILD_MARKERS = (
    "rudroid_test_build_marker",
    "rudroid_elf_image_chain_reset",
    "rudroid_pgo_training_flush",
)

SHT_DYNSYM = 11
MACHO_MAGICS = (
    b"\xcf\xfa\xed\xfe",
    b"\xfe\xed\xfa\xcf",
    b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa\xce",
    b"\xca\xfe\xba\xbe",
)


def _elf_exports(data: bytes) -> set[str]:
    """Names of the defined symbols in every SHT_DYNSYM section; a
    truncated or malformed header yields the names read so far."""
    names: set[str] = set()
    wide = data[4] == 2
    order = "<" if data[5] == 1 else ">"
    try:
        if wide:
            shoff, = struct.unpack_from(order + "Q", data, 0x28)
            shentsize, shnum = struct.unpack_from(order + "HH", data, 0x3A)
            section = order + "IIQQQQIIQQ"
            symbol, sym_size = order + "IBBHQQ", 24
        else:
            shoff, = struct.unpack_from(order + "I", data, 0x20)
            shentsize, shnum = struct.unpack_from(order + "HH", data, 0x2E)
            section = order + "IIIIIIIIII"
            symbol, sym_size = order + "IIIBBH", 16
        sections = [
            struct.unpack_from(section, data, shoff + index * shentsize)
            for index in range(shnum)
        ]
    except struct.error:
        return names
    for header in sections:
        sh_type, sh_offset, sh_size, sh_link = header[1], header[4], header[5], header[6]
        if sh_type != SHT_DYNSYM or sh_link >= len(sections):
            continue
        str_offset, str_size = sections[sh_link][4], sections[sh_link][5]
        strtab = data[str_offset:str_offset + str_size]
        for offset in range(sh_offset, sh_offset + sh_size - sym_size + 1, sym_size):
            try:
                fields = struct.unpack_from(symbol, data, offset)
            except struct.error:
                break
            st_name = fields[0]
            st_shndx = fields[3] if wide else fields[5]
            if st_shndx == 0 or st_name >= len(strtab):
                continue
            end = strtab.find(b"\0", st_name)
            if end < 0:
                continue
            names.add(strtab[st_name:end].decode("utf-8", "replace"))
    return names


def test_build_marker(path: Path) -> str | None:
    """The first non-production symbol `path` exports, if any."""
    data = path.read_bytes()
    if data[:4] == b"\x7fELF" and len(data) > 5:
        exports = _elf_exports(data)
        return next((m for m in TEST_BUILD_MARKERS if m in exports), None)
    if data[:4] in MACHO_MAGICS:
        return next(
            (m for m in TEST_BUILD_MARKERS if b"\0_" + m.encode("ascii") + b"\0" in data),
            None,
        )
    return None
