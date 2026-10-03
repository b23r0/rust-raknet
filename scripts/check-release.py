#!/usr/bin/env python3
"""Validate a release tag and avoid publishing an immutable crate version twice."""

import argparse
import json
import os
from pathlib import Path
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def already_published(name, version):
    request = Request(
        f"https://crates.io/api/v1/crates/{name}/{version}",
        headers={"User-Agent": "rust-raknet-release-workflow"},
    )
    try:
        with urlopen(request, timeout=30) as response:
            data = json.load(response)
    except HTTPError as error:
        if error.code == 404:
            return False
        raise
    if data["version"]["num"] != version:
        raise ValueError("Registry returned an unexpected crate version")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    version = package["version"]
    if args.tag not in (version, f"v{version}"):
        raise ValueError(f"Release tag {args.tag!r} does not match crate version {version}")
    published = already_published(package["name"], version)
    output = Path(os.environ["GITHUB_OUTPUT"])
    with output.open("a", encoding="utf-8") as stream:
        stream.write(f"already_published={str(published).lower()}\n")
    if published:
        print(f"{package['name']} {version} is already published; skipping duplicate upload.")
    else:
        print(f"{package['name']} {version} is ready to publish.")


if __name__ == "__main__":
    main()
