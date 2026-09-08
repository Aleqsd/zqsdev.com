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

    def test_only_one_pdf_and_all_assets_are_local(self):
        self.assertEqual(list(CV.rglob("*.pdf")), [CV / "resume.pdf"])
        self.assertTrue((CV / "resume.pdf").read_bytes().startswith(b"%PDF-"))
        doc = Document((CV / "index.html").read_text(encoding="utf-8"))
        for tag, attrs in doc.elements:
            path = attrs.get("src") or (attrs.get("href") if tag == "link" and attrs.get("rel") != "canonical" else None)
            if path:
                self.assertFalse(urlparse(path).scheme, path)
                self.assertTrue((CV / path).is_file(), path)
        self.assertTrue(any(tag == "a" and a.get("href") == "https://www.viberank.app/profile/Aleqsd" for tag, a in doc.elements))

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
            for suffix in ("", "resume.pdf", "resume.css", "assets/viberank.svg"):
                with self.subTest(host=host, suffix=suffix):
                    self.assertEqual(resolve(f"https://{host}.zqsdev.com/{suffix}"), f"/cv/{suffix}")
        for host in ("www", "cv"):
            for variant in ("founding", "devops", "software"):
                for suffix in ("", "resume.pdf"):
                    with self.subTest(host=host, variant=variant, suffix=suffix):
                        self.assertEqual(resolve(f"https://{host}.zqsdev.com/cv/{variant}/{suffix}"), f"/cv/{suffix}")


if __name__ == "__main__":
    unittest.main()
