#!/usr/bin/env bash
# INPUT:  fixed Docker environment and the ordinary kit build arguments
# OUTPUT:  complete Rust kit followed by its optional native Wayland WSI dependency
# POS:    keeps the external layer after Cargo produces the Rust allocator archive
set -euo pipefail
bash scripts/build-kit.sh "$@"
rudroid_wsi_profile=debug
rudroid_wsi_cargo_profile=()
rudroid_wsi_clients=()
[ "${RUDROID_PORTABLE_WAYLAND:-0}" != 1 ] || rudroid_wsi_clients=(--portable-wayland)
for rudroid_wsi_arg in "$@"; do
    if [ "$rudroid_wsi_arg" = --release ]; then
        rudroid_wsi_profile=release
        rudroid_wsi_cargo_profile=(--release)
    fi
done
cargo run --locked -p rudroid-native-build --bin rudroid-vulkan-wsi-builder \
    "${rudroid_wsi_cargo_profile[@]+"${rudroid_wsi_cargo_profile[@]}"}" \
    -- "$CARGO_TARGET_DIR" "$rudroid_wsi_profile" "${rudroid_wsi_clients[@]+"${rudroid_wsi_clients[@]}"}"
