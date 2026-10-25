#!/usr/bin/env python3
"""Check that every link and image inside the website (site/) points at something that exists:
local pages and files, #anchors, and links back to https://radiomaster.dovafm.com/… plus the sitemap.
Links to other sites are not fetched. Run from the repository root after the site is assembled
(CI copies docs/screenshots, assets/icon.png and docs/social-card.png into site/img first):
    python3 tools/check_site.py"""
import html.parser, pathlib, re, sys, urllib.parse

SITE = pathlib.Path("site")
DOMAIN = "https://radiomaster.dovafm.com"


class Page(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids = [], set()

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("id"):
            self.ids.add(a["id"])
        if tag == "a" and a.get("name"):
            self.ids.add(a["name"])
        for k in ("href", "src"):
            if a.get(k):
                self.links.append((self.getpos()[0], a[k]))
        if tag == "meta" and a.get("http-equiv", "").lower() == "refresh":
            m = re.search(r"url=(\S+)", a.get("content", ""), re.I)
            if m:
                self.links.append((self.getpos()[0], m.group(1)))
        if tag == "meta" and (a.get("property") in ("og:image", "og:image:url") or a.get("name") == "twitter:image"):
            if a.get("content"):
                self.links.append((self.getpos()[0], a["content"]))


def target(path: str):
    """The file a site path is served from, or None."""
    f = SITE / path.lstrip("/")
    if f.is_dir():
        f = f / "index.html"
    return f if f.is_file() else None


pages = {}
for f in sorted(SITE.rglob("*.html")):
    p = Page()
    p.feed(f.read_text(encoding="utf-8"))
    pages[f] = p

bad = []
for f, p in pages.items():
    base = "/" + f.relative_to(SITE).as_posix()
    for line, link in p.links:
        if link.startswith(DOMAIN):
            link = link[len(DOMAIN):] or "/"
        u = urllib.parse.urlsplit(link)
        if u.scheme or u.netloc or link.startswith(("mailto:", "tel:", "data:", "javascript:")):
            continue
        path = urllib.parse.urljoin(base, u.path) if u.path else base
        t = target(urllib.parse.unquote(path))
        if t is None:
            bad.append(f"{f}:{line}: {link} (no such page or file)")
        elif u.fragment and u.fragment != "top" and t in pages and u.fragment not in pages[t].ids:
            bad.append(f"{f}:{line}: {link} (no #{u.fragment} on that page)")

sitemap = SITE / "sitemap.xml"
for loc in re.findall(r"<(?:image:)?loc>([^<]+)</", sitemap.read_text(encoding="utf-8")):
    if not loc.startswith(DOMAIN) or target(loc[len(DOMAIN):] or "/") is None:
        bad.append(f"{sitemap}: {loc} (not on the site)")

print("\n".join(bad) or f"{len(pages)} pages checked, all links resolve")
sys.exit(1 if bad else 0)
