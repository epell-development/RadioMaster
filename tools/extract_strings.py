#!/usr/bin/env python3
"""Collect every translatable text in the source into lang/template.json.
Run from the repository root:  python3 tools/extract_strings.py"""
import json, re, pathlib

texts = set()
for f in pathlib.Path("src").glob("*.rs"):
    s = f.read_text(encoding="utf-8")
    for m in re.finditer(r'tr\(\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)', s):
        texts.add(m.group(1))
    # column headings: `for t in ["#", "Title", …]` and `let mut heads = vec![…]`, plus heads.push("…")
    for arr in re.findall(r'for (?:t|h) in \[([^\]]*)\]', s) + re.findall(r'heads = vec!\[([^\]]*)\]', s):
        texts.update(x for x in re.findall(r'"((?:[^"\\]|\\.)*)"', arr) if re.search("[A-Za-z]", x))
    texts.update(re.findall(r'heads\.push\("((?:[^"\\]|\\.)*)"\)', s))
    # labels returned by `fn label(self)` match arms (tabs, tools, settings pages…)
    for body in re.findall(r'fn label\(self\) -> &\'static str \{(.*?)\n    \}', s, re.S):
        texts.update(re.findall(r'=> "((?:[^"\\]|\\.)*)"', body))
texts = sorted(t.encode().decode("unicode_escape").encode("latin-1").decode("utf-8") if "\\" in t else t for t in texts)
out = {t: "" for t in texts}
pathlib.Path("lang").mkdir(exist_ok=True)
pathlib.Path("lang/template.json").write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
print(f"{len(out)} texts written to lang/template.json")
