#!/usr/bin/env python3
"""Test every combination of public Cargo features, including doctests."""

import argparse
import itertools
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def commands():
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    features = sorted(name for name in manifest.get("features", {}) if name != "default")
    for count in range(len(features) + 1):
        for selection in itertools.combinations(features, count):
            flags = ["--features", ",".join(selection)] if selection else []
            if selection and len(selection) == len(features):
                flags = ["--all-features"]
            label = ",".join(selection) or "default"
            for target in ["--all-targets", "--doc"]:
                yield label, ["cargo", "test", target, *flags]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--list", action="store_true", help="print the matrix without running it")
    args = parser.parse_args()
    for label, command in commands():
        if args.offline:
            command.append("--offline")
        print(f"[{label}] {' '.join(command)}", flush=True)
        if not args.list:
            subprocess.run(command, cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
