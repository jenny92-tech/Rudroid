#!/usr/bin/env python3
# INPUT:  cargo build output dir (lib*.so/lib*.dylib and optional rudroid-unity-loader),
#         capability id list or --from-profile file, an optional existing deploy dir's
#         .deploy-select-manifest.json, kit_markers.py, linux_abi.py
# OUTPUT:  deploy dir (exact member set of lib<id>.* plus optional loader exe) +
#         atomically written .deploy-select-manifest.json management manifest; exit 0 success / 1 id
#         unresolvable, test-build artifact or deploy failure / 2 bad arguments (incl. case-conflicting ids) or build dir error
# POS:    Deploy Selection copy tool: deploys a chosen capability subset into a run dir with an exact
#         member set; reselection replaces only the files it manages (ticket 58)
"""Deploy Selection: copy a chosen subset of built capabilities into a run dir.

The kit is built once; deployment copies. This script takes a build output
dir (e.g. `crates/target/debug`), a destination run dir and a list of
capability ids (C++-style plugin ids, see crates/docs/MIGRATION-MAP.md) and copies
the matching dynamic libraries. It never compiles anything.

Usage:
    deploy-select.py --build <dir> --dest <dir> <id> [<id> ...] [--include-loader]
    deploy-select.py --build <dir> --dest <dir> --from-profile <file> [--include-loader]

Capability ids resolve to `lib<id>.so` (Linux) or `lib<id>.dylib` (macOS) in
the build dir; the other platform's extension is also accepted when present
(deploying a Linux kit from a macOS machine that only has the Linux guest
artifacts, for example). Every requested id must resolve or the script fails
without copying anything.

Ticket 58 semantics:

- Deployed file names equal the capability id: the loader allowlist
  (`[plugins].enabled`) and Deploy Selection profiles match file stems, so a
  member is always deployed as `lib<id>.<ext>`. The one id whose build
  artifact name differs is `reply_catalog` (crate `rudroid-android-reply-
  catalog` builds `librudroid_android_reply_catalog.so`); it is resolved
  through an explicit alias and deployed as `libreply_catalog.so` so the
  catalog stays independently distributable under its descriptor id
  (tickets 43/45).
- The destination's final member set is exactly the requested selection:
  re-selecting into an existing dir removes the script's previous members
  that the new selection no longer includes. Only files recorded in
  `.deploy-select-manifest.json` (written by this script) are ever removed —
  user data (gamedata, Host Configs, anything else) in the dest dir is never
  touched. A dir deployed by pre-58 versions has no manifest; unknown files
  are then left in place (remove them manually or deploy into a fresh dir).
- New members are staged in a sibling staging dir on the same filesystem and
  moved into place with atomic renames. The manifest is replaced atomically
  (temp file + fsync + rename): first as an intent record covering old and
  new members, finally as exactly what is on disk (stale members that could
  not be removed stay recorded and are retried). A redeploy while a loader
  is running from the same dir is not supported.
- Ids resolve case-exactly and case-variant duplicates are refused, so a
  case-insensitive filesystem (APFS/exFAT) cannot fold two members into one
  file. Manifest entries must be bare file names; a manifest naming paths
  (`../x`) or an unreadable one is refused before anything is touched.
- Artifacts exporting a test-only symbol (kit_markers.py: `cargo test`
  feature-unified builds) are refused; deploy from a `cargo build` output.
- Linux ELF members must pass the static ARM64/glibc audit before any copy;
  default maximum direct glibc requirement is 2.28 (`--max-glibc` can lower it).
  Host SDL/EGL/driver dependencies still need target-side verification.
- The loader executable is NOT part of the capability set (it has no
  capability id). Pass `--include-loader` to also copy `rudroid-unity-
  loader` from the build dir; it is then tracked like a member.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

from kit_markers import test_build_marker
from linux_abi import AbiError, DEFAULT_GLIBC_MAX, audit_file

MANIFEST_NAME = ".deploy-select-manifest.json"
MANIFEST_FORMAT = 1
LOADER_NAME = "rudroid-unity-loader"

# Ids whose build artifact name differs from the deployed `lib<id>.<ext>`
# convention. Keys are capability ids (descriptor ids, see
# crates/docs/MIGRATION-MAP.md); values are the crate lib names in the build dir.
ID_ARTIFACT_ALIASES = {
    "reply_catalog": "rudroid_android_reply_catalog",
}


def artifact_for(build_dir: Path, cap_id: str) -> Path | None:
    artifact_id = ID_ARTIFACT_ALIASES.get(cap_id, cap_id)
    candidates = [
        build_dir / f"lib{artifact_id}.so",
        build_dir / f"lib{artifact_id}.dylib",
        build_dir / f"{artifact_id}.so",
        build_dir / f"{artifact_id}.dylib",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def ids_from_profile(profile: Path) -> list[str]:
    ids: list[str] = []
    for raw in profile.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            ids.append(line)
    return ids


class DeployError(Exception):
    """A deploy that must stop; `code` is the process exit status."""

    def __init__(self, message: str, code: int = 1):
        super().__init__(message)
        self.code = code


def is_plain_name(name: str) -> bool:
    """A bare file name inside the dest dir: no separators, no `.`/`..`."""
    return (
        bool(name)
        and name not in (".", "..")
        and "/" not in name
        and "\\" not in name
        and "\0" not in name
    )


def read_managed(dest: Path) -> list[str] | None:
    """Managed member names, or None when there is no manifest (pre-58 dir).

    A manifest that exists but is unreadable or names anything other than a
    bare file name is refused: re-deploy would otherwise delete files it
    never wrote (`../outside/victim`) or silently forget its members.
    """
    manifest_path = dest / MANIFEST_NAME
    if not manifest_path.exists():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise DeployError(
            f"cannot read deploy manifest {manifest_path}: {err}; "
            "remove it to redeploy (unknown files are then left in place)"
        )
    if not isinstance(manifest, dict):
        manifest = {}
    managed = manifest.get("managed")
    if manifest.get("format") != MANIFEST_FORMAT or not isinstance(managed, list):
        raise DeployError(f"unsupported deploy manifest format: {manifest_path}")
    invalid = [name for name in managed if not isinstance(name, str) or not is_plain_name(name)]
    if invalid:
        raise DeployError(
            f"deploy manifest {manifest_path} names entries outside the deploy dir: "
            + ", ".join(repr(name) for name in invalid)
            + "; refusing to touch them"
        )
    return list(managed)


def write_manifest(dest: Path, managed: list[str], ids: list[str], loader: str | None) -> None:
    """Atomically replace the manifest: temp file + fsync + rename (+ dir
    fsync), so a crash leaves the old or the new manifest, never a torn one."""
    manifest = {"format": MANIFEST_FORMAT, "managed": managed, "ids": ids, "loader": loader}
    fd, temp = tempfile.mkstemp(prefix=MANIFEST_NAME + ".", suffix=".tmp", dir=dest)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, dest / MANIFEST_NAME)
    except BaseException:
        Path(temp).unlink(missing_ok=True)
        raise
    try:
        dir_fd = os.open(dest, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(dir_fd)
    except OSError:
        pass
    finally:
        os.close(dir_fd)


def resolve_artifact(build_dir: Path, cap_id: str, listing: set[str]) -> Path | None:
    """Like artifact_for, but case-exact: on a case-insensitive build dir
    (APFS/exFAT) `Android_base` must not resolve to `libandroid_base.so`."""
    artifact = artifact_for(build_dir, cap_id)
    if artifact is None or artifact.name not in listing:
        return None
    return artifact


def deploy(args: argparse.Namespace, ids: list[str]) -> int:
    build_dir = Path(args.build)
    if not build_dir.is_dir():
        raise DeployError(f"build dir is not a directory: {build_dir}", 2)

    bad_ids = [cap_id for cap_id in ids if not is_plain_name(f"lib{cap_id}.so")]
    if bad_ids:
        raise DeployError(
            "capability ids must be plain names: " + ", ".join(repr(i) for i in bad_ids), 2
        )
    # Case-variant duplicates collapse into one file on case-insensitive
    # destinations (the second rename would find its staging file gone).
    folded: dict[str, str] = {}
    for cap_id in ids:
        other = folded.setdefault(cap_id.casefold(), cap_id)
        if other != cap_id:
            raise DeployError(
                f"capability ids {other!r} and {cap_id!r} differ only in case; "
                "ids are exact file stems — select one", 2
            )

    listing = {entry.name for entry in build_dir.iterdir()}
    resolved: list[tuple[str, Path]] = []
    missing: list[str] = []
    for cap_id in ids:
        artifact = resolve_artifact(build_dir, cap_id, listing)
        if artifact is None:
            missing.append(cap_id)
        else:
            resolved.append((cap_id, artifact))
    if missing:
        raise DeployError(
            "no built artifact for capability id(s): "
            + ", ".join(missing)
            + f" — looked for lib<id>.so / lib<id>.dylib in {build_dir}; "
            "run `cargo build --workspace` first and check crates/docs/MIGRATION-MAP.md for exact ids"
        )

    # Deployed name is always lib<id>.<ext> (stem == id, loader contract);
    # a member whose build artifact carries a different name is renamed.
    members: list[tuple[str, Path]] = []
    for cap_id, artifact in resolved:
        deployed_name = f"lib{cap_id}{artifact.suffix}"
        members.append((deployed_name, artifact))
    if args.include_loader:
        loader = build_dir / LOADER_NAME
        if not loader.is_file():
            raise DeployError(
                f"--include-loader requested but {loader} does not exist; "
                "build the workspace (the loader is the workspace's rudroid-unity-loader bin)"
            )
        members.append((LOADER_NAME, loader))

    test_builds = [
        f"{artifact.name} exports {marker}"
        for _, artifact in members
        if (marker := test_build_marker(artifact)) is not None
    ]
    if test_builds:
        raise DeployError(
            "refusing test-feature build artifacts or PGO training artifacts: "
            + "; ".join(test_builds)
            + " — rebuild through docker-rust.sh without --pgo-generate"
        )

    # Linux ELF deployments get a conservative ABI ceiling before any destination
    # mutation. Mach-O deployments keep their existing independent contract.
    linux_members = 0
    for _, artifact in members:
        with artifact.open("rb") as handle:
            is_elf = handle.read(4) == b"\x7fELF"
        if is_elf:
            try:
                audit_file(artifact, args.max_glibc)
            except AbiError as error:
                raise DeployError(f"Linux ABI audit rejected deployment: {error}") from error
            linux_members += 1
    if linux_members:
        print(f"Linux ABI audit: {linux_members} ARM64 members, glibc ceiling {args.max_glibc} (direct dependencies)")

    dest = Path(args.dest)
    dest.mkdir(parents=True, exist_ok=True)
    previous_managed = read_managed(dest)
    if previous_managed is None:
        previous_managed = []
        legacy = sorted(
            p.name
            for p in dest.iterdir()
            if p.suffix in (".so", ".dylib") and p.name not in {name for name, _ in members}
        )
        if legacy:
            print(
                "note: no deploy manifest in " + str(dest) + "; pre-58 deployment layout — "
                "leaving unknown member files in place: " + ", ".join(legacy)
            )

    new_managed = [name for name, _ in members]
    cap_ids = [cap_id for cap_id, _ in resolved]
    loader_name = LOADER_NAME if args.include_loader else None

    # Stage on the same filesystem as dest so every move is an atomic rename.
    staging = Path(tempfile.mkdtemp(prefix=".deploy-staging-", dir=dest))
    try:
        staged: list[tuple[Path, Path]] = []
        for deployed_name, artifact in members:
            source = staging / deployed_name
            shutil.copy2(artifact, source)
            staged.append((source, dest / deployed_name))
        # Intent record before the first rename: whatever the moves leave on
        # disk is tracked, so a crash or a failed rename is cleaned by the
        # next deploy instead of orphaning new members.
        write_manifest(
            dest, list(dict.fromkeys(previous_managed + new_managed)), cap_ids, loader_name
        )
        placed: list[str] = []
        try:
            for source, target in staged:
                os.replace(source, target)
                placed.append(target.name)
                print(f"deployed {target.name} -> {target}")
        except OSError as err:
            write_manifest(
                dest, list(dict.fromkeys(previous_managed + placed)), cap_ids, loader_name
            )
            raise DeployError(f"cannot move {source.name} into {dest}: {err}")
    finally:
        shutil.rmtree(staging, ignore_errors=True)

    kept: list[str] = []
    for name in previous_managed:
        if name in new_managed:
            continue
        victim = dest / name
        # Case-insensitive dest: a stale `libFoo.so` may BE the new `libfoo.so`.
        if any(_same_file(victim, dest / member) for member in new_managed):
            continue
        try:
            victim.unlink(missing_ok=True)
            print(f"removed stale member {name}")
        except OSError as err:
            print(f"error: cannot remove stale managed member {name}: {err}", file=sys.stderr)
            kept.append(name)

    # The manifest records what is actually on disk: undeletable stale
    # members stay managed so the next deploy retries them.
    write_manifest(dest, new_managed + kept, cap_ids, loader_name)
    return 1 if kept else 0


def _same_file(a: Path, b: Path) -> bool:
    try:
        return os.path.samefile(a, b)
    except OSError:
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--build", required=True, help="build output dir with the built kit")
    parser.add_argument("--dest", required=True, help="run dir to copy selected artifacts into")
    parser.add_argument("--from-profile", type=Path, help="file with one capability id per line")
    parser.add_argument("--max-glibc", default=DEFAULT_GLIBC_MAX,
                        help="maximum direct GNU ELF requirement; default 2.28, use e.g. 2.17 for older firmware")
    parser.add_argument(
        "--include-loader",
        action="store_true",
        help="also deploy the rudroid-unity-loader executable from the build dir",
    )
    parser.add_argument("ids", nargs="*", help="capability ids to deploy (see crates/docs/MIGRATION-MAP.md)")
    args = parser.parse_args()

    ids = list(args.ids)
    if args.from_profile:
        ids += ids_from_profile(args.from_profile)
    # Selection is a set; preserve the first occurrence's deployment order.
    ids = list(dict.fromkeys(ids))
    if not ids and not args.include_loader:
        parser.error("no capability ids given (arguments or --from-profile)")

    try:
        return deploy(args, ids)
    except DeployError as err:
        print(f"error: {err}", file=sys.stderr)
        return err.code


if __name__ == "__main__":
    sys.exit(main())
