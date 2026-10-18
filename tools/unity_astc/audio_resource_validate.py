#!/usr/bin/env python3
# INPUT:  ntpath, dataclasses, pathlib and external audio references
# OUTPUT:  AudioRef, validate_ref()
# POS:    校验 AudioClip 外部资源范围与音频头以发现打包错配
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2025-2026 jenny92-tech <jennyliu90223@gmail.com>
"""Internal external-audio range/header checks; called by resource preparation."""

import ntpath
from dataclasses import dataclass
from pathlib import Path


DEFAULT_AUDIO_HEADERS = {
    b"FSB5": "FSB5",
    b"OggS": "Ogg",
    b"RIFF": "RIFF/WAV",
    b"fLaC": "FLAC",
}


@dataclass(frozen=True)
class AudioRef:
    clip_name: str
    source: str
    offset: int
    size: int


def _source_path(resource_dir, source):
    name = ntpath.basename(source)
    if name in ("", ".", ".."):
        raise ValueError(f"invalid resource basename: {source!r}")
    return Path(resource_dir) / name


def validate_ref(resource_dir, ref, headers=DEFAULT_AUDIO_HEADERS):
    problems = []
    source = ref.source or ""
    if not source or source.startswith("archive:"):
        return problems

    path = _source_path(resource_dir, source)
    label = f"{ref.clip_name!r} -> {path.name}:{ref.offset}+{ref.size}"
    if path.is_symlink() or Path(resource_dir).is_symlink():
        return [f"{label}: resource symlink is not supported"]
    if not path.is_file():
        return [f"{label}: missing resource file {path}"]
    if ref.offset < 0 or ref.size <= 0:
        return [f"{label}: invalid offset/size"]

    fsize = path.stat().st_size
    end = ref.offset + ref.size
    if end > fsize:
        return [f"{label}: out of bounds (file size {fsize})"]

    max_header = max(len(h) for h in headers)
    with open(path, "rb") as f:
        f.seek(ref.offset)
        head = f.read(min(ref.size, max_header))

    if not any(head.startswith(h) for h in headers):
        shown = head[:16].hex()
        problems.append(f"{label}: invalid audio header at offset "
                        f"(got {shown or 'empty'})")
    return problems

