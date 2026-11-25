#!/usr/bin/env python3
# INPUT:  Local PortMaster-Unity source and upstream CLI arguments
# OUTPUT: Upstream reports, converted assets, Android player data or port packages
# POS:    Rudroid host-side entry for desktop Unity port preparation
import argparse
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


REVISION = "7f617e282953f8981ca4a38537277a8a9983ff90"
ARCHIVE_SHA256 = "6cad05941b1d1c435021664936b7bff849de09685701247d1aec13e2bbec5c4e"
PORT_COMMANDS = ("check", "new", "package", "routes")
PREPARE_COMMANDS = ("player", "convert", "android-data")


def git(source, *args):
    try:
        return subprocess.check_output(
            ["git", "--no-optional-locks", "-C", str(source), *args],
            stderr=subprocess.DEVNULL, text=True,
        ).strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ValueError(f"Cannot verify PortMaster-Unity Git checkout: {source}") from exc


@contextmanager
def source_tree(source):
    if source.is_dir():
        if (source / ".git").exists():
            revision = git(source, "rev-parse", "HEAD")
            if revision != REVISION:
                raise ValueError(f"Expected PortMaster-Unity at {REVISION}, found {revision}: {source}")
            if git(source, "status", "--porcelain", "--untracked-files=normal"):
                print(f"Using PortMaster-Unity at {REVISION} with local changes (unverified): {source}",
                      file=sys.stderr)
        else:
            print(f"Using extracted PortMaster-Unity source (unverified; expected {REVISION}): {source}",
                  file=sys.stderr)
        yield source
        return

    if not source.is_file():
        raise ValueError(f"PortMaster-Unity source does not exist: {source}")
    with source.open("rb") as archive:
        digest = hashlib.file_digest(archive, "sha256").hexdigest()
        if digest != ARCHIVE_SHA256:
            raise ValueError(f"Expected PortMaster-Unity archive SHA-256 {ARCHIVE_SHA256}, "
                             f"found {digest}: {source}")
        archive.seek(0)
        with tempfile.TemporaryDirectory(prefix="rudroid-pmunity-") as directory:
            # Only the byte-pinned upstream archive reaches extraction. Use the
            # standard data filter where available across supported Python versions.
            with tarfile.open(fileobj=archive, mode="r:gz") as snapshot:
                options = {"filter": "data"} if hasattr(tarfile, "data_filter") else {}
                snapshot.extractall(directory, **options)
            yield Path(directory) / f"portmaster-unity-{REVISION}"


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Run PortMaster-Unity port authoring and host preparation.",
        allow_abbrev=False,
    )
    parser.add_argument("--source", type=Path, default=os.environ.get("PMUNITY_SOURCE"),
                        help="local PortMaster-Unity checkout, fixed tar.gz, or trusted directory "
                             "(default: PMUNITY_SOURCE)")
    parser.add_argument("command", choices=PORT_COMMANDS + PREPARE_COMMANDS + ("prepare",))
    parser.add_argument("args", nargs=argparse.REMAINDER, help="upstream command arguments")
    args = parser.parse_args(argv)
    if sys.version_info < (3, 11):
        parser.error("PortMaster-Unity requires Python 3.11 or newer")
    if args.command == "prepare":
        if __package__:
            from .pmunity_rudroid import prepare
        else:
            from pmunity_rudroid import prepare
        preparation = prepare.parse_args(args.args)
    if not args.source:
        parser.error("set --source or PMUNITY_SOURCE to local PortMaster-Unity source")

    commands = ["port", args.command] if args.command in PORT_COMMANDS else [args.command]
    # Resolve the module independently of the caller's directory. Keep all
    # upstream arguments and cwd intact, and do not create source bytecode.
    launch = (
        "import runpy, sys\n"
        "sys.path.insert(0, sys.argv.pop(1))\n"
    )
    child_env = None
    if args.command == "check":
        # Use the upstream report hook so text and JSON share the same scan.
        launch += f"sys.path.insert(1, {str(Path(__file__).resolve().parent)!r})\n"
        hook = "pmunity_rudroid.check"
        hooks = list(filter(None, os.environ.get("PMUNITY_PORT_HOOKS", "").split(",")))
        child_env = dict(os.environ, PMUNITY_PORT_HOOKS=",".join([h for h in hooks if h != hook] + [hook]))
    launch += "runpy.run_module('pmunity', run_name='__main__', alter_sys=True)\n"
    try:
        upstream = args.source.resolve()
        with source_tree(upstream) as source:
            converter = source / "convert"
            if not (converter / "pmunity" / "__main__.py").is_file():
                parser.error(f"pmunity is missing in the source tree: {converter}")

            def run(arguments):
                result = subprocess.run([
                    sys.executable, "-I", "-B", "-c", launch, str(converter),
                    *arguments,
                ], env=child_env)
                return result.returncode if result.returncode >= 0 else 128 - result.returncode

            if args.command == "prepare":
                return prepare.prepare(preparation, upstream, run)
            return run([*commands, *args.args])
    except (ValueError, OSError, tarfile.TarError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    sys.exit(main())
