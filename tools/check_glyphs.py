"""List characters in src/*.rs that egui's built-in fonts can't draw (they show as squares).
Usage: python tools/check_glyphs.py [path to epaint_default_fonts/fonts]   (needs: pip install fonttools)"""
import glob, os, sys
from fontTools.ttLib import TTFont

fonts = sys.argv[1] if len(sys.argv) > 1 else glob.glob(os.path.expanduser("~/.cargo/registry/src/*/epaint_default_fonts-*/fonts"))[-1]
cover = set()
for f in ["Ubuntu-Light.ttf", "NotoEmoji-Regular.ttf", "emoji-icon-font.ttf", "Hack-Regular.ttf"]:
    cover |= set(TTFont(os.path.join(fonts, f)).getBestCmap().keys())
bad = 0
for p in sorted(glob.glob("src/*.rs")):
    for n, line in enumerate(open(p, encoding="utf-8"), 1):
        if line.strip().startswith("//"):
            continue
        for ch in sorted(set(line)):
            if ord(ch) > 127 and ord(ch) not in cover and ch not in "️‍":
                print(f"{p}:{n}: {ch!r} U+{ord(ch):04X}")
                bad += 1
sys.exit(1 if bad else 0)
