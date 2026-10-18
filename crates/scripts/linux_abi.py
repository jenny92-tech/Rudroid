#!/usr/bin/env python3
# INPUT:  Linux ARM64 kit ELF files and an explicit maximum glibc version
# OUTPUT:  static interpreter/SONAME/version/import audit, JSON report or nonzero rejection
# POS:    detects host ABI drift before deployment without executing ELF or ldd
"""Audit direct Linux ARM64 GNU dependencies, not transitive drivers or playability.

python3 crates/scripts/linux_abi.py --dir <deployed-kit> --max-glibc 2.28
Section headers must be retained. The conservative ceiling includes weak
imports: symbol binding does not suppress the linker's VERNEED node check.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import struct
import sys

DEFAULT_GLIBC_MAX = "2.28"


class AbiError(ValueError):
    pass


def version_tuple(version: str) -> tuple[int, ...]:
    if not re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", version):
        raise AbiError(f"invalid glibc version: {version!r}")
    parts = tuple(int(part) for part in version.split("."))
    return parts + (0,) * (3 - len(parts))


class Elf:
    def __init__(self, data: bytes):
        self.data = data
        if data[:7] != b"\x7fELF\x02\x01\x01":
            raise AbiError("expected ELF64 little endian version 1")
        header = self.unpack("<HHIQQQIHHHHHH", 16)
        if header[0] not in (2, 3) or header[1] != 183 or header[2] != 1:
            raise AbiError("expected ARM64 executable/shared object")
        if header[7] != 64 or header[10] != 64 or header[11] == 0:
            raise AbiError("retain ordinary ELF64 section headers for the ABI audit")
        shoff, count = header[5], header[11]
        self.slice(shoff, count * 64)
        self.sections = [self.unpack("<IIQQQQIIQQ", shoff + i * 64) for i in range(count)]
        for section in self.sections:
            if section[1] != 8:  # SHT_NOBITS has no file payload
                self.slice(section[4], section[5])
        phoff, phsize, phcount = header[4], header[8], header[9]
        if phcount and phsize != 56:
            raise AbiError("invalid ELF64 program header size")
        self.slice(phoff, phsize * phcount)
        self.interpreter = None
        for i in range(phcount):
            program = self.unpack("<IIQQQQQQ", phoff + i * phsize)
            if program[0] == 3:  # PT_INTERP
                if self.interpreter is not None:
                    raise AbiError("multiple ELF interpreters")
                self.interpreter = self.string(self.slice(program[2], program[5]), 0)
                if not self.interpreter.endswith("/ld-linux-aarch64.so.1"):
                    raise AbiError(f"unsupported GNU interpreter: {self.interpreter}")

    def slice(self, offset: int, size: int) -> bytes:
        if offset < 0 or size < 0 or offset > len(self.data) or size > len(self.data) - offset:
            raise AbiError("truncated/out-of-range ELF table")
        return self.data[offset:offset + size]

    def unpack(self, layout: str, offset: int):
        return struct.unpack(layout, self.slice(offset, struct.calcsize(layout)))

    @staticmethod
    def string(table: bytes, offset: int) -> str:
        if offset < 0 or offset >= len(table):
            raise AbiError("invalid ELF string offset")
        end = table.find(b"\0", offset)
        if end < 0:
            raise AbiError("unterminated ELF string")
        try:
            return table[offset:end].decode("utf-8")
        except UnicodeDecodeError as error:
            raise AbiError("invalid ELF string encoding") from error

    def payload(self, section) -> bytes:
        return self.slice(section[4], section[5])

    def linked_strings(self, section) -> bytes:
        index = section[6]
        if index >= len(self.sections) or self.sections[index][1] != 3:
            raise AbiError("ELF table does not link to a string table")
        return self.payload(self.sections[index])


def audit_elf(data: bytes, maximum: str = DEFAULT_GLIBC_MAX) -> dict:
    ceiling = version_tuple(maximum)
    elf = Elf(data)
    needed = []
    versions: dict[int, dict] = {}
    for section in elf.sections:
        if section[1] == 6:  # SHT_DYNAMIC
            if section[9] != 16 or section[5] % 16:
                raise AbiError("invalid dynamic table size")
            strings = elf.linked_strings(section)
            terminated = False
            for offset in range(section[4], section[4] + section[5], 16):
                tag, value = elf.unpack("<qQ", offset)
                if tag == 0:
                    terminated = True
                    break
                if tag == 1:  # DT_NEEDED
                    name = elf.string(strings, value)
                    if not name or "/" in name or "\\" in name:
                        raise AbiError(f"nonportable DT_NEEDED path: {name!r}")
                    needed.append(name)
            if not terminated:
                raise AbiError("unterminated dynamic table")
        elif section[1] == 0x6ffffffe:  # SHT_GNU_verneed
            table, strings = elf.payload(section), elf.linked_strings(section)
            offset = 0
            for node in range(section[7]):  # sh_info is the node count
                if offset + 16 > len(table):
                    raise AbiError("truncated VERNEED node")
                version, count, file, auxiliary, following = struct.unpack_from("<HHIII", table, offset)
                if version != 1 or count == 0 or auxiliary < 16 or count > len(table) // 16:
                    raise AbiError("invalid VERNEED node")
                library = elf.string(strings, file)
                position = offset + auxiliary
                for entry in range(count):
                    if position + 16 > len(table):
                        raise AbiError("truncated VERNEED auxiliary")
                    _, flags, index, name, next_entry = struct.unpack_from("<IHHII", table, position)
                    index &= 0x7fff
                    if index < 2 or index in versions:
                        raise AbiError("invalid/duplicate VERNEED index")
                    versions[index] = {"library": library, "version": elf.string(strings, name), "flags": flags}
                    if entry + 1 < count:
                        if next_entry < 16:
                            raise AbiError("invalid VERNEED auxiliary chain")
                        position += next_entry
                    elif next_entry != 0:
                        raise AbiError("VERNEED auxiliary count disagrees with chain")
                if node + 1 < section[7]:
                    if following < 16:
                        raise AbiError("invalid VERNEED chain")
                    offset += following
                elif following != 0:
                    raise AbiError("VERNEED node count disagrees with chain")
            if table and section[7] == 0:
                raise AbiError("missing VERNEED node count")

    imports = []
    for index, section in enumerate(elf.sections):
        if section[1] != 11:  # SHT_DYNSYM
            continue
        if section[9] != 24 or section[5] % 24:
            raise AbiError("invalid dynamic symbol table size")
        strings = elf.linked_strings(section)
        count = section[5] // 24
        candidates = [s for s in elf.sections if s[1] == 0x6fffffff and s[6] == index]
        if len(candidates) > 1:
            raise AbiError("multiple VERSYM tables for one DYNSYM")
        versym = elf.payload(candidates[0]) if candidates else None
        if versym is not None and len(versym) != count * 2:
            raise AbiError("VERSYM size disagrees with DYNSYM")
        for i in range(1, count):
            name, info, _, defined, _, _ = elf.unpack("<IBBHQQ", section[4] + i * 24)
            if defined != 0 or name == 0:
                continue
            version_index = struct.unpack_from("<H", versym, i * 2)[0] & 0x7fff if versym else 1
            requirement = versions.get(version_index)
            if version_index >= 2 and requirement is None:
                raise AbiError("undefined symbol has an unknown version index")
            imports.append({"name": elf.string(strings, name), "weak": info >> 4 == 2, **(requirement or {})})

    requirements = sorted(versions.values(), key=lambda v: (v["library"], v["version"]))
    glibc = []
    for requirement in requirements:
        name = requirement["version"]
        if not name.startswith("GLIBC_"):
            continue
        # Private/non-numeric GNU ABI nodes have no supported old-system floor.
        if name == "GLIBC_PRIVATE" or not re.fullmatch(r"GLIBC_[0-9]+\.[0-9]+(?:\.[0-9]+)?", name):
            raise AbiError(f"unsupported host ABI requirement: {name}")
        version = name.removeprefix("GLIBC_")
        if version_tuple(version) > ceiling:
            raise AbiError(f"{name} from {requirement['library']} exceeds glibc ceiling {maximum} (weak imports included)")
        glibc.append(version)
    return {
        "architecture": "aarch64", "interpreter": elf.interpreter,
        "needed": sorted(set(needed)),
        "glibc_max_required": max(glibc, key=version_tuple) if glibc else None,
        "version_requirements": requirements,
        "imports": sorted(imports, key=lambda item: item["name"]),
    }


def audit_file(path: Path, maximum: str = DEFAULT_GLIBC_MAX) -> dict:
    try:
        return audit_elf(path.read_bytes(), maximum)
    except (OSError, AbiError) as error:
        raise AbiError(f"{path.name}: {error}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--dir", type=Path, help="deployed kit directory")
    parser.add_argument("--file", type=Path, action="append", default=[],
                        help="also audit an exact target dependency, including .so.N files")
    parser.add_argument("--max-glibc", default=DEFAULT_GLIBC_MAX)
    parser.add_argument("--output", type=Path, help="write the full dependency/import JSON report")
    args = parser.parse_args()
    try:
        version_tuple(args.max_glibc)
        if args.dir is not None and not args.dir.is_dir():
            raise AbiError(f"not a directory: {args.dir}")
        paths = args.file + (sorted(p for p in args.dir.iterdir() if p.is_file() and
                            (p.suffix == ".so" or p.name == "rudroid-unity-loader")) if args.dir else [])
        if not paths:
            raise AbiError("no Linux kit files to audit")
        if len({p.name for p in paths}) != len(paths):
            raise AbiError("duplicate member basename; audit these dependencies separately")
        members = {p.name: audit_file(p, args.max_glibc) for p in paths}
        report = {"format": 1, "glibc_ceiling": args.max_glibc,
                  "scope": "direct ELF requirements; audit target SDL/EGL/drivers separately", "members": members}
        if args.output:
            args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        for name, member in members.items():
            print(f"{name}: glibc={member['glibc_max_required'] or 'none'} needed={','.join(member['needed']) or 'none'}")
        print(f"LINUX_ABI_AUDIT members={len(members)} ceiling={args.max_glibc} result=passed scope=direct")
        return 0
    except (OSError, AbiError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
