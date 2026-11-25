#!/usr/bin/env bash
# INPUT:  fixed Linux ARM64 image, verified source cache and existing CARGO_TARGET_DIR
# OUTPUT: separately audited Release-only vulkan-malisx2 bundle
# POS:    fixed Docker entry's optional third-party ICD post-build, never a system driver install
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
if [ "$#" = 1 ] && [ "$1" = --check-tools ]; then
    python3 scripts/malisx2-input.py --preflight -
    exit
fi
[ "$#" = 0 ] || { echo 'malisx2-kit.sh accepts only --check-tools; use docker-rust.sh --malisx2' >&2; exit 2; }
: "${CARGO_TARGET_DIR:?Run through the fixed docker-rust.sh entry}"
rudroid_mali_inputs="$CARGO_TARGET_DIR/native-inputs/malisx2"
rudroid_mali_objects="$CARGO_TARGET_DIR/native-objects/malisx2"
rudroid_mali_runtime="$CARGO_TARGET_DIR/release/vulkan-malisx2"
mkdir -p "$rudroid_mali_objects"
python3 scripts/malisx2-input.py --preflight "$rudroid_mali_objects/tools.json"
python3 scripts/malisx2-input.py --verify "$rudroid_mali_inputs"
export CARGO_NET_OFFLINE=true
# Prevent parent checkout discovery from stamping Rudroid's SHA into this
# archived upstream tree; the exact source identities live in sources.json.
export GIT_CEILING_DIRECTORIES="$rudroid_mali_inputs"
export LIBCLANG_PATH=/usr/lib/llvm-11/lib
export MESON_PACKAGE_CACHE_DIR="$rudroid_mali_inputs"
if [ -f "$rudroid_mali_objects/build.ninja" ]; then
    python3 -m mesonbuild.mesonmain setup --reconfigure "$rudroid_mali_objects" "$rudroid_mali_inputs/source" \
        --native-file "$rudroid_mali_objects/python.ini" \
        --buildtype=release --strip -Db_ndebug=true -Dmeasurement=false --wrap-mode=nodownload
else
    python3 -m mesonbuild.mesonmain setup "$rudroid_mali_objects" "$rudroid_mali_inputs/source" \
        --native-file "$rudroid_mali_objects/python.ini" \
        --buildtype=release --strip -Db_ndebug=true -Dmeasurement=false --wrap-mode=nodownload
fi
python3 scripts/malisx2-input.py --check-python-build "$rudroid_mali_objects"
ninja -C "$rudroid_mali_objects" -j2
python3 scripts/malisx2-input.py --check-output "$rudroid_mali_runtime"
mkdir -p "$rudroid_mali_runtime"
strip --strip-unneeded -o "$rudroid_mali_runtime/libvulkan_malisx2.so" \
    "$rudroid_mali_objects/src/libvulkan_malisx2.so"
python3 scripts/malisx2-input.py --runtime "$rudroid_mali_runtime" \
    --inputs "$rudroid_mali_inputs" --build "$rudroid_mali_objects" --max-glibc 2.33
