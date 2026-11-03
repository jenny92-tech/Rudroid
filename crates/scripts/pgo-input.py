#!/usr/bin/env python3
# INPUT:  one regular LLVM profdata file and the existing target cache
# OUTPUT:  immutable content-addressed profile snapshot; stdout SHA256
# POS:    binds PGO input bytes to the compiler path and kit fingerprint
import argparse
import hashlib
import os
from pathlib import Path
import stat
import tempfile

LIMIT = 64 * 1024 * 1024


def freeze(source, cache):
    fd = os.open(source, os.O_RDONLY | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= LIMIT:
            raise ValueError("PGO input must be a nonempty regular file <=64 MiB")
        data = stream.read(LIMIT + 1)
        after = os.fstat(stream.fileno())
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns) or len(data) != before.st_size:
            raise ValueError("PGO input changed during snapshot")
    digest = hashlib.sha256(data).hexdigest()
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    dest = cache / (digest + ".profdata")
    if dest.exists():
        if dest.stat().st_size != len(data) or hashlib.sha256(dest.read_bytes()).hexdigest() != digest:
            raise ValueError("existing PGO snapshot has different bytes")
        return digest
    fd, name = tempfile.mkstemp(prefix=".pgo-", dir=cache)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fchmod(stream.fileno(), 0o444)
        os.replace(name, dest)
    finally:
        if os.path.exists(name):
            os.unlink(name)
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", required=True)
    parser.add_argument("source")
    args = parser.parse_args()
    try:
        print(freeze(args.source, args.cache))
    except (OSError, ValueError) as error:
        parser.exit(2, "PGO input: " + str(error) + "\n")


if __name__ == "__main__":
    main()
