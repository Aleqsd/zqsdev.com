"""Regenerate static/cv/resume.pdf and resume-fr.pdf from the HTML CV with headless Chrome.

Usage: python3 scripts/render_cv_pdfs.py [--check]

The HTML uses a 512px portrait; the approved PDFs embed the 1254px original, which is not
shipped as a web asset. It is taken from the current resume.pdf and swapped in for printing
only. Prints the text diff against the current PDFs; --check renders without replacing them.
Requires Chrome/Chromium (or CHROME=/path/to/chrome) and PyMuPDF (`pip install pymupdf`).
"""
import difflib, functools, http.server, os, re, shutil, subprocess, sys, tempfile, threading
from pathlib import Path

import fitz

CV = Path(__file__).resolve().parents[1] / "static" / "cv"
PAGES = (("index.html", "resume.pdf"), ("fr.html", "resume-fr.pdf"))
PORTRAIT = re.compile(r'(<img[^>]*class="portrait"[^>]*src=")([^"]+)(")')


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def find_chrome():
    candidates = [os.environ.get("CHROME"),
                  r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                  *(shutil.which(name) for name in ("google-chrome", "chromium", "chromium-browser", "chrome"))]
    return next(c for c in candidates if c and Path(c).is_file())


def page_text(path):
    with fitz.open(path) as doc:
        assert len(doc) == 1, f"{path.name} must stay on one page, got {len(doc)}"
        return doc[0].get_text().splitlines()


def main(check):
    chrome = find_chrome()
    # Short temp paths: Chrome on Windows silently writes nothing when --print-to-pdf is too long.
    with tempfile.TemporaryDirectory(prefix="cv") as tmp:
        site, out = Path(tmp) / "site", Path(tmp) / "out"
        shutil.copytree(CV, site)
        out.mkdir()
        with fitz.open(CV / "resume.pdf") as doc:
            xref = max(doc[0].get_images(), key=lambda image: image[2])[0]
            fitz.Pixmap(doc, xref).save(site / "assets" / "portrait-print.png")
        for page, _ in PAGES:
            html = (site / page).read_text(encoding="utf-8")
            html, swapped = PORTRAIT.subn(r"\1assets/portrait-print.png\3", html)
            assert swapped == 1, f"portrait not found in {page}"
            (site / page).write_text(html, encoding="utf-8")

        handler = functools.partial(QuietHandler, directory=str(site))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            for page, pdf in PAGES:
                subprocess.run([chrome, "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
                                f"--user-data-dir={Path(tmp) / 'profile'}", "--no-pdf-header-footer",
                                "--run-all-compositor-stages-before-draw", "--virtual-time-budget=8000",
                                f"--print-to-pdf={out / pdf}", f"http://127.0.0.1:{server.server_address[1]}/{page}"],
                               check=True, capture_output=True, timeout=120)
                assert (out / pdf).is_file(), f"Chrome did not write {pdf}"
        finally:
            server.shutdown()

        for _, pdf in PAGES:
            diff = [line for line in difflib.unified_diff(page_text(CV / pdf), page_text(out / pdf), lineterm="", n=0)
                    if line[:1] in "+-" and line[:3] not in ("+++", "---")]
            print(f"{pdf}: {'no text change' if not diff else f'{len(diff)} changed lines'}")
            for line in diff:
                print("  " + line)
            if not check:
                shutil.copyfile(out / pdf, CV / pdf)


if __name__ == "__main__":
    main("--check" in sys.argv[1:])
