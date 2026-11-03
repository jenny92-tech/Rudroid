#!/usr/bin/env sh
# INPUT:  cargo/rustup, aarch64-linux-gnu-gcc cross toolchain, optional --no-sdl argument
# OUTPUT:  the full set of aarch64 artifacts under target/aarch64-unknown-linux-gnu/debug
#         (cdylibs and rudroid-unity-loader), with an artifact list and deployment hints
# POS:    ARM64 Linux cross-build entry (ticket 34): passes --target only for this invocation,
#         without affecting host cargo config (ticket 58 added the header comment and device deploy commands)
#
# ARM64 Linux cross-build for the Rust crates workspace (ticket 34).
#
# The Rust kit targets aarch64 Linux handhelds; macOS/x86 dev machines must
# not pretend to be the guest. This script cross-compiles through the kit
# entry (`scripts/build-kit.sh --workspace --locked --target
# aarch64-unknown-linux-gnu`, so the feature choice enters the kit id) and
# requires, on a Linux host that already provides them:
#
#   - rustup target add aarch64-unknown-linux-gnu
#   - an aarch64 Linux GCC toolchain on PATH (aarch64-linux-gnu-gcc)
#
# Never install toolchains inside the project containers (crates/AGENTS.md
# Docker contract); on macOS use `bash crates/scripts/docker-rust.sh build`,
# whose linux/arm64 container builds natively without cross tools.
#
# Host `cargo test` on the dev machine is NOT affected: the target triple is
# only passed to this invocation, never configured globally.
#
# Usage:
#   scripts/cross-build-linux.sh [--no-sdl]
#       --no-sdl  build without the SDL backend (no aarch64 SDL2 needed);
#                 the device build wants SDL2 for platform_sdl_runtime, which
#                 must come from the target sysroot (libsdl2-dev:arm64).
set -e

cd "$(dirname "$0")/.."

TARGET=aarch64-unknown-linux-gnu
FEATURES="--features rudroid-linux-sdl-runtime/sdl"  # same spelling as docker-rust.sh --sdl

case "${1:-}" in
    --no-sdl) FEATURES="" ;;
    "") ;;
    *) echo "Usage: $0 [--no-sdl]" >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || { echo "Usage: $0 [--no-sdl]" >&2; exit 2; }

# Pure-Rust workspace: the GNU linker is the only external requirement.
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

bash scripts/build-kit.sh --workspace --locked --target "$TARGET" $FEATURES

echo
echo "Artifacts (aarch64 Linux):"
ls "target/$TARGET/debug" | grep -E '\.(so|d)$|rudroid-unity-loader' || true
echo
echo "Deploy with: python3 scripts/deploy-select.py --build target/$TARGET/debug --dest <run-dir> --from-profile deploy/silksong.txt"
echo "(add --include-loader to also copy the rudroid-unity-loader exe; ticket 58)"
