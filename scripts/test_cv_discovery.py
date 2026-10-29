"""CV discovery and canonical routing contracts; no network access required."""

from fnmatch import fnmatchcase
from pathlib import Path
import re
import tomllib
import unittest
from urllib.parse import urlparse
from urllib.robotparser import RobotFileParser
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
CV = ROOT / "static" / "cv"
ORIGIN = "https://cv.zqsdev.com"
CONFIG = tomllib.loads((ROOT / "netlify.toml").read_text(encoding="utf-8"))


def resolve(url):
    """Model the site's ordered exact/splat rules, stopping at a rewrite.

    Provider behavior (headers after rewrites, real 404s, query retention) still
    needs a deployed HTTP check; this helper protects rule order and targets.
    """
    visited = set()
    redirects = []
    while url not in visited:
        visited.add(url)
        for rule in CONFIG["redirects"]:
            pattern = rule["from"]
            candidate = url if pattern.startswith("https://") else urlparse(url).path
            match = re.fullmatch(re.escape(pattern).replace(r"\*", "(.*)"), candidate)
            if not match:
                continue
            target = rule["to"].replace(":splat", match.group(1) if match.groups() else "")
            if rule["status"] == 200:
                return url, target, redirects
            redirects.append((rule["status"], target))
            url = target
            break
        else:
            raise AssertionError(f"No route for {url}")
    raise AssertionError(f"Redirect loop at {url}")


def headers_for(path):
    result = {}
    for rule in CONFIG["headers"]:
        if fnmatchcase(path, rule["for"]):
            result.update(rule["values"])
    return result


