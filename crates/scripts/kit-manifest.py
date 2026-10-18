#!/usr/bin/env python3
# INPUT:  kit artifacts in a build or deploy dir (lib*.so and loader binary), JSON manifest, kit_markers.py
# OUTPUT:  kit manifest writing (scan) and distribution consistency check (verify), per-file runtime_build_id
# POS:    Ticket 57 distribution manifest tool: records the kit id and verifies the deployed set has no members from a different build
"""Kit manifest: record and verify the kit-wide runtime_build_id (ticket 57).

The kit is same-build only: every deployed member must carry the loader's
`rust-kit-...` id. This script reads the ids embedded in the built binaries
(no dlopen needed), records them in a JSON manifest and verifies a build or
deploy dir against a recorded manifest.

Usage:
    kit-manifest.py scan --build <dir> --output kit-manifest.json
    kit-manifest.py verify --dir <dir> --manifest kit-manifest.json

`scan` refuses a build dir whose members carry more than one distinct id
(that is a mixed kit). Both commands refuse members exporting a test-only
symbol (kit_markers.py): `cargo test` builds carry the kit id unchanged. `verify` checks every id-bearing file in the dir
against the manifest's kit id; files without a kit id are ignored (they are
not kit members), a subset deployment is legitimate.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

from kit_markers import test_build_marker

KIT_ID_RE = re.compile(rb"rust-kit-[0-9a-f]{16}(?:-ci-[0-9A-Za-z._-]{1,64})?")
MANIFEST_FORMAT = 1


def is_kit_member_file(path: Path) -> bool:
    if path.name.endswith(".so"):
        return True
    return path.is_file() and path.name == "rudroid-unity-loader"


def ids_in_file(path: Path) -> list[str]:
    data = path.read_bytes()
    return sorted({match.decode("ascii") for match in KIT_ID_RE.findall(data)})


def scan_dir(build_dir: Path) -> dict[str, str]:
    if not build_dir.is_dir():
        print(f"error: not a directory: {build_dir}", file=sys.stderr)
        raise SystemExit(2)
    members: dict[str, str] = {}
    for path in sorted(build_dir.iterdir()):
        if not is_kit_member_file(path):
            continue
        found = ids_in_file(path)
        if not found:
            continue
        marker = test_build_marker(path)
        if marker is not None:
            print(
                f"error: {path.name} exports non-production symbol {marker}: "
                "test-feature or PGO training output; rebuild through "
                "docker-rust.sh without --pgo-generate",
                file=sys.stderr,
            )
            raise SystemExit(1)
        if len(found) > 1:
            print(
                f"error: {path.name} carries multiple kit ids ({', '.join(found)})",
                file=sys.stderr,
            )
            raise SystemExit(1)
        members[path.name] = found[0]
    if not members:
        print(
            f"error: no kit member ids found in {build_dir} "
            "(build the workspace first)",
            file=sys.stderr,
        )
        raise SystemExit(1)
    return members


def kit_id_of(members: dict[str, str]) -> str:
    distinct = sorted(set(members.values()))
    if len(distinct) != 1:
        print(
            "error: mixed kit — members carry "
            + len(distinct).__str__()
            + " distinct ids:\n  "
            + "\n  ".join(
                f"{name}: {members[name]}" for name in sorted(members)
            ),
            file=sys.stderr,
        )
        raise SystemExit(1)
    return distinct[0]


def cmd_scan(args: argparse.Namespace) -> int:
    members = scan_dir(Path(args.build))
    kit_id = kit_id_of(members)
    manifest = {
        "format": MANIFEST_FORMAT,
        "kit_id": kit_id,
        "members": dict(sorted(members.items())),
    }
    output = Path(args.output)
    output.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"kit_id={kit_id} members={len(members)} manifest={output}")
    return 0


def cmd_verify(args: argparse.Namespace) -> int:
    manifest_path = Path(args.manifest)
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        print(f"error: cannot read manifest {manifest_path}: {err}", file=sys.stderr)
        return 2
    if manifest.get("format") != MANIFEST_FORMAT or "kit_id" not in manifest:
        print(f"error: unsupported manifest format: {manifest_path}", file=sys.stderr)
        return 2
    recorded_kit_id = manifest["kit_id"]
    recorded_members: dict[str, str] = manifest.get("members", {})

    members = scan_dir(Path(args.dir))
    failures = []
    for name, found in sorted(members.items()):
        expected = recorded_members.get(name)
        if expected is None:
            if found == recorded_kit_id:
                print(f"note: {name} not in manifest but matches the kit id (new member)")
            else:
                failures.append(
                    f"{name}: carries {found}, which is neither its recorded id "
                    f"({recorded_members.get(name, 'n/a')}) nor the kit id ({recorded_kit_id})"
                )
        elif expected != found:
            failures.append(f"{name}: recorded {expected} but found {found}")
    if failures:
        print("error: distribution does not match the manifest:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print(
        f"verified kit_id={recorded_kit_id} members={len(members)} "
        f"against {manifest_path}"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    sub = parser.add_subparsers(dest="command", required=True)

    scan = sub.add_parser("scan", help="record member ids into a JSON manifest")
    scan.add_argument("--build", required=True, help="build output dir to scan")
    scan.add_argument("--output", required=True, help="manifest path to write")
    scan.set_defaults(func=cmd_scan)

    verify = sub.add_parser("verify", help="verify a dir against a manifest")
    verify.add_argument("--dir", required=True, help="build or deploy dir to check")
    verify.add_argument("--manifest", required=True, help="manifest path to check against")
    verify.set_defaults(func=cmd_verify)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
