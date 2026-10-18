#!/usr/bin/env bash
# INPUT:  cargo, cargo build command-line args (--features/--no-default-features/--all-features forwarded verbatim)
# OUTPUT:  a single cargo build invocation, exporting the feature fingerprint RUDROID_KIT_FEATURES
# POS:    Unified kit build entry: the feature selection counts toward the kit-wide build id (ticket 57)
#
# common/build.rs folds RUDROID_KIT_FEATURES into runtime_build_id. When bypassing this entry
# and calling cargo directly, any invocation that changes the feature selection must export the same
# RUDROID_KIT_FEATURES itself, otherwise the two kits will share one id (contract in crates/AGENTS.md).

set -euo pipefail

workspace="$(cd "$(dirname "$0")/.." && pwd)"
cd "$workspace"

fingerprint=()
pass_through=()
while [ $# -gt 0 ]; do
    case "$1" in
        --features)
            [ $# -ge 2 ] || { echo "build-kit.sh: --features needs a value" >&2; exit 2; }
            fingerprint+=("$1" "$2")
            pass_through+=("$1" "$2")
            shift 2
            ;;
        --features=*)
            fingerprint+=("$1")
            pass_through+=("$1")
            shift
            ;;
        --no-default-features|--all-features)
            fingerprint+=("$1")
            pass_through+=("$1")
            shift
            ;;
        *)
            pass_through+=("$1")
            shift
            ;;
    esac
done

if [ "${#fingerprint[@]}" -gt 0 ]; then
    exported="${RUDROID_KIT_FEATURES:-${fingerprint[*]}}"
    if [ "${RUDROID_KIT_FEATURES:-}" != "" ] && [ "${RUDROID_KIT_FEATURES}" != "${fingerprint[*]}" ]; then
        exported="${RUDROID_KIT_FEATURES} ${fingerprint[*]}"
    fi
    export RUDROID_KIT_FEATURES="$exported"
fi

exec cargo build "${pass_through[@]}"
