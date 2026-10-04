#!/usr/bin/env python3
"""Keep the crate version and dependency snippets in sync (Python 3.11+)."""

import argparse
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SNIPPETS = ("README.md", "README.zh-CN.md", "src/lib.rs")
DEPENDENCY = re.compile(r'(?m)^(?P<prefix>(?://! )?rust-raknet = (?:\{\s*version = )?")(?P<version>[^"\n]+)(?P<suffix>"[^\n]*)$')
PACKAGE = re.compile(r'(?m)^(?P<prefix>version = ")(?P<version>[^"\n]+)(?P<suffix>")$')
RELEASE = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)')


def version_match(text, pattern, filename):
    matches = list(pattern.finditer(text))
    if len(matches) != 1:
        raise ValueError(f"{filename}: expected one version entry, found {len(matches)}")
    return matches[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="new MAJOR.MINOR.PATCH release version")
    parser.add_argument("--check", action="store_true", help="check documented versions without editing")
    args = parser.parse_args()
    if args.check == bool(args.version):
        parser.error("pass either VERSION or --check")
    if args.version and not RELEASE.fullmatch(args.version):
        parser.error("version must be MAJOR.MINOR.PATCH, for example 0.16.0")

    manifest = ROOT / "Cargo.toml"
    manifest_text = manifest.read_text(encoding="utf-8")
    current = tomllib.loads(manifest_text)["package"]["version"]
    documents = [(ROOT / name, (ROOT / name).read_text(encoding="utf-8")) for name in SNIPPETS]
    entries = [(path, text, match) for path, text in documents for match in DEPENDENCY.finditer(text)]
    for path, _ in documents:
        if not any(entry[0] == path for entry in entries):
            raise ValueError(f"{path.name}: no dependency version entry")
    if args.check:
        mismatches = [f"{path.relative_to(ROOT)}: {match['version']} (crate: {current})"
                      for path, _, match in entries if match["version"] != current]
        if mismatches:
            raise ValueError("Documented versions are out of sync:\n" + "\n".join(mismatches))
        print(f"Crate and documented dependency versions match: {current}")
        return

    package = version_match(manifest_text, PACKAGE, "Cargo.toml")
    edits = [(manifest, manifest_text, [package])]
    edits.extend((path, text, list(DEPENDENCY.finditer(text))) for path, text in documents)
    # Resolve entries first; replace from the end so earlier match offsets remain valid.
    for path, text, matches in edits:
        for match in reversed(matches):
            text = text[:match.start("version")] + args.version + text[match.end("version"):]
        path.write_text(text, encoding="utf-8")
    print(f"Updated crate and documented dependency versions: {current} -> {args.version}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
