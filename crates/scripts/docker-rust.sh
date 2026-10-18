#!/usr/bin/env bash
# INPUT:  Docker, fixed image/cache, workspace, optional Release ThinLTO/PGO and local game data
# OUTPUT:  build and test artifacts in the fixed target/docker-linux-arm64 directory
# POS:    Sole Rust Docker entry: reuses the image and caches, pins the container name and auto-cleans up
set -euo pipefail

usage() {
    echo 'Usage: bash crates/scripts/docker-rust.sh {build|test|check} [--release] [--thin-lto] [--pgo-generate | --pgo-use <profdata>] [--sdl | --vulkan | --portable-wayland] [-- <cargo args>]' >&2
    exit 2
}
[ "$#" -gt 0 ] || usage
action="$1"; shift
case "$action" in build|test|check) ;; *) usage ;; esac
args=(--locked)
features=''
sdl=0
vulkan=0
portable_wayland=0
release=0
thin_lto=0
pgo_mode=''
pgo_file=''
extra=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --release) args+=(--release); release=1 ;;
        --thin-lto) thin_lto=1 ;;
        --pgo-generate) [ -z "$pgo_mode" ] || usage; pgo_mode=generate ;;
        --pgo-use) [ -z "$pgo_mode" ] && [ "$#" -ge 2 ] || usage; pgo_mode=use; pgo_file="$2"; shift ;;
        --sdl) sdl=1 ;;
        --vulkan) vulkan=1; sdl=1 ;;
        --portable-wayland) portable_wayland=1; vulkan=1; sdl=1 ;;
        --) shift; extra=("$@"); break ;;
        *) usage ;;
    esac
    shift
done
[ "$portable_wayland" = 0 ] || [ "$action" = build ] || usage
if [ "$sdl" = 1 ]; then features='rudroid-linux-sdl-runtime/sdl'; fi
if [ "$vulkan" = 1 ]; then features+=',rudroid-linux-sdl-runtime/vulkan'; fi
# Cargo profile environment reaches nested fixture builds as well. Keep the
# normal Release profile/output directory and fingerprint this codegen input.
codegen_options=()
if [ "$thin_lto" = 1 ]; then
    [ "$release" = 1 ] || { echo '--thin-lto requires --release.' >&2; exit 2; }
    codegen_options=(-e CARGO_PROFILE_RELEASE_LTO=thin)
fi
if [ "$vulkan" = 1 ] && [ -n "$pgo_mode" ]; then
    echo '--vulkan with PGO has not been validated; use the ordinary Vulkan kit.' >&2
    exit 2
fi
# Targeted check/test (-p/--package) narrows the workspace; build stays kit-wide.
scoped=0
for arg in "${extra[@]+"${extra[@]}"}"; do
    case "$arg" in -p|--package|--package=*) scoped=1 ;; esac
done
[ "$action" != build ] || [ "${#extra[@]}" -eq 0 ] || usage
[ "$scoped" = 1 ] || args=(--workspace "${args[@]}")
[ -z "$features" ] || args+=(--features "$features")
root="$(cd "$(dirname "$0")/../.." && pwd)"
game_options=()
if [ -n "${RUDROID_VAMPIRE_GAME_DIR:-}" ]; then
    [ -d "$RUDROID_VAMPIRE_GAME_DIR" ] || { echo 'RUDROID_VAMPIRE_GAME_DIR must be an existing game-data directory.' >&2; exit 2; }
    game_dir="$(cd "$RUDROID_VAMPIRE_GAME_DIR" && pwd -P)"
    # Original game files stay outside the source repository and remain read-only.
    game_options=(--mount "type=bind,src=$game_dir,dst=/game-data/vampire-survivors,readonly" \
        -e RUDROID_VAMPIRE_GAME_DIR=/game-data/vampire-survivors)
fi
image='rudroid-rust-gles:20260917'
# No pull/build fallback: a missing image must never start another toolchain build.
if ! platform="$(docker image inspect "$image" --format '{{.Os}}/{{.Architecture}}')"; then
    echo "Required local image missing: $image. See BUILD-DOCKER.md." >&2
    exit 1
fi
[ "$platform" = linux/arm64 ] || { echo "Wrong image platform: $platform" >&2; exit 1; }
cache="$root/crates/target/docker-linux-arm64"
mkdir -p "$cache/target" "$cache/cargo"
if [ "$vulkan" = 1 ]; then
    python3 "$root/crates/scripts/vulkan-input.py" --cache "$cache/target/native-inputs/vulkan-wsi"
fi
if [ "$portable_wayland" = 1 ]; then
    python3 "$root/crates/scripts/wayland-input.py" --cache "$cache/target/native-inputs/portable-wayland"
