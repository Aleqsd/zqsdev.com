#!/usr/bin/env python3
"""Nightly live smoke test runner for https://www.zqsdev.com.

This script exercises the production website once per run, covering:
  • critical static assets served from Netlify
  • the `/api/data` payload that powers the terminal commands
  • dataset sanity checks (profile, skills, experience, projects, FAQ, testimonials)
  • a single `/api/ai` question to confirm the concierge is responding

Run with:
    python3 scripts/live_smoke_test.py
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import sys
import time
from pathlib import Path
from typing import Callable, List, Optional, Sequence, Tuple

import requests


@dataclasses.dataclass
class TestResult:
    name: str
    passed: bool
    detail: str
    duration: float


class StopOnFailure(Exception):
    """Raised internally when --fail-fast is active."""


class LiveSmokeTester:
    def __init__(
        self,
        base_url: str,
        ai_question: str,
        timeout: float,
        fail_fast: bool,
    ) -> None:
        normalized = base_url.rstrip("/")
        if not normalized.startswith("http://") and not normalized.startswith(
            "https://"
        ):
            raise ValueError(f"Base URL must include scheme (got {base_url!r})")
        self.base_url = normalized
        self.ai_question = ai_question
        self.timeout = timeout
        self.fail_fast = fail_fast
        self.session = requests.Session()
        self.session.headers.update(
            {
                "User-Agent": "zqsdev-live-smoke/1.0 (+https://www.zqsdev.com)",
                "Accept-Language": "en-US,en;q=0.9",
            }
        )
        self.results: List[TestResult] = []
        self.terminal_data: Optional[dict] = None
        self.expected_tests: int = 0

    def run(self) -> bool:
        tests: Sequence[Tuple[str, Callable[[], str]]] = (
            ("Homepage loads", self.test_homepage),
            ("Static assets respond", self.test_static_assets),
            ("Backend version endpoint", self.test_backend_version_endpoint),
            ("Terminal data endpoint", self.test_terminal_data_endpoint),
            ("Profile dataset", self.test_profile_dataset),
            ("Skills dataset", self.test_skills_dataset),
            ("Experience dataset", self.test_experience_dataset),
            ("Projects dataset", self.test_projects_dataset),
            ("Projects command icons", self.test_projects_command_icons),
            ("FAQ dataset", self.test_faq_dataset),
            ("Testimonials dataset", self.test_testimonials_dataset),
            ("AI projects tech detail", self.test_ai_projects_detail),
            ("AI concierge", self.test_ai_endpoint),
        )
        self.expected_tests = len(tests)
        try:
            for name, fn in tests:
                self._run_test(name, fn)
        except StopOnFailure:
            pass
        return self.all_passed

    def _run_test(self, name: str, fn: Callable[[], str]) -> None:
        start = time.perf_counter()
        passed = False
        detail = ""
        try:
            detail = fn()
            passed = True
        except AssertionError as exc:
            detail = f"assertion failed: {exc}"
        except requests.RequestException as exc:
            detail = f"request error: {exc}"
        except Exception as exc:  # noqa: BLE001 - we want a hard fail summary
            detail = f"unexpected error: {exc}"
        duration = time.perf_counter() - start
        self.results.append(
            TestResult(name=name, passed=passed, detail=detail, duration=duration)
        )
        if self.fail_fast and not passed:
            raise StopOnFailure

    def print_report(self) -> None:
        for result in self.results:
            status = "PASS" if result.passed else "FAIL"
            print(
                f"{status:>4}  {result.name:<24} {result.detail} ({result.duration:.2f}s)"
            )
        passed = sum(1 for r in self.results if r.passed)
        failed = len(self.results) - passed
        skipped = max(self.expected_tests - len(self.results), 0)
        if skipped:
            print(f"Totals: {passed} passed, {failed} failed, {skipped} skipped")
        else:
            print(f"Totals: {passed} passed, {failed} failed")

    def write_json(self, path: str) -> None:
        payload = [dataclasses.asdict(result) for result in self.results]
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2)

    @property
    def all_passed(self) -> bool:
        return all(result.passed for result in self.results)

    def _url(self, path: str) -> str:
        if path.startswith("http://") or path.startswith("https://"):
            return path
        path = path.lstrip("/")
        return f"{self.base_url}/{path}" if path else self.base_url

    def _head_or_get(self, path: str) -> requests.Response:
        url = self._url(path)
        response = self.session.head(url, timeout=self.timeout, allow_redirects=True)
        if response.status_code in (405, 501):
            response = self.session.get(url, timeout=self.timeout, allow_redirects=True)
        return response

    def _require_terminal_data(self) -> dict:
        if self.terminal_data is None:
            raise AssertionError("terminal data was not loaded before dataset checks")
        return self.terminal_data

    @property
    def any_skipped(self) -> bool:
        return len(self.results) < self.expected_tests

    def test_homepage(self) -> str:
        response = self.session.get(
            self.base_url,
            timeout=self.timeout,
            headers={"Accept": "text/html,application/xhtml+xml"},
        )
        status = response.status_code
        assert status == 200, f"unexpected status {status}"
        body = response.text
        markers = ("ZQSDev Terminal", "AI Mode", "zqs@dev:~$")
        missing = [marker for marker in markers if marker not in body]
        assert not missing, f"missing markers: {', '.join(missing)}"
        cache_control = response.headers.get("Cache-Control", "none")
        return f"status=200 size={len(body)} cache={cache_control}"

    def test_static_assets(self) -> str:
        assets: Sequence[Tuple[str, Tuple[str, ...]]] = (
            ("style.min.css", ("text/css",)),
            ("pkg/zqs_terminal.js", ("application/javascript", "text/javascript")),
            ("pkg/zqs_terminal_bg.wasm", ("application/wasm",)),
            ("images/zqsdev_gradient_logo.webp", ("image/webp",)),
        )
        detail_parts: List[str] = []
        for asset, expected_types in assets:
            response = self._head_or_get(asset)
            status = response.status_code
            assert status == 200, f"{asset} returned {status}"
            content_type = response.headers.get("Content-Type", "")
            assert any(
                content_type.startswith(prefix) for prefix in expected_types
            ), f"{asset} content-type {content_type!r} not in {expected_types}"
            detail_parts.append(f"{asset}:{status}/{content_type.split(';', 1)[0]}")
        return ", ".join(detail_parts)

    def test_terminal_data_endpoint(self) -> str:
        response = self.session.get(self._url("/api/data"), timeout=self.timeout)
        status = response.status_code
        if status == 200:
            cache_control = response.headers.get("Cache-Control", "missing")
            data = response.json()
            expected_keys = (
                "profile",
                "skills",
                "experiences",
                "education",
                "projects",
                "testimonials",
                "faq",
            )
            missing = [key for key in expected_keys if key not in data]
            assert not missing, f"missing keys: {missing}"
            self._validate_terminal_payload(data)
            self.terminal_data = data
            return (
                f"profile={data['profile']['name']} "
                f"skills={len(data['skills'])} "
                f"exp={len(data['experiences'])} "
                f"projects={len(data['projects'].get('projects', []))} "
                f"faq={len(data['faq'])} "
                f"cache={cache_control}"
            )

        static_payload = self._load_terminal_data_from_static()
        self._validate_terminal_payload(static_payload)
        self.terminal_data = static_payload
        return f"fallback=static status={status}"

    def _load_terminal_data_from_static(self) -> dict:
        manifest = {
            "profile": "data/profile.json",
            "skills": "data/skills.json",
            "experiences": "data/experience.json",
            "education": "data/education.json",
            "projects": "data/projects.json",
            "testimonials": "data/testimonials.json",
            "faq": "data/faq.json",
        }
        payload: dict = {}
        for key, path in manifest.items():
            response = self.session.get(self._url(path), timeout=self.timeout)
            status = response.status_code
            assert status == 200, f"{path} returned {status}"
            payload[key] = response.json()
        return payload

    @staticmethod
    def _validate_terminal_payload(data: dict) -> None:
        assert data["profile"].get("name"), "profile missing name"
        skills = data["skills"]
        experiences = data["experiences"]
        projects = data["projects"]
        faqs = data["faq"]
        testimonials = data["testimonials"]
        assert isinstance(skills, dict) and skills, "skills empty"
        assert isinstance(experiences, list) and experiences, "experiences empty"
        assert isinstance(projects, dict) and projects.get("projects"), "projects empty"
        assert isinstance(faqs, list) and faqs, "faq empty"
        assert isinstance(testimonials, list) and testimonials, "testimonials empty"

    def test_profile_dataset(self) -> str:
        data = self._require_terminal_data()
        profile = data["profile"]
        assert (
            profile.get("name") == "Alexandre DO-O ALMEIDA"
        ), "unexpected profile name"
        email = profile.get("email", "")
        assert "@" in email and "." in email, "invalid email"
        languages = profile.get("languages", [])
        assert isinstance(languages, list) and languages, "languages missing"
        links = profile.get("links", {})
        resume_url = links.get("resume_url")
        assert resume_url, "resume_url missing"
        resume_response = self._head_or_get(resume_url)
        assert (
            200 <= resume_response.status_code < 400
        ), f"resume_url returned {resume_response.status_code}"
        return f"email={email} resume={resume_response.status_code}"

    def test_skills_dataset(self) -> str:
        data = self._require_terminal_data()
        skills = data["skills"]
        assert isinstance(skills, dict) and skills, "skills dataset missing"
        empty = [category for category, values in skills.items() if not values]
        assert not empty, f"categories without entries: {empty}"
        contains_rust = any("Rust" in values for values in skills.values())
        assert contains_rust, "expected Rust in skills"
        return f"categories={len(skills)}"

    def test_experience_dataset(self) -> str:
        data = self._require_terminal_data()
        experiences = data["experiences"]
        assert (
            isinstance(experiences, list) and experiences
        ), "experiences dataset missing"
        first = experiences[0]
        assert first.get("company"), "first experience missing company"
        playstation_mentions = any(
            "PlayStation"
            in (exp.get("company", "") + " " + " ".join(exp.get("highlights", [])))
            for exp in experiences
        )
        assert playstation_mentions, "expected PlayStation mention"
        return f"records={len(experiences)} first_company={first.get('company')}"

    def test_projects_dataset(self) -> str:
        data = self._require_terminal_data()
        projects = data["projects"]
        main_projects = projects.get("projects", [])
        awards = projects.get("awards", [])
        assert (
            isinstance(main_projects, list) and main_projects
        ), "projects list missing"
        assert isinstance(awards, list) and awards, "awards list missing"
        linked = sum(1 for project in main_projects if project.get("link"))
        return f"projects={len(main_projects)} awards={len(awards)} linked={linked}"

    def test_projects_command_icons(self) -> str:
        data = self._require_terminal_data()
        projects = data["projects"].get("projects", [])
        assert projects, "projects dataset missing"
        webassembly_projects = [
            project
            for project in projects
            if any(
                isinstance(tech, str) and tech.strip().lower() == "webassembly"
                for tech in project.get("tech", [])
            )
        ]
        assert webassembly_projects, "no project lists WebAssembly in tech stack"

        icon_response = self.session.get(
            self._url("/icons/wasm-original.svg"), timeout=self.timeout
        )
        assert icon_response.status_code == 200, "wasm icon missing from static/icons"
        content_type = icon_response.headers.get("Content-Type", "").lower()
        assert content_type.startswith(
            "image/svg+xml"
        ), f"wasm icon is not an SVG asset (got {content_type})"
        assert (
            "<svg" in icon_response.text.lower()
        ), "wasm icon payload does not contain an SVG tag"

        return f"webassembly_projects={len(webassembly_projects)} icon=wasm-original.svg"

    def test_backend_version_endpoint(self) -> str:
        response = self.session.get(self._url("/api/version"), timeout=self.timeout)
        status = response.status_code
        assert status == 200, f"version endpoint returned {status}"
        payload = response.json()
        version = payload.get("version", "")
        commit = payload.get("commit", "unknown")
        assert version, "backend version missing in version payload"
        return f"backend_version={version} commit={commit}"

    def test_ai_projects_detail(self) -> str:
        payload = {
            "question": "Which technologies power the ZQSDev Terminal project?"
        }
        response = self.session.post(
            self._url("/api/ai"), json=payload, timeout=self.timeout
        )
        status = response.status_code
        assert status == 200, f"unexpected status {status}"
        data = response.json()
        assert data.get("ai_enabled") is True, "ai disabled for project detail test"
        answer = data.get("answer", "")
        required_terms = ("WebAssembly", "Rust", "RAG")
        missing = [term for term in required_terms if term.lower() not in answer.lower()]
        assert not missing, f"missing tech terms in answer: {missing}"
        contexts = data.get("context_chunks") or []
        projects_context = any(
            chunk.get("source") == "projects.json" for chunk in contexts
        )
        assert projects_context, "AI response missing projects.json context"
        return f"terms_present={','.join(required_terms)} contexts={len(contexts)}"

    def test_faq_dataset(self) -> str:
        data = self._require_terminal_data()
        faqs = data["faq"]
        assert isinstance(faqs, list) and len(faqs) >= 5, "faq dataset too small"
        structured = [faq for faq in faqs if faq.get("question") and faq.get("answer")]
        assert len(structured) == len(faqs), "faq entries with missing fields"
        return f"entries={len(faqs)}"

    def test_testimonials_dataset(self) -> str:
        data = self._require_terminal_data()
        testimonials = data["testimonials"]
        assert (
            isinstance(testimonials, list) and testimonials
        ), "testimonials dataset missing"
        authors = [item.get("author") for item in testimonials if item.get("author")]
        assert len(authors) == len(testimonials), "testimonial without author"
        return f"entries={len(testimonials)}"

    def test_ai_endpoint(self) -> str:
        payload = {"question": self.ai_question}
        response = self.session.post(
            self._url("/api/ai"), json=payload, timeout=self.timeout
        )
        status = response.status_code
        assert status == 200, f"unexpected status {status}"
        data = response.json()
        assert data.get("ai_enabled") is True, f"ai disabled: {data.get('reason')}"
        answer = data.get("answer", "")
        assert len(answer.strip()) >= 32, "answer too short"
        contexts = data.get("context_chunks") or []
        assert contexts, "context_chunks missing or empty"
        first = contexts[0]
        for key in ("id", "source", "topic"):
            assert key in first, f"context chunk missing {key}"
        model = data.get("model") or "unknown"
        return (
            f"model={model} answer_len={len(answer.strip())} "
            f"contexts={len(contexts)} first_chunk={first['id']}"
        )


def parse_args(argv: Optional[Sequence[str]] = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run live smoke tests against www.zqsdev.com"
    )
    parser.add_argument(
        "--base-url",
        default="https://www.zqsdev.com",
        help="Base URL to target (default: %(default)s)",
    )
    parser.add_argument(
        "--ai-question",
        default="Could you summarise Alexandre's recent work in one sentence?",
        help="Question to send to the AI concierge (default: %(default)s)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=12.0,
        help="Per-request timeout in seconds (default: %(default)s)",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop after the first failing test case",
    )
    parser.add_argument(
        "--json-output",
        help="Optional path to write a JSON report",
    )
    parser.add_argument(
        "--no-pushover",
        action="store_true",
        help="Skip Pushover notification even if credentials are configured",
    )
    return parser.parse_args(argv)


def load_env_files() -> None:
    for candidate in (".env.local", ".env"):
        path = Path(candidate)
        if not path.is_file():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line[len("export ") :]
            if "=" not in line:
                continue
            key, value = line.split("=", 1)
            key = key.strip()
            value = value.strip().strip('"').strip("'")
            os.environ.setdefault(key, value)


def send_pushover_notification(tester: LiveSmokeTester) -> bool:
    token = os.environ.get("PUSHOVER_API_TOKEN")
    user = os.environ.get("PUSHOVER_USER_KEY")
    if not token or not user:
        return True

    total = len(tester.results)
    passed = sum(1 for result in tester.results if result.passed)
    failed = total - passed
    skipped = max(tester.expected_tests - total, 0)
    elapsed = sum(result.duration for result in tester.results)
    status_emoji = "❌" if failed else "⚠️"
    summary_suffix = "failed" if failed else f"completed with {skipped} skipped"
    headline = f"{status_emoji} ZQSDev live smoke: {passed}/{tester.expected_tests} checks {summary_suffix}"
    lines = [headline, f"Duration: {elapsed:.1f}s"]

    if skipped:
        lines.append("Run stopped early; remaining checks did not execute.")
    if failed:
        failing = [result for result in tester.results if not result.passed]
        for idx, result in enumerate(failing, start=1):
            lines.append(f"{idx}. {result.name}: {result.detail}")
            if idx == 3:
                break
    else:
        ai_summary = next(
            (r.detail for r in tester.results if r.name == "AI concierge"), None
        )
        if ai_summary:
            lines.append(f"AI: {ai_summary}")

    message = "\n".join(lines)
    payload = {
        "token": token,
        "user": user,
        "message": message,
        "title": "ZQSDev nightly smoke",
        "priority": 1 if failed else 0,
        "url": tester.base_url,
        "url_title": "Open www.zqsdev.com",
    }
    try:
        response = requests.post(
            "https://api.pushover.net/1/messages.json",
            data=payload,
            timeout=10,
        )
        if response.status_code != 200:
            raise RuntimeError(f"status {response.status_code}: {response.text}")
        return True
    except Exception as exc:  # noqa: BLE001
        print(f"Pushover notification failed: {exc}", file=sys.stderr)
        return False


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = parse_args(argv)
    load_env_files()
    tester = LiveSmokeTester(
        base_url=args.base_url,
        ai_question=args.ai_question,
        timeout=args.timeout,
        fail_fast=args.fail_fast,
    )
    tester.run()
    tester.print_report()
    if args.json_output:
        tester.write_json(args.json_output)
    notification_ok = True
    should_notify = (not tester.all_passed) or tester.any_skipped
    if not args.no_pushover and should_notify:
        notification_ok = send_pushover_notification(tester)
    if tester.all_passed and notification_ok:
        return 0
    if not tester.all_passed:
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main())