class CvDiscoveryTests(unittest.TestCase):
    def test_robots_allows_search_and_ai_crawlers_without_changing_training_policy(self):
        text = (CV / "robots.txt").read_text(encoding="utf-8")
        parser = RobotFileParser()
        parser.parse(text.splitlines())
        self.assertEqual(parser.site_maps(), [f"{ORIGIN}/sitemap.xml"])
        for crawler in ("Googlebot", "Bingbot", "OAI-SearchBot", "Claude-SearchBot", "PerplexityBot", "*"):
            for path in ("/", "/fr.html", "/resume.pdf", "/resume-fr.pdf"):
                self.assertTrue(parser.can_fetch(crawler, ORIGIN + path), (crawler, path))
        self.assertEqual(re.findall(r"^User-agent:\s*(.+)$", text, re.M), ["*"])

    def test_sitemap_contains_only_canonical_language_pages_and_reciprocal_alternates(self):
        root = ET.parse(CV / "sitemap.xml").getroot()
        ns = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9", "x": "http://www.w3.org/1999/xhtml"}
        self.assertEqual(root.tag, f"{{{ns['s']}}}urlset")
        urls = root.findall("s:url", ns)
        self.assertEqual({url.findtext("s:loc", namespaces=ns) for url in urls}, {ORIGIN + "/", ORIGIN + "/fr.html"})
        self.assertEqual(len(urls), 2)
        expected = {"en": ORIGIN + "/", "fr": ORIGIN + "/fr.html", "x-default": ORIGIN + "/"}
        for url in urls:
            alternates = {link.attrib["hreflang"]: link.attrib["href"] for link in url.findall("x:link", ns)}
            self.assertEqual(alternates, expected)
            self.assertTrue(all(link.attrib["rel"] == "alternate" for link in url.findall("x:link", ns)))
            # Avoid fabricated freshness timestamps, priorities, or change rates.
            self.assertIsNone(url.find("s:lastmod", ns))
            self.assertIsNone(url.find("s:priority", ns))
            self.assertIsNone(url.find("s:changefreq", ns))

    def test_llms_summary_points_to_real_sources_without_prompt_or_policy_instructions(self):
        text = (CV / "llms.txt").read_text(encoding="utf-8")
        self.assertTrue(text.startswith("# Alexandre DO-O ALMEIDA\n"))
        for url in (ORIGIN + "/", ORIGIN + "/fr.html", ORIGIN + "/resume.pdf", ORIGIN + "/resume-fr.pdf"):
            self.assertIn(f"]({url})", text)
        self.assertIn("self-reported", text)
        self.assertIn("optional", text)
        self.assertIn("does not grant or restrict crawler access", text)
        for unsupported in ("guaranteed", "ignore previous", "rank this", "knowledge cutoff"):
            self.assertNotIn(unsupported, text.lower())
        self.assertLess(len(text.encode("utf-8")), 4096)

    def test_aliases_redirect_to_the_two_canonical_language_urls(self):
        cases = {
            ORIGIN + "/index.html": ORIGIN + "/",
            ORIGIN + "/fr": ORIGIN + "/fr.html",
            ORIGIN + "/fr/": ORIGIN + "/fr.html",
            "https://zqsdev.com/cv": ORIGIN + "/",
            "https://zqsdev.com/cv/": ORIGIN + "/",
            "https://www.zqsdev.com/cv/": ORIGIN + "/",
            "https://www.zqsdev.com/cv/fr.html": ORIGIN + "/fr.html",
            "https://zqsdev.com/cv/index.html": ORIGIN + "/",
            "https://zqsdev.com/cv/fr": ORIGIN + "/fr.html",
        }
        for source, expected in cases.items():
            with self.subTest(source=source):
                final, target, redirects = resolve(source)
                self.assertEqual(final, expected)
                self.assertTrue(redirects)
                self.assertTrue(all(status == 301 for status, _ in redirects))
                self.assertIn(target, ("/cv/", "/cv/fr.html"))

    def test_discovery_assets_and_pdfs_use_the_cv_rewrite(self):
        for name in ("robots.txt", "sitemap.xml", "llms.txt", "resume.pdf", "resume-fr.pdf", "resume.css", "resume.js"):
            with self.subTest(name=name):
                final, target, redirects = resolve(f"{ORIGIN}/{name}")
                self.assertEqual(final, f"{ORIGIN}/{name}")
                self.assertEqual(target, f"/cv/{name}")
                self.assertEqual(redirects, [])
                self.assertTrue((ROOT / "static" / target.lstrip("/")).is_file())
        # Unknown CV URLs are rewritten to their missing files, not the SPA.
        self.assertEqual(resolve(ORIGIN + "/missing-cv-page")[1], "/cv/missing-cv-page")
        self.assertFalse((CV / "missing-cv-page").exists())

    def test_main_site_api_and_scheduler_routes_are_preserved(self):
        for host in ("zqsdev.com", "www.zqsdev.com"):
            for path in ("/", "/contact", "/version"):
                _, target, redirects = resolve(f"https://{host}{path}")
                self.assertEqual(target, "static/index.html")
                self.assertEqual(redirects, [])
            self.assertEqual(resolve(f"https://{host}/api/ai")[1], "https://api.zqsdev.com/api/ai")
            for pdf in ("resume.pdf", "resume-fr.pdf"):
                final, target, redirects = resolve(f"https://{host}/{pdf}")
                self.assertEqual(final, f"{ORIGIN}/{pdf}")
                self.assertEqual(target, f"/cv/{pdf}")
                self.assertTrue(redirects)
        scheduler = next(rule for rule in CONFIG["redirects"] if rule["from"] == "https://calendly.zqsdev.com/*")
        self.assertEqual(scheduler["to"], "https://calendly.com/alexandre-zqsdev/30min")

    def test_pdf_canonical_headers_and_discovery_revalidation(self):
        for filename, canonical in (("resume.pdf", ORIGIN + "/"), ("resume-fr.pdf", ORIGIN + "/fr.html")):
            # Production subdomain requests match the public path before
            # rewriting. Direct preview requests use the /cv backing path.
            for prefix in ("/", "/cv/"):
                headers = headers_for(prefix + filename)
                self.assertEqual(headers["Link"], f'<{canonical}>; rel="canonical"')
                self.assertNotIn("noindex", headers.get("X-Robots-Tag", ""))
        for filename in ("index.html", "fr.html", "robots.txt", "sitemap.xml", "llms.txt"):
            headers = headers_for("/cv/" + filename)
            self.assertEqual(headers["Cache-Control"], "public, max-age=0, must-revalidate")
        for name, mime in (("robots.txt", "text/plain"), ("llms.txt", "text/plain"), ("sitemap.xml", "application/xml")):
            self.assertEqual(headers_for("/cv/" + name)["Content-Type"], mime + "; charset=utf-8")
        self.assertIn("immutable", headers_for("/cv/assets/portrait-slate-8526a4f2bf20.webp")["Cache-Control"])


if __name__ == "__main__":
    unittest.main()