fi
pgo_options=()
if [ -n "$pgo_mode" ]; then
    [ "$release" = 1 ] || { echo 'PGO requires --release.' >&2; exit 2; }
    # An explicit target keeps profiling flags out of host build scripts and
    # proc macros. Nested Cargo calls inherit the same target and flags.
    pgo_options=(-e CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu)
    if [ "$pgo_mode" = generate ]; then
        mkdir -p "$cache/target/pgo/raw"
        pgo_options+=(-e RUSTFLAGS=-Cprofile-generate=/work/crates/target/pgo/raw)
    else
        # Freeze bytes first. The content-addressed compiler path is part of
        # RUSTFLAGS and therefore of the existing kit-wide build fingerprint.
        digest="$(python3 "$root/crates/scripts/pgo-input.py" --cache "$cache/target/pgo/inputs" "$pgo_file")"
        pgo_options+=(--mount "type=bind,src=$cache/target/pgo/inputs/$digest.profdata,dst=/pgo/$digest.profdata,readonly"
            -e "RUSTFLAGS=-Cprofile-use=/pgo/$digest.profdata -Cllvm-args=-pgo-warn-missing-function")
    fi
fi
# Mesa's cache must not fall back to /root/.cache on the read-only root.
# Reuse the existing writable target mount; no new volume or container layer.
mkdir -p "$cache/target/mesa-shader-cache"
command=(cargo "$action")
if [ "$action" = build ]; then command=(bash scripts/build-kit.sh); fi
if [ "$action" = build ] && [ "$vulkan" = 1 ]; then command=(bash scripts/vulkan-kit.sh); fi
display=0
if [ "$action" = test ] && [ "$sdl" = 1 ]; then
    # Keep the display server stable while lifecycle tests close their last
    # client and immediately create another SDL session in the same process.
    command=(xvfb-run -a -e /dev/stderr -s '-screen 0 1280x1024x24 -noreset' "${command[@]}")
    display=1
fi
name='rudroid-rust'
# Only a live run counts: a stale exited/created container with the name makes
# `docker run` fail visibly (125) instead of queueing forever.
running() { [ -n "$(docker ps -q --filter "name=^${name}\$")" ]; }
# Fixed name also prevents concurrent runs/worktrees from multiplying containers:
# a second caller queues behind the running one instead of renaming.
# Read-only root ensures compiler outputs cannot accumulate in Docker layers.
while :; do
    if running; then
        echo "docker-rust.sh: waiting for the running $name container to finish..." >&2
        while running; do sleep 5; done
    fi
    status=0
    docker run --init --rm --pull=never --name "$name" --platform linux/arm64 \
        --read-only --tmpfs /tmp:rw,exec,size=1g --cpus=2 --memory=4g \
        --mount "type=bind,src=$root,dst=/work,readonly" \
        --mount "type=bind,src=$cache/target,dst=/work/crates/target" \
        --mount "type=bind,src=$cache/cargo,dst=/cargo-cache" \
        -w /work/crates -e CARGO_TARGET_DIR=/work/crates/target \
        -e CARGO_HOME=/cargo-cache -e CARGO_BUILD_JOBS=2 -e CARGO_INCREMENTAL=0 \
        -e "RUDROID55_DISPLAY=$display" -e LIBGL_ALWAYS_SOFTWARE=1 \
        -e XDG_CACHE_HOME=/work/crates/target/mesa-shader-cache \
        -e MESA_SHADER_CACHE_DIR=/work/crates/target/mesa-shader-cache \
        -e MESA_SHADER_CACHE_MAX_SIZE=32M \
        -e "RUDROID_KIT_FEATURES=${features:+--features $features}" \
        -e "RUDROID_PORTABLE_WAYLAND=$portable_wayland" \
        "${codegen_options[@]+"${codegen_options[@]}"}" \
        "${pgo_options[@]+"${pgo_options[@]}"}" \
        "${game_options[@]+"${game_options[@]}"}" \
        "$image" "${command[@]}" "${args[@]}" "${extra[@]+"${extra[@]}"}" || status=$?
    # 125 with the name taken means another caller won the race; queue again
    # (a brief pause lets a container that was still being created start).
    if [ "$status" = 125 ]; then sleep 3; fi
    if [ "$status" = 125 ] && running; then continue; fi
    if [ "$status" = 125 ] && [ -n "$(docker ps -aq --filter "name=^${name}\$")" ]; then
        echo "docker-rust.sh: a stopped container named $name blocks the run; inspect it with 'docker ps -a --filter name=$name' and remove it ('docker rm $name') once confirmed stale." >&2
    fi
    if [ "$status" = 0 ] && [ "$portable_wayland" = 1 ]; then
        rudroid_client_profile=debug
        [ "$release" = 0 ] || rudroid_client_profile=release
        python3 "$root/crates/scripts/wayland-input.py" --runtime "$cache/target/$rudroid_client_profile/portable-wayland" || status=$?
    fi
    exit "$status"
done
