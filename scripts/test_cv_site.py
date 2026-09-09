"""Static public CV contracts; no network or AI credentials required."""

from html.parser import HTMLParser
from pathlib import Path
import re
import tomllib
import unittest
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
CV = ROOT / "static" / "cv"


class Document(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.elements = []
        self.copy = []
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        self.elements.append((tag, dict(attrs)))

    def handle_data(self, text):
        self.copy.append(text)


class PublicCvTests(unittest.TestCase):
    def test_html_is_readable_without_pdf_renderer(self):
        doc = Document((CV / "index.html").read_text(encoding="utf-8"))
        tags = [tag for tag, _ in doc.elements]
        self.assertIn("main", tags)
        self.assertEqual(tags.count("h1"), 1)
        self.assertFalse({"canvas", "iframe", "object", "embed"}.intersection(tags))
        copy = " ".join(" ".join(doc.copy).split())
        for text in ("Lead AI Software Engineer", "50,000 students", "5,000+ games", "millions of players", "first 30 hires", "60+ employees"):
            self.assertIn(text, copy)
        for obsolete in ("View PDF", "DevOps CV", "Software CV", "Resume Selector"):
            self.assertNotIn(obsolete, copy)
        downloads = [a for tag, a in doc.elements if tag == "a" and "download" in a]
        self.assertEqual(len(downloads), 1)
        self.assertEqual(downloads[0]["href"], "resume.pdf")

    def test_one_pdf_per_language_and_all_assets_are_local(self):
        self.assertEqual(set(CV.rglob("*.pdf")), {CV / "resume.pdf", CV / "resume-fr.pdf"})
        for name in ("resume.pdf", "resume-fr.pdf"):
            self.assertTrue((CV / name).read_bytes().startswith(b"%PDF-"))
        for name in ("index.html", "fr.html"):
            doc = Document((CV / name).read_text(encoding="utf-8"))
            for tag, attrs in doc.elements:
                path = attrs.get("src") or (attrs.get("href") if tag == "link" and attrs.get("rel") not in ("canonical", "alternate") else None)
                if path:
                    self.assertFalse(urlparse(path).scheme, path)
                    self.assertTrue((CV / path).is_file(), path)
            self.assertTrue(any(tag == "a" and a.get("href") == "https://www.viberank.app/profile/Aleqsd" for tag, a in doc.elements))

    def test_language_switch_and_matching_download_work_without_javascript(self):
        cases = (
            ("index.html", "en", "fr", "fr.html", "resume.pdf", "Download PDF", "Copy Email"),
            ("fr.html", "fr", "en", "./", "resume-fr.pdf", "Télécharger le PDF", "Copier l’e-mail"),
        )
        for name, language, target_language, target, pdf, label, clipboard in cases:
            with self.subTest(language=language):
                doc = Document((CV / name).read_text(encoding="utf-8"))
                self.assertIn(("html", {"lang": language}), doc.elements)
                links = [a for tag, a in doc.elements if tag == "a"]
                switch = next(a for a in links if a.get("class") == "language-switch")
                self.assertEqual(switch["href"], target)
                self.assertEqual(switch["hreflang"], target_language)
                self.assertEqual(switch["lang"], target_language)
                downloads = [a for a in links if "download" in a]
                self.assertEqual(len(downloads), 1)
                self.assertEqual(downloads[0]["href"], pdf)
                if language == "fr":
                    self.assertTrue(downloads[0]["download"].endswith("_FR.pdf"))
                copy = " ".join(" ".join(doc.copy).split())
                self.assertIn(label, copy)
                self.assertIn(clipboard, copy)
                self.assertEqual(sum(tag == "h1" for tag, _ in doc.elements), 1)
                self.assertFalse({"canvas", "iframe", "object", "embed"}.intersection(tag for tag, _ in doc.elements))
                alternates = {a["hreflang"]: a["href"] for tag, a in doc.elements if tag == "link" and a.get("rel") == "alternate"}
                self.assertEqual(alternates["en"], "https://cv.zqsdev.com/")
                self.assertEqual(alternates["fr"], "https://cv.zqsdev.com/fr.html")
        french = " ".join(Document((CV / "fr.html").read_text(encoding="utf-8")).copy)
        for text in ("50 000 étudiants", "5 000+ jeux", "millions de joueurs", "30 premiers recrutements", "60+ collaborateurs", "12 M$ en série A", "148 Md+ tokens", "cache inclus", "TOEIC 990/990"):
            self.assertIn(text, french)

    def test_toolbar_has_decorative_monochrome_brand_marks(self):
        for name in ("index.html", "fr.html"):
            doc = Document((CV / name).read_text(encoding="utf-8"))
            icons = [a for tag, a in doc.elements if tag == "svg" and a.get("class") == "brand-icon"]
            self.assertEqual(len(icons), 2)
            for icon in icons:
                self.assertEqual(icon["fill"], "currentColor")
                self.assertEqual(icon["aria-hidden"], "true")
                self.assertEqual(icon["focusable"], "false")

    def test_above_the_fold_portrait_stays_lightweight(self):
        # A ~100px portrait must not make mobile visitors download the 1.8MB master.
        for name in ("index.html", "fr.html"):
            with self.subTest(page=name):
                doc = Document((CV / name).read_text(encoding="utf-8"))
                portrait = next(a for tag, a in doc.elements if tag == "img" and a.get("class") == "portrait")
                self.assertLess((CV / portrait["src"]).stat().st_size, 64 * 1024)
                self.assertNotEqual(portrait.get("loading"), "lazy")
                self.assertEqual(portrait.get("fetchpriority"), "high")
                self.assertGreater(int(portrait["width"]), 0)
                self.assertEqual(portrait["width"], portrait["height"])

    def test_legacy_links_reach_the_single_resume(self):
        rules = tomllib.loads((ROOT / "netlify.toml").read_text(encoding="utf-8"))["redirects"]

        def resolve(url):
            # Resolve this site's exact / wildcard rules, stopping at a rewrite.
            visited = set()
            while url not in visited:
                visited.add(url)
                for rule in rules:
                    pattern = rule["from"]
                    candidate = url if pattern.startswith("https://") else urlparse(url).path
                    match = re.fullmatch(re.escape(pattern).replace(r"\*", "(.*)"), candidate)
                    if not match:
                        continue
                    target = rule["to"].replace(":splat", match.group(1) if match.groups() else "")
                    if rule["status"] == 200:
                        return target
                    url = target
                    break
                else:
                    self.fail(f"No route for {url}")
            self.fail(f"Redirect loop at {url}")

        for host in ("cv", "founding", "devops", "software"):
            for suffix in ("", "fr.html", "resume.pdf", "resume-fr.pdf", "resume.css", "assets/viberank.svg"):
                with self.subTest(host=host, suffix=suffix):
                    self.assertEqual(resolve(f"https://{host}.zqsdev.com/{suffix}"), f"/cv/{suffix}")
        for host in ("www", "cv"):
            for variant in ("founding", "devops", "software"):
                for suffix in ("", "resume.pdf"):
                    with self.subTest(host=host, variant=variant, suffix=suffix):
                        self.assertEqual(resolve(f"https://{host}.zqsdev.com/cv/{variant}/{suffix}"), f"/cv/{suffix}")


if __name__ == "__main__":
    unittest.main()
