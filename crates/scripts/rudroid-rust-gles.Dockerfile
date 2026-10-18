# INPUT:  pinned official Rust 1.88/Bullseye ARM64 image and archived Debian packages
# OUTPUT: rudroid-rust-gles:20260917, the fixed Rust/SDL/GLES/Vulkan build environment
# POS:    public bootstrap recipe; docker-rust.sh never builds or replaces its image
#
# The old review base added only /review and Cargo artifacts to this official
# image. Neither is a build dependency: sources and caches are mounted by the
# fixed entry. Preserve its compiler and package recipe without the private base.
FROM rust:1.88-bullseye@sha256:b315f988b86912bafa7afd39a6ded0a497bf850ec36578ca9a3bdd6a14d5db4e

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
    && rm -rf /var/lib/apt/lists/*
