#!/usr/bin/env python3
"""Check that the version is the same everywhere a release reads it: Cargo.toml, Cargo.lock,
installer/RadioMaster.iss (AppVersion) and the newest CHANGELOG.md section. A tag build takes the
version and the release notes from these, so a mismatch ships the wrong version.
Usage: python3 tools/check_versions.py [vX.Y.Z]   (with a tag, it must match too)"""
import pathlib, re, sys


def find(path, pattern):
    m = re.search(pattern, pathlib.Path(path).read_text(encoding="utf-8"), re.M | re.S)
    return m.group(1) if m else None


found = {
    "Cargo.toml": find("Cargo.toml", r'^\[package\].*?^version\s*=\s*"([^"]+)"'),
    "Cargo.lock": find("Cargo.lock", r'^name = "radiomaster"\nversion = "([^"]+)"'),
    "installer/RadioMaster.iss": find("installer/RadioMaster.iss", r'#define AppVersion "([^"]+)"'),
    "CHANGELOG.md": find("CHANGELOG.md", r"^## v(\d[\w.-]*)"),
}
if len(sys.argv) > 1:
    found["tag"] = sys.argv[1].removeprefix("refs/tags/").lstrip("v")

want = found["Cargo.toml"]
bad = [f"{k}: {v or 'no version found'}" for k, v in found.items() if v != want]
if bad:
    print(f"Version mismatch (Cargo.toml says {want}):\n  " + "\n  ".join(bad))
    sys.exit(1)
print(f"All versions are {want}")
