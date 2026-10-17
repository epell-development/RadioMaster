#!/usr/bin/env python3
"""Render CHANGELOG.md into site/updates.html (between the RELEASES markers).
Run from the repository root:  python3 tools/build_updates.py"""
import html, pathlib, re

REPO = "https://github.com/epell-development/RadioMaster"


def inline(text: str) -> str:
    t = html.escape(text)
    t = re.sub(r"\*\*(.+?)\*\*", r"<b>\1</b>", t)
    t = re.sub(r"`(.+?)`", r"<code>\1</code>", t)
    return t


def render(md: str) -> str:
    out = []
    releases = re.split(r"^## ", md, flags=re.M)[1:]
    for k, rel in enumerate(releases):
        lines = rel.strip("\n").split("\n")
        head = lines[0]
        m = re.match(r"(v[\d.]+)\s*[—-]\s*(.+)", head)
        ver, date = (m.group(1), m.group(2).strip()) if m else (head.strip(), "")
        body, items, group = [], [], None

        def flush():
            nonlocal items
            if items:
                body.append("<ul>" + "".join(f"<li>{inline(i)}</li>" for i in items) + "</ul>")
                items = []

        for line in lines[1:]:
            if not line.strip():
                continue
            g = re.match(r"\*\*(.+?)\*\*\s*$", line.strip())
            if g:
                flush()
                body.append(f"<h3>{html.escape(g.group(1))}</h3>")
            elif line.startswith("- "):
                items.append(line[2:].strip())
            elif line.startswith("  ") and items:
                items[-1] += " " + line.strip()
            else:
                flush()
                body.append(f"<p class=\"lede\" style=\"margin-bottom:14px\">{inline(line.strip())}</p>")
        flush()
        latest = '<span class="tag-latest">Latest</span>' if k == 0 else ""
        out.append(
            f'<article class="release" id="{ver}">\n'
            f'  <div class="ver">{latest}<b>{ver}</b><span>{html.escape(date)}</span></div>\n'
            f'  <div>{"".join(body)}\n'
            f'    <div class="dl-row"><a class="btn {"primary" if k == 0 else "ghost"} small" href="{REPO}/releases/tag/{ver}">Download {ver}</a></div>\n'
            f'  </div>\n</article>'
        )
    return "\n".join(out)


page = pathlib.Path("site/updates.html")
text = page.read_text(encoding="utf-8")
new = render(pathlib.Path("CHANGELOG.md").read_text(encoding="utf-8"))
text = re.sub(r"(<!-- RELEASES -->).*?(<!-- /RELEASES -->)", lambda m: m.group(1) + "\n" + new + "\n" + m.group(2), text, flags=re.S)
page.write_text(text, encoding="utf-8")
print("site/updates.html updated")
