#!/usr/bin/env python3
"""Print one version's section of CHANGELOG.md (for the GitHub release notes).
Usage: python3 tools/changelog_section.py v0.2.3 [out.md]"""
import re, sys, pathlib

ver = sys.argv[1]
md = pathlib.Path("CHANGELOG.md").read_text(encoding="utf-8")
m = re.search(rf"^## {re.escape(ver)}\b.*?$(.*?)(?=^## |\Z)", md, flags=re.M | re.S)
text = m.group(1).strip() if m else f"RadioMaster {ver}"
if len(sys.argv) > 2:
    pathlib.Path(sys.argv[2]).write_text(text + "\n", encoding="utf-8")
else:
    sys.stdout.reconfigure(encoding="utf-8")
    print(text)
