#!/usr/bin/env python3
# INPUT:  Linux namespaces, one game-owned native library directory and a command
# OUTPUT: exec with an optional private executable library view and isolated network
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
CLONE_NEWNS = 0x00020000
CLONE_NEWNET = 0x40000000

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
        if path.is_symlink() and path.name != 'libil2cpp.so':
            raise ValueError('native libraries must be regular files')
        resolved = path.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError('native libraries must resolve to regular files')
        with resolved.open('rb') as source:
            header = source.read(20)
        if header[:7] != b'\x7fELF\x02\x01\x01' or int.from_bytes(header[18:20], 'little') != 183:
            raise ValueError('expected ELF64 little-endian AArch64 libraries')
    return library


def unshare_namespace(flag, name):
    before = os.readlink('/proc/self/ns/' + name)
    # Linux unshare(int flags). libc owns no returned pointer;
    # explicit scalar prototype and errno cover older device Python versions.
    libc = ctypes.CDLL(None, use_errno=True)
    unshare = libc.unshare
    unshare.argtypes = [ctypes.c_int]
    unshare.restype = ctypes.c_int
    if unshare(flag):
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))
    if os.readlink('/proc/self/ns/' + name) == before:
        raise RuntimeError(name + ' namespace did not change')
    return before


def unshare_mounts():
    return unshare_namespace(CLONE_NEWNS, 'mnt')


KERNEL_TUNNEL_DEFAULTS = frozenset(
    {'sit0', 'tunl0', 'ip6tnl0', 'gre0', 'gretap0', 'erspan0', 'ip6gre0', 'ip6_vti0', 'ip_vti0'}
)


def _is_down_default_tunnel(interface):
    if interface not in KERNEL_TUNNEL_DEFAULTS:
        return False
    flags_path = Path('/sys/class/net') / interface / 'flags'
    try:
        # IFF_UP == 0x1; kernel built-in tunnel defaults are created DOWN.
        return (int(flags_path.read_text().strip(), 0) & 1) == 0
    except OSError:
        return False


_TRUSTED_TOOL_DIRS = ('/bin', '/usr/bin', '/sbin', '/usr/sbin')


def _system_tool(name):
    found = shutil.which(name)
    if found and str(Path(found).parent) in _TRUSTED_TOOL_DIRS:
        return found
    for directory in _TRUSTED_TOOL_DIRS:
        candidate = f'{directory}/{name}'
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def prepare_network():
    """Explicit offline requests fail closed; never fall back to host networking."""
    if sys.platform != 'linux':
        raise RuntimeError('offline network requires Linux namespaces')
    unshare_namespace(CLONE_NEWNET, 'net')
    if os.readlink('/proc/self/ns/net') == os.readlink('/proc/1/ns/net'):
        raise RuntimeError('offline network shares the system namespace')
    interfaces = {line.split(':', 1)[0].strip()
                  for line in Path('/proc/net/dev').read_text().splitlines()[2:]
                  if ':' in line}
    unexpected = {iface for iface in interfaces if iface != 'lo' and not _is_down_default_tunnel(iface)}
    if 'lo' not in interfaces or unexpected:
        raise RuntimeError('offline network has unexpected interfaces')


def prepare_view(library):
    if sys.platform != 'linux':
        return 'fallback: Linux mount namespaces are required'
    if not str(library).startswith('/'):
        raise ValueError('library directory must be an absolute path')
    flags = os.statvfs(library).f_flag
    if not flags & ST_NOEXEC:
        return 'already executable'
    mount = _system_tool('mount')
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
            subprocess.run([_system_tool('umount') or '/bin/umount', str(library)], check=True)
        return 'fallback: ' + str(error)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game-root', required=True)
    parser.add_argument('--library-dir', required=True)
    parser.add_argument('--offline-network', action='store_true',
                        help='Require a new loopback-only network namespace before exec')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    if not command:
        parser.error('provide the launcher command after --')
    library = library_directory(args.game_root, args.library_dir)
    if args.offline_network:
        prepare_network()
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
