#!/usr/bin/env python3
# INPUT:  Linux mount namespaces, one game-owned native library directory and a command
# OUTPUT:  exec of that command with an optional private read-only executable library view
# POS:    lets the existing ELF file mapper reclaim clean native code from noexec game media
"""Explicit launcher opt-in; no file copies, frame callbacks or global mount changes."""
import argparse
import ctypes
import os
from pathlib import Path
import shutil
import subprocess
import sys


ST_RDONLY = 1  # Linux statvfs flags, independent of the test host.
ST_NOEXEC = 8

def library_directory(game_root, relative):
    root = Path(game_root).resolve(strict=True)
    if not root.is_dir() or Path(relative).is_absolute():
        raise ValueError('provide a game directory and a relative library directory')
    library = (root / relative).resolve(strict=True)
    if library == root or root not in library.parents or not library.is_dir():
        raise ValueError('library directory must be inside the game')
    files = sorted(library.glob('*.so'))
    if not files:
        raise ValueError('no native shared libraries')
    for path in files:
        if path.is_symlink() or not path.is_file():
            raise ValueError('native libraries must be regular files')
        with path.open('rb') as source:
            header = source.read(20)
        if header[:7] != b'\x7fELF\x02\x01\x01' or int.from_bytes(header[18:20], 'little') != 183:
            raise ValueError('expected ELF64 little-endian AArch64 libraries')
    return library


def unshare_mounts():
    before = os.readlink('/proc/self/ns/mnt')
    # Linux unshare(int flags), CLONE_NEWNS. libc owns no returned pointer;
    # explicit scalar prototype and errno cover older device Python versions.
    libc = ctypes.CDLL(None, use_errno=True)
    unshare = libc.unshare
    unshare.argtypes = [ctypes.c_int]
    unshare.restype = ctypes.c_int
    if unshare(0x00020000):
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))
    if os.readlink('/proc/self/ns/mnt') == before:
        raise RuntimeError('mount namespace did not change')
    return before


def prepare_view(library):
    if sys.platform != 'linux':
        return 'fallback: Linux mount namespaces are required'
    flags = os.statvfs(library).f_flag
    if not flags & ST_NOEXEC:
        return 'already executable'
    mount = shutil.which('mount')
    if mount is None:
        return 'fallback: mount is unavailable'
    bound = False
    try:
        unshare_mounts()
        # Make propagation private BEFORE the first bind: a shared mount must
        # never propagate this game view back into the caller namespace.
        subprocess.run([mount, '--make-rprivate', '/'], check=True)
        subprocess.run([mount, '--bind', str(library), str(library)], check=True)
        bound = True
        subprocess.run([mount, '-o', 'remount,bind,ro,exec,nosuid,nodev', str(library)], check=True)
        updated = os.statvfs(library).f_flag
        if updated & ST_NOEXEC or not updated & ST_RDONLY:
            raise RuntimeError('library view flags were not applied')
        return 'private readonly executable view'
    except (OSError, AttributeError, RuntimeError, subprocess.CalledProcessError) as error:
        if bound:
            # Undo a partial setup in OUR namespace before the legacy fallback.
            # If cleanup fails, do not execute with an unverified view.
            subprocess.run([shutil.which('umount') or 'umount', str(library)], check=True)
        return 'fallback: ' + str(error)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game-root', required=True)
    parser.add_argument('--library-dir', required=True)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    if not command:
        parser.error('provide the launcher command after --')
    library = library_directory(args.game_root, args.library_dir)
    result = prepare_view(library)
    print('[guest-exec-view] ' + result, file=sys.stderr, flush=True)
    # Replace Python instead of keeping a supervisor/interpreter resident.
    # Signals, PID, exit status and the existing launcher cleanup stay intact.
    os.execvp(command[0], command)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print('[guest-exec-view] ' + str(error), file=sys.stderr)
        sys.exit(1)
