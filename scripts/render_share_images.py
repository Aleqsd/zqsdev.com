"""Regenerate the CV link-preview images (og:image / twitter:image) with headless Chrome.

Writes static/cv/assets/share-<lang>-<sha256[:12]>.png (1200x630), deletes the previous
image for that language and points the og:image / twitter:image tags of the matching page at
the new file. Everything sits in the central 630x630 square (portrait, name, role, YC / Sony),
so apps that crop the preview to a square thumbnail or to 2:1 never cut text; the render fails
if anything but the background and the bottom bar lands outside that safe zone.

Usage: python3 scripts/render_share_images.py [--out DIR]
  --out DIR  only write preview PNGs to DIR (no change to static/cv).
Requires Chrome/Chromium (or CHROME=/path/to/chrome) and Pillow.
"""
import argparse, hashlib, html, io, os, re, shutil, subprocess, tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
CV = ROOT / "static" / "cv"
ASSETS = CV / "assets"
FONT = ASSETS / "ibm-plex-sans-3b031aa42161.ttf"
PORTRAIT = ASSETS / "portrait-slate-8526a4f2bf20.webp"
WIDTH, HEIGHT = 1200, 630
# Central square minus a margin; X crops 2:1 (15px top and bottom), WhatsApp/Teams/DMs crop squares.
SAFE_BOX = (305, 24, 895, 606)
BAR_HEIGHT = 8
SHARE_URL = re.compile(r"https://cv\.zqsdev\.com/assets/share-(en|fr)-[0-9a-f]{12}\.png")

YC_LOGO = ASSETS / "yc-program-854511d37323.svg"
SONY_LOGO = ASSETS / "brand-sony-c9d33af30df8.svg"

# Role first (Lead AI), then the two strongest references as logos (YC, Sony PlayStation).
PAGES = {
    "en": dict(page="index.html", role="Lead AI Software Engineer"),
    "fr": dict(page="fr.html", role="Lead Software Engineer · IA"),
}

# Colors follow the resume header (--ink / --accent / --muted in resume.css).
CSS = """
@font-face {font-family:Plex;src:url("%(font)s");font-weight:100 700;font-stretch:75%% 100%%}
* {box-sizing:border-box;margin:0}
html, body {width:%(w)dpx;height:%(h)dpx;overflow:hidden}
body {display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;
  background:#fff;color:#1e2d3b;font-family:Plex,sans-serif;padding-bottom:%(bar)dpx}
body::after {content:"";position:absolute;left:0;right:0;bottom:0;height:%(bar)dpx;background:#244e70}
.portrait {width:262px;height:262px;margin:0 0 -4px}
h1 {font-size:64px;font-weight:700;letter-spacing:-1.5px;line-height:1.02}
.role {font-size:36px;font-weight:600;letter-spacing:-.3px;line-height:1.2;color:#244e70;margin-top:10px}
.proof {display:flex;align-items:center;gap:22px;margin-top:22px;font-size:27px;font-weight:600;color:#1e2d3b}
.proof span {display:flex;align-items:center;gap:10px}
.proof i {width:2px;height:34px;background:#cbd7e0}
.yc {height:36px}
.sony {height:21px}
"""


def find_chrome():
    candidates = [os.environ.get("CHROME"),
                  r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                  *(shutil.which(name) for name in ("google-chrome", "chromium", "chromium-browser", "chrome"))]
    return next(c for c in candidates if c and Path(c).is_file())


def page_html(lang):
    text = PAGES[lang]
    css = CSS % {"font": FONT.as_uri(), "w": WIDTH, "h": HEIGHT, "bar": BAR_HEIGHT}
    return f"""<!doctype html><html lang="{lang}"><head><meta charset="utf-8"><style>{css}</style></head><body>
<img class="portrait" src="{PORTRAIT.as_uri()}" alt="">
<h1>Alexandre<br>DO-O ALMEIDA</h1>
<p class="role">{html.escape(text['role'])}</p>
<p class="proof"><span><img class="yc" src="{YC_LOGO.as_uri()}" alt="Y Combinator">S20</span><i></i>
<span><img class="sony" src="{SONY_LOGO.as_uri()}" alt="Sony">PlayStation</span></p>
</body></html>"""


def outside_safe_zone(image):
    """Bounding box of non-background pixels outside SAFE_BOX (bottom bar excluded), or None."""
    left, top, right, bottom = SAFE_BOX
    ink = image.convert("L").point(lambda v: 255 if v < 245 else 0)
    ink.paste(0, (left, top, right, bottom))
    ink.paste(0, (0, HEIGHT - BAR_HEIGHT, WIDTH, HEIGHT))
    return ink.getbbox()


def render(chrome, lang, tmp):
    src, shot = tmp / f"share-{lang}.html", tmp / f"share-{lang}.png"
    src.write_text(page_html(lang), encoding="utf-8")
    subprocess.run([chrome, "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
                    "--hide-scrollbars", "--force-device-scale-factor=1", "--allow-file-access-from-files",
                    f"--window-size={WIDTH},{HEIGHT}", f"--screenshot={shot}", src.as_uri()],
                   check=True, capture_output=True, timeout=60)
    image = Image.open(shot).convert("RGB")
    assert image.size == (WIDTH, HEIGHT), f"Chrome rendered {image.size}, expected {(WIDTH, HEIGHT)}"
    spill = outside_safe_zone(image)
    assert spill is None, f"{lang}: content outside the safe zone {SAFE_BOX} at {spill}"
    buffer = io.BytesIO()
    image.save(buffer, "PNG", optimize=True)
    return buffer.getvalue()


def publish(lang, data):
    name = f"share-{lang}-{hashlib.sha256(data).hexdigest()[:12]}.png"
    for old in ASSETS.glob(f"share-{lang}-*.png"):
        if old.name != name:
            old.unlink()
    (ASSETS / name).write_bytes(data)
    page = CV / PAGES[lang]["page"]
    source = page.read_bytes().decode("utf-8")  # keep the file's own line endings
    updated, count = SHARE_URL.subn(f"https://cv.zqsdev.com/assets/{name}", source)
    assert count == 2, f"{page.name}: expected og:image and twitter:image, found {count} share URLs"
    if updated != source:
        page.write_bytes(updated.encode("utf-8"))
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, help="write previews here instead of updating static/cv")
    args = parser.parse_args()
    chrome = find_chrome()
    with tempfile.TemporaryDirectory(prefix="zqs-share-") as tmp:
        for lang in PAGES:
            data = render(chrome, lang, Path(tmp))
            if args.out:
                args.out.mkdir(parents=True, exist_ok=True)
                target = args.out / f"share-{lang}.png"
                target.write_bytes(data)
            else:
                target = ASSETS / publish(lang, data)
            print(f"{target} ({len(data) // 1024} KB)")


if __name__ == "__main__":
    main()
