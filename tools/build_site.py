"""Keep the shared parts of the website the same on every page: the header menu, the footer and the
lists of guides. Each lives between marker comments (<!-- NAV -->, <!-- FOOTER -->, <!-- GUIDES:… -->).

Run it after adding a page or a guide: python3 tools/build_site.py (the Pages workflow runs it too)."""

import html
import pathlib
import re

SITE = pathlib.Path("site")
REPO = "https://github.com/epell-development/RadioMaster"
DISCORD = "https://discord.gg/zVxeUXCjhc"

# (label, link, the part of the site it lights up for)
NAV = [
    ("Features", "/#features", None),
    ("Guides", "/guides/", "/guides/"),
    ("Server", "/server/", "/server/"),
    ("Updates", "/updates/", "/updates/"),
    ("Support", DISCORD, None),
]

# Every guide, in the order they're listed. (address, title, one line about it, group, on the home page)
GUIDES = [
    ("/guides/start-an-internet-radio-station/", "Start an internet radio station", "Music and licences, a streaming server, playlists and going live, step by step.", "Get started", True),
    ("/guides/stream-to-icecast-shoutcast/", "Stream to Icecast or Shoutcast", "Add an encoder, pick a format and bitrate, and send song titles to your listeners.", "Get started", False),
    ("/guides/playlist-generator/", "Use the playlist generator", "Categories, templates and patterns that build hours of programming in one click.", "Run the station", True),
    ("/guides/advertisement-scheduler/", "Schedule adverts and jingles", "Rules for every N songs, every N minutes or set times, with run dates for each advert.", "Run the station", True),
    ("/guides/event-scheduler/", "Automate with the event scheduler", "Generate tomorrow's playlist, switch shows and run commands at set times.", "Run the station", True),
    ("/guides/users-and-permissions/", "Set up users and permissions", "A login for every presenter, and exactly which tools each one can open.", "Run the station", False),
    ("/guides/live-shows/", "Present a live show", "The microphone, the cart wall, voice tracks and text to speech.", "On air", False),
    ("/guides/azuracast/", "Manage an AzuraCast station", "Connect your AzuraCast server, then run the queue, requests, media and DJs from the studio.", "On air", True),
    ("/guides/broadcast-from-anywhere/", "Broadcast from anywhere", "Put the station on a server and run it from the desktop app or a phone browser.", "Server and remote", True),
    ("/server/docs/", "RadioMaster Server documentation", "Install, HTTPS and Let's Encrypt, the web portal, Cloudflare Tunnel, reverse proxies and troubleshooting.", "Server and remote", False),
    ("/guides/plugins/", "Make a plugin", "Run your own scripts when a song starts or the hour changes, and control RadioMaster from them.", "Extend RadioMaster", True),
]


def header(here: str) -> str:
    links = []
    for label, href, part in NAV:
        on = part is not None and here.startswith(part)
        attrs = ' class="active" aria-current="page"' if on else ""
        links.append(f'      <a{attrs} href="{href}">{label}</a>')
    return (
        '<header class="top">\n'
        '  <div class="wrap">\n'
        '    <a class="brand" href="/"><img src="/img/icon.png" alt="RadioMaster logo" width="34" height="34"><span>Radio<b>Master</b></span></a>\n'
        '    <nav class="links" id="site-nav" aria-label="Main">\n'
        + "\n".join(links)
        + "\n    </nav>\n"
        '    <a class="btn primary small nav-dl" href="/#download">Download</a>\n'
        '    <button class="menu-btn" type="button" aria-controls="site-nav" aria-expanded="false" aria-label="Open menu"><span></span></button>\n'
        "  </div>\n"
        "</header>\n"
        '<script src="/site.js" defer></script>'
    )


FOOTER = f"""<footer>
  <div class="wrap">
    <div class="row">
      <a class="brand" href="/" style="font-size:1.2rem"><img src="/img/icon.png" alt="" style="width:24px;height:24px;border-radius:6px"><span>Radio<b>Master</b></span></a>
      <a href="/guides/">Guides</a>
      <a href="/server/">RadioMaster Server</a>
      <a href="/server/docs/">Server docs</a>
      <a href="/updates/">Updates</a>
      <a href="/#faq">FAQ</a>
      <a href="{REPO}">GitHub</a>
      <a href="{DISCORD}">Support Discord</a>
      <a href="{REPO}/releases">Releases</a>
      <a href="{REPO}/issues">Report a problem</a>
      <a href="{REPO}/blob/main/LICENSE">MIT licence</a>
    </div>
    <p>RadioMaster is an independent project and is not affiliated with DJSoft.net (RadioBOSS), SAM Broadcaster, ZaraRadio, Icecast, Shoutcast or AzuraCast. Names are used only to describe compatibility.</p>
    <p>RadioMaster - A <a href="https://deg.dovafm.com">DovaFM Digital Entertainment Group</a> Brand.</p>
  </div>
</footer>"""


def card(href: str, title: str, blurb: str, group: str | None) -> str:
    tag = f'<span class="tag">{html.escape(group)}</span>' if group else ""
    return f'<a class="guide-card" href="{href}">{tag}<b>{html.escape(title)}</b><span>{html.escape(blurb)}</span></a>'



def guides_home() -> str:
    cards = [card(h, t, b, g) for h, t, b, g, home in GUIDES if home]
    return '<div class="guide-grid">\n' + "\n".join(cards) + "\n</div>"


def guides_all() -> str:
    out = []
    for group in dict.fromkeys(g for _, _, _, g, _ in GUIDES):
        cards = [card(h, t, b, None) for h, t, b, g, _ in GUIDES if g == group]
        out.append(f'<h2 class="guide-group">{html.escape(group)}</h2>\n<div class="guide-grid">\n' + "\n".join(cards) + "\n</div>")
    return "\n".join(out)


def related(here: str) -> str:
    """Up to three other guides from the same group (then the first ones), for the end of a guide."""
    me = next((g for h, _, _, g, _ in GUIDES if h == here), None)
    others = [x for x in GUIDES if x[0] != here]
    picks = [x for x in others if x[3] == me] + [x for x in others if x[3] != me]
    return '<div class="guide-grid">\n' + "\n".join(card(h, t, b, g) for h, t, b, g, _ in picks[:3]) + "\n</div>"


def fill(text: str, name: str, body: str) -> str:
    pat = re.compile(rf"(<!-- {re.escape(name)} -->).*?(<!-- /{re.escape(name)} -->)", re.S)
    return pat.sub(lambda m: m.group(1) + "\n" + body + "\n" + m.group(2), text)


changed = 0
for page in sorted(SITE.rglob("*.html")):
    text = page.read_text(encoding="utf-8")
    if "<!-- NAV -->" not in text:
        continue  # redirects and other pages without the shared parts
    rel = page.relative_to(SITE).as_posix()
    here = "/" + (rel[: -len("index.html")] if rel.endswith("index.html") else rel)
    new = fill(text, "NAV", header(here))
    new = fill(new, "FOOTER", FOOTER)
    new = fill(new, "GUIDES:home", guides_home())
    new = fill(new, "GUIDES:all", guides_all())
    new = fill(new, "GUIDES:related", related(here))
    if new != text:
        page.write_text(new, encoding="utf-8")
        changed += 1
print(f"site: shared header, footer and guide lists checked, {changed} page(s) updated")
