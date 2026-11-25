#!/usr/bin/env bash
# INPUT:  cargo/rustup, aarch64-linux-gnu-gcc cross toolchain, optional --no-sdl / --debug arguments
# OUTPUT:  the full set of aarch64 artifacts under target/aarch64-unknown-linux-gnu/{release,debug}
#         (cdylibs and rudroid-unity-loader), with an artifact list and deployment hints
# POS:    ARM64 Linux cross-build entry (ticket 34): passes --target only for this invocation,
#         without affecting host cargo config (ticket 58 added the header comment and device deploy commands)
#
# Usage:
#   scripts/cross-build-linux.sh [--no-sdl] [--debug]
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET=aarch64-unknown-linux-gnu
PROFILE=release
PROFILE_ARGS=(--release)
FEATURES=(--features rudroid-linux-sdl-runtime/sdl)

while [ "$#" -gt 0 ]; do
    case "$1" in
        --no-sdl) FEATURES=() ;;
        --debug) PROFILE=debug; PROFILE_ARGS=() ;;
        --release) PROFILE=release; PROFILE_ARGS=(--release) ;;
        *) echo "Usage: $0 [--no-sdl] [--debug|--release]" >&2; exit 2 ;;
    esac
    shift
done

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

bash scripts/build-kit.sh --workspace --locked --target "$TARGET" \
    "${PROFILE_ARGS[@]+"${PROFILE_ARGS[@]}"}" \
    "${FEATURES[@]+"${FEATURES[@]}"}"

echo
echo "Artifacts (aarch64 Linux, $PROFILE):"
ls "target/$TARGET/$PROFILE" | grep -E '\.(so|d)$|rudroid-unity-loader' || true
echo
if [ "$PROFILE" = release ]; then
    echo "Deploy with: python3 scripts/deploy-select.py --build target/$TARGET/$PROFILE --dest <run-dir> --from-profile deploy/silksong.txt"
    echo "(add --include-loader to also copy the rudroid-unity-loader exe; ticket 58)"
else
    echo "Debug build complete (private investigation only; deploy-select.py requires --release)."
fi
