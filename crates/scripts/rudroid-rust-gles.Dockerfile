# INPUT:  pinned official Rust/Python Bullseye ARM64 images and verified build packages
# OUTPUT: rudroid-rust-gles:20260917, the fixed Rust/SDL/GLES/Vulkan build environment
# POS:    public bootstrap recipe; docker-rust.sh never builds or replaces its image
#
# The old review base added only /review and Cargo artifacts to this official
# image. Neither is a build dependency: sources and caches are mounted by the
# fixed entry. Preserve its compiler and package recipe without the private base.
FROM python:3.11-slim-bullseye@sha256:9e25f400253a5fa3191813d6a67eb801ca1e6f012b3bd2588fa6920b59e3eba6 AS malisx2-python

FROM rust:1.88-bullseye@sha256:b315f988b86912bafa7afd39a6ded0a497bf850ec36578ca9a3bdd6a14d5db4e

# This official stage resolves to CPython 3.11.13 on the same Bullseye base.
# Mesa's generators use Python match syntax. Keep the Rust/GCC/glibc base and
# Debian's /usr/bin/python3 intact; build tools use this private interpreter.
COPY --from=malisx2-python /usr/local/bin/python3.11 /usr/local/bin/python3.11
COPY --from=malisx2-python /usr/local/lib/libpython3.11.so.1.0 /usr/local/lib/libpython3.11.so.1.0
COPY --from=malisx2-python /usr/local/lib/python3.11/ /usr/local/lib/python3.11/
RUN ldconfig && ln -sf python3.11 /usr/local/bin/python3

# Match the accepted SDL image's archived package selection and libudev pair.
# No compiler upgrade or target-driver installation occurs here.
RUN printf 'deb http://archive.debian.org/debian bullseye main\n' \
    > /etc/apt/sources.list \
 && apt-get -o Acquire::Check-Valid-Until=false update \
    && apt-get -o Acquire::Check-Valid-Until=false install -y --no-install-recommends \
        --allow-downgrades \
        libudev1=247.3-7+deb11u5 \
        libsdl2-dev \
        libgl1-mesa-dev \
        libgles-dev \
        libegl1-mesa-dev \
        xvfb \
        xauth \
        ninja-build \
        pkg-config \
        libclang-11-dev \
        libdrm-dev \
        libgbm-dev \
        libudev-dev=247.3-7+deb11u5 \
    && rm -rf /var/lib/apt/lists/*

# Optional malisx2 Linux source build. Rebuilding this fixed image requires
# explicit maintainer approval; docker-rust.sh never installs missing tools.
# Rust/GCC and the target GPU driver stay unchanged. Meson uses a hash-pinned
# pure-Python wheel; bindgen uses this exact registry release and its lockfile.
RUN printf '%s\n' \
      'meson==1.9.2 --hash=sha256:1a284dc1912929098a6462401af58dc49ae3f324e94814a38a8f1020cee07cba' \
      'Mako==1.3.10 --hash=sha256:baef24a52fc4fc514a0887ac600f9f1cff3d82c61d4d700a1fa84d597b88db59' \
      'PyYAML==6.0.2 --hash=sha256:5d225db5a45f21e78dd9358e58a98702a0302f2659a3c6cd320564b75b86f47c' \
      'MarkupSafe==2.1.5 --hash=sha256:6ec585f69cec0aa07d945b20805be741395e28ac1627333b1c5b0105962ffced' \
      > /tmp/malisx2-tools.txt \
    && /usr/local/bin/python3.11 -m pip install --no-cache-dir --no-deps --only-binary=:all: \
         --require-hashes -r /tmp/malisx2-tools.txt \
    && /usr/local/bin/python3.11 -c 'from pathlib import Path; import mesonbuild.backend.ninjabackend as backend; path=Path(backend.__file__); old="str(Path(file.subdir) / f)"; new="str(Path(file.get_subdir() if isinstance(file, build.CustomTargetIndex) else file.subdir) / f)"; text=path.read_text(); assert text.count(old)==1, "Meson structured-source patch no longer matches"; path.write_text(text.replace(old,new))' \
    && cargo install --locked bindgen-cli --version 0.72.1 \
    && rm /tmp/malisx2-tools.txt

ENV LIBCLANG_PATH=/usr/lib/llvm-11/lib

# Meson 1.9.2 still reads .subdir on a structured-source CustomTargetIndex;
# that proxy exposes get_subdir(), while GeneratedList keeps .subdir. The
# uniquely matched compatibility edit above leaves other source types alone.
# Both the recipe and installed backend hashes are recorded in tools.json.
