"""Static public CV contracts; no network or AI credentials required."""

from html.parser import HTMLParser
from pathlib import Path
import json
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
    def test_profile_schema_identifies_the_same_person_in_both_languages(self):
        identities = []
        for filename, language, url in (
            ("index.html", "en", "https://cv.zqsdev.com/"),
            ("fr.html", "fr", "https://cv.zqsdev.com/fr.html"),
        ):
            with self.subTest(language=language):
                text = (CV / filename).read_text(encoding="utf-8")
                blocks = re.findall(r'<script type="application/ld\+json">\s*(.*?)\s*</script>', text, re.S)
                self.assertEqual(len(blocks), 1)
                schema = json.loads(blocks[0])
                self.assertEqual(schema["@context"], "https://schema.org")
                graph = {node["@type"]: node for node in schema["@graph"]}
                self.assertEqual(set(graph), {"WebSite", "ProfilePage", "Person"})
                page, person, website = (graph[k] for k in ("ProfilePage", "Person", "WebSite"))
                self.assertEqual(page["url"], url)
                self.assertEqual(page["inLanguage"], language)
                self.assertEqual(page["mainEntity"], {"@id": person["@id"]})
                self.assertEqual(page["isPartOf"], {"@id": website["@id"]})
                self.assertEqual(website["publisher"], {"@id": person["@id"]})
                self.assertEqual(person["@id"], "https://cv.zqsdev.com/#person")
                self.assertEqual(person["name"], "Alexandre DO-O ALMEIDA")
                self.assertEqual(person["address"], {"@type": "PostalAddress", "addressLocality": "Montpellier", "addressCountry": "FR"})
                self.assertEqual(person["knowsLanguage"], ["fr", "en"])
                doc = Document(text)
                links = {a.get("href") for tag, a in doc.elements if tag == "a"}
                self.assertTrue(set(person["sameAs"]).issubset(links))
                self.assertIn(person["email"], links)
                portrait = next(a["src"] for tag, a in doc.elements if tag == "img" and a.get("class") == "portrait")
                self.assertEqual(person["image"], "https://cv.zqsdev.com/" + portrait)
                ids = {a["id"] for _, a in doc.elements if "id" in a}
                for part in page["hasPart"]:
                    self.assertIn(urlparse(part["@id"]).fragment, ids)
                # Employment, dates and ratings must not be invented from a CV.
                for key in ("worksFor", "award", "aggregateRating", "interactionStatistic"):
                    self.assertNotIn(key, person)
                self.assertNotIn("dateModified", page)
                self.assertNotIn("dateCreated", page)
                identities.append({k: person[k] for k in ("@id", "name", "url", "image", "email", "sameAs")})
        self.assertEqual(*identities)

    def test_search_metadata_matches_the_language_and_visible_profile(self):
        for filename, locale in (("index.html", "fr_FR"), ("fr.html", "en_US")):
            with self.subTest(page=filename):
                doc = Document((CV / filename).read_text(encoding="utf-8"))
                metadata = {a.get("property") or a.get("name"): a.get("content") for tag, a in doc.elements if tag == "meta"}
                self.assertEqual(metadata["author"], "Alexandre DO-O ALMEIDA")
                self.assertIn("index, follow", metadata["robots"])
                self.assertIn("max-image-preview:large", metadata["robots"])
                self.assertEqual(metadata["og:locale:alternate"], locale)
                self.assertEqual(metadata["twitter:image:alt"], metadata["og:image:alt"])
                self.assertLessEqual(len(metadata["description"]), 165)
                if filename == "fr.html":
                    self.assertIn("DevOps pour PlayStation", metadata["description"])
                    self.assertNotIn("Sony PlayStation et Studi", metadata["description"])
                sitemap = next(a for tag, a in doc.elements if tag == "link" and a.get("rel") == "sitemap")
                self.assertEqual(sitemap["href"], "sitemap.xml")
                self.assertTrue((CV / sitemap["href"]).is_file())

    def test_citable_sections_and_machine_readable_dates_exist_in_both_languages(self):
        for filename in ("index.html", "fr.html"):
            with self.subTest(page=filename):
                doc = Document((CV / filename).read_text(encoding="utf-8"))
                ids = [a["id"] for _, a in doc.elements if "id" in a]
                self.assertEqual(len(ids), len(set(ids)))
                for identifier in ("resume", "summary", "studi", "playstation", "jam-gg", "experience-title", "skills-title", "education-title"):
                    self.assertIn(identifier, ids)
                self.assertIn(("section", {"aria-labelledby": "summary", "class": "agentic"}), doc.elements)
                dates = [a["datetime"] for tag, a in doc.elements if tag == "time"]
                self.assertEqual(dates, ["2026-04", "2023-02", "2026-03", "2020-03", "2023-03"])

    def test_html_is_readable_without_pdf_renderer(self):
        doc = Document((CV / "index.html").read_text(encoding="utf-8"))
        tags = [tag for tag, _ in doc.elements]
        self.assertIn("main", tags)
        self.assertEqual(tags.count("h1"), 1)
        self.assertFalse({"canvas", "iframe", "object", "embed"}.intersection(tags))
        copy = " ".join(" ".join(doc.copy).split())
        for text in ("Lead AI Software Engineer", "50,000 students", "5,000+ games", "millions of players", "first 30 hires", "60+ employees", "152B+ tokens", "Top 1% worldwide on Token Tracker in October 2026"):
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
            self.assertTrue(any(tag == "a" and a.get("href") == "https://www.tokentracker.cc/u/ad7485bd-6569-45a5-850d-44a9de505b1b" for tag, a in doc.elements))

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
        for text in ("50 000 étudiants", "5 000+ jeux", "millions de joueurs", "30 premiers recrutements", "60+ collaborateurs", "12 M$ en série A", "152 Md+ tokens", "Top 1 % mondial sur Token Tracker en octobre 2026", "TOEIC 990/990"):
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

    def test_secondary_links_and_contact_are_available_without_javascript(self):
        for name in ("index.html", "fr.html"):
            doc = Document((CV / name).read_text(encoding="utf-8"))
            self.assertTrue(any(tag == "details" for tag, _ in doc.elements))
            self.assertTrue(any(tag == "summary" and attrs.get("aria-label") for tag, attrs in doc.elements))
            self.assertTrue(any(tag == "a" and attrs.get("href") == "mailto:alexandre@zqsdev.com" for tag, attrs in doc.elements))
            for tag, attrs in doc.elements:
                if tag == "svg" and attrs.get("class") == "action-icon":
                    self.assertEqual(attrs.get("aria-hidden"), "true")
            for emoji in ("🚀", "⬇️", "✉️"):
                self.assertNotIn(emoji, " ".join(doc.copy))

    def test_share_previews_and_compressed_font(self):
        for name in ("index.html", "fr.html"):
            doc = Document((CV / name).read_text(encoding="utf-8"))
            metadata = {a.get("property") or a.get("name"): a.get("content") for tag, a in doc.elements if tag == "meta"}
            image_url = metadata["og:image"]
            self.assertTrue(image_url.startswith("https://cv.zqsdev.com/assets/share-"))
            image = CV / urlparse(image_url).path.lstrip("/")
            content = image.read_bytes()
            self.assertTrue(content.startswith(b"\x89PNG\r\n\x1a\n"))
            self.assertEqual(int.from_bytes(content[16:20], "big"), 1200)
            self.assertEqual(int.from_bytes(content[20:24], "big"), 630)
            self.assertEqual(metadata["twitter:image"], image_url)
            self.assertEqual(metadata["twitter:card"], "summary_large_image")
            preload = next(a for tag, a in doc.elements if tag == "link" and a.get("as") == "font")
            font = (CV / preload["href"]).read_bytes()
            self.assertEqual(font[:4], b"wOF2")
            self.assertLess(len(font), 100 * 1024)

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
            for suffix in ("", "fr.html", "resume.pdf", "resume-fr.pdf", "resume.css", "assets/tokentracker.svg"):
                with self.subTest(host=host, suffix=suffix):
                    self.assertEqual(resolve(f"https://{host}.zqsdev.com/{suffix}"), f"/cv/{suffix}")
        for host in ("www", "cv"):
            for variant in ("founding", "devops", "software"):
                for suffix in ("", "resume.pdf"):
                    with self.subTest(host=host, variant=variant, suffix=suffix):
                        self.assertEqual(resolve(f"https://{host}.zqsdev.com/cv/{variant}/{suffix}"), f"/cv/{suffix}")


if __name__ == "__main__":
    unittest.main()
