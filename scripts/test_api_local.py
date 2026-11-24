#!/usr/bin/env python3
"""Local end-to-end test of the zqsdev-api Worker behind the zqsdev site Worker.

By default it starts everything itself and stops it at the end:
  1. scripts/openai_stub.py (no OpenAI key, nothing paid);
  2. `wrangler dev --local` of api/, built with ZQS_FAULT_INJECTION=1 so that a request header can
     fail a ledger transaction after N writes, with its Durable Object SQLite in a temporary
     directory and OPENAI_BASE_URL pointing at the stub;
  3. `wrangler dev --local` of the site, whose "API" service binding connects to that session.
It then runs every check through the site Worker, stops the API session to check the 502 of a
disconnected binding, and stops the rest. Only 127.0.0.1 is contacted: no Cloudflare account, no
remote binding, no secret (a throwaway admin token is generated in the temporary directory).

    python3 scripts/test_api_local.py
    python3 scripts/test_api_local.py --site-port 8732 --api-port 8731 --stub-port 8733

With --no-start it tests servers started by hand (README, "Local development"); the admin token
then comes from the ZQS_ADMIN_TOKEN environment variable, the fault checks need an API built with
ZQS_FAULT_INJECTION=1, and the disconnected-binding check is skipped.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass, field
from ipaddress import ip_address
from pathlib import Path
from typing import Callable, Optional

ROOT = Path(__file__).resolve().parents[1]
DATA_FILES = {
    "profile": "profile.json",
    "skills": "skills.json",
    "experiences": "experience.json",
    "education": "education.json",
    "projects": "projects.json",
    "testimonials": "testimonials.json",
    "faqs": "faq.json",
}
FAIL_AFTER_HEADER = "X-Zqs-Fail-After"
# Never handed to the local wrangler sessions.
SECRET_ENV = ("OPENAI_API_KEY", "ADMIN_TOKEN", "ZQS_ADMIN_TOKEN", "CLOUDFLARE_API_TOKEN",
              "CLOUDFLARE_ACCOUNT_ID", "CF_API_TOKEN")
HSTS = "max-age=31536000"
USER_AGENT = "zqsdev-local-test/1.0"
FALLBACK_ANSWER = (
    "AI is temporarily unavailable. Classic commands are ready: about, experience, projects, "
    "testimonials, resume. / IA temporairement indisponible : les commandes classiques restent "
    "disponibles."
)


def load_jsonc(path: Path):
    """Parse a wrangler JSONC file: drop // and /* */ comments outside strings, then trailing commas."""
    text = path.read_text(encoding="utf-8")
    out, i, in_string = [], 0, False
    while i < len(text):
        char = text[i]
        if in_string:
            out.append(char)
            if char == "\\":
                out.append(text[i + 1])
                i += 1
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
            out.append(char)
        elif text.startswith("//", i):
            i = text.find("\n", i)
            if i < 0:
                break
            continue
        elif text.startswith("/*", i):
            i = text.index("*/", i) + 2
            continue
        else:
            out.append(char)
        i += 1
    return json.loads(re.sub(r",(\s*[}\]])", r"\1", "".join(out)))


# ---------------------------------------------------------------- HTTP

@dataclass
class Reply:
    status: int
    headers: dict
    body: bytes
    seconds: float

    def header(self, name: str) -> Optional[str]:
        for key, value in self.headers.items():
            if key.lower() == name.lower():
                return value
        return None

    def json(self):
        return json.loads(self.body)

    @property
    def text(self) -> str:
        return self.body.decode("utf-8", errors="replace")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Report 3xx responses instead of following them (a redirect must never leave 127.0.0.1)."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect)


def http(method: str, url: str, body=None, headers: Optional[dict] = None, timeout: float = 30.0,
         raw: Optional[bytes] = None, content_type: Optional[str] = "application/json") -> Reply:
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode("utf-8"))
    request_headers = {"User-Agent": USER_AGENT}
    if data is not None and content_type:
        request_headers["Content-Type"] = content_type
    request_headers.update(headers or {})
    request = urllib.request.Request(url, data=data, headers=request_headers, method=method)
    started = time.monotonic()
    try:
        with OPENER.open(request, timeout=timeout) as response:
            return Reply(response.status, dict(response.headers), response.read(), time.monotonic() - started)
    except urllib.error.HTTPError as error:
        return Reply(error.code, dict(error.headers), error.read(), time.monotonic() - started)


def port_in_use(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) == 0


# ---------------------------------------------------------------- processes

class Process:
    def __init__(self, name: str, args: list, cwd: Path, env: dict, log_path: Path) -> None:
        self.name = name
        self.log_path = log_path
        self.log = open(log_path, "wb")
        kwargs = {}
        if os.name == "nt":
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        else:
            kwargs["start_new_session"] = True
        self.proc = subprocess.Popen(args, cwd=str(cwd), env=env, stdout=self.log,
                                     stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, **kwargs)

    def alive(self) -> bool:
        return self.proc.poll() is None

    def stop(self) -> None:
        if self.alive():
            if os.name == "nt":
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(self.proc.pid)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
            else:
                try:
                    os.killpg(self.proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            try:
                self.proc.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.log.close()

    def tail(self, lines: int = 25) -> str:
        try:
            text = self.log_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return ""
        return "\n".join(text.splitlines()[-lines:])


def wait_for(description: str, probe: Callable[[], bool], timeout: float, process: Optional[Process] = None) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process is not None and not process.alive():
            raise RuntimeError(f"{process.name} exited before {description}:\n{process.tail()}")
        try:
            if probe():
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(1.0)
    tail = f":\n{process.tail()}" if process is not None else ""
    raise RuntimeError(f"timed out waiting for {description}{tail}")


# ---------------------------------------------------------------- checks

class CheckFailed(AssertionError):
    pass


def expect(condition, message: str) -> None:
    if not condition:
        raise CheckFailed(message)


@dataclass
class Result:
    name: str
    status: str  # PASS, FAIL, SKIP
    detail: str
    seconds: float


@dataclass
class Suite:
    site: str
    api: str
    stub: str
    token: str
    managed: bool
    stop_api: Optional[Callable[[], None]] = None
    results: list = field(default_factory=list)
    _ip_counter: int = 0

    # -- helpers

    def fake_ip(self) -> str:
        """A distinct documentation address per scenario, so the per-IP windows never interfere.
        Local wrangler dev forwards a client-sent CF-Connecting-IP; the Cloudflare edge overwrites it."""
        self._ip_counter += 1
        return f"198.51.100.{self._ip_counter}"

    def admin(self, op: str, body: Optional[dict] = None, token: Optional[str] = None, base: Optional[str] = None) -> Reply:
        payload = dict(body or {})
        payload["token"] = self.token if token is None else token
        return http("POST", f"{base or self.site}/api/admin/{op}", payload, timeout=20)

    def stats(self) -> dict:
        reply = self.admin("stats")
        expect(reply.status == 200, f"admin stats returned {reply.status}")
        return reply.json()

    def export(self, table: str, after: str = "0", limit: Optional[int] = None) -> list:
        lines, cursor, pages = [], after, 0
        while True:
            body = {"table": table, "after": cursor}
            if limit is not None:
                body["limit"] = limit
            reply = self.admin("export", body)
            expect(reply.status == 200, f"export {table} returned {reply.status}: {reply.text[:200]}")
            expect((reply.header("Content-Type") or "").startswith("application/x-ndjson"),
                   f"export content type {reply.header('Content-Type')}")
            lines.extend(json.loads(line) for line in reply.text.splitlines() if line.strip())
            pages += 1
            cursor = reply.header("X-Next-After")
            if not cursor:
                return lines
            expect(pages < 10_000, "export never ends")

    def stub_stats(self) -> dict:
        return http("GET", f"{self.stub}/__stats", timeout=5).json()

    def ask(self, question: str, ip: Optional[str] = None, headers: Optional[dict] = None, timeout: float = 30.0,
            history: Optional[list] = None) -> Reply:
        request_headers = dict(headers or {})
        if ip:
            request_headers["CF-Connecting-IP"] = ip
        body = {"question": question, "history": history or []}
        return http("POST", f"{self.site}/api/ai", body, headers=request_headers, timeout=timeout)

    def run(self, name: str, check: Callable[[], str]) -> None:
        started = time.monotonic()
        try:
            detail = check()
            status = "SKIP" if detail.startswith("skipped") else "PASS"
        except CheckFailed as error:
            status, detail = "FAIL", str(error)
        except Exception as error:  # noqa: BLE001 - report every failure
            status, detail = "FAIL", f"{type(error).__name__}: {error}"
        result = Result(name, status, detail, time.monotonic() - started)
        self.results.append(result)
        print(f"{status}  {name:<34} {detail} ({result.seconds:.1f}s)", flush=True)

    # -- static routes

    def check_data(self) -> str:
        via_site = http("GET", f"{self.site}/api/data")
        direct = http("GET", f"{self.api}/api/data")
        expect(via_site.status == 200, f"status {via_site.status}")
        expect(via_site.header("Content-Type") == "application/json", f"content type {via_site.header('Content-Type')}")
        expect(via_site.header("Cache-Control") == "public, max-age=60, must-revalidate",
               f"cache control {via_site.header('Cache-Control')}")
        expect(via_site.header("Strict-Transport-Security") == HSTS, "HSTS missing through the site")
        expect(via_site.body == direct.body, "the site changed the /api/data bytes")
        order = json.loads(via_site.body, object_pairs_hook=lambda pairs: [key for key, _ in pairs])
        expect(order == sorted(order), f"top-level keys not sorted: {order}")
        expected = {key: json.loads((ROOT / "static" / "data" / name).read_text(encoding="utf-8"))
                    for key, name in DATA_FILES.items()}
        expected["faq"] = expected["faqs"]
        expect(via_site.json() == expected, "/api/data differs from static/data/*.json")
        head = http("HEAD", f"{self.site}/api/data")
        expect(head.status == 200 and head.body == b"", f"HEAD returned {head.status} with {len(head.body)} bytes")
        expect(head.header("Cache-Control") == "public, max-age=60, must-revalidate", "HEAD lost Cache-Control")
        return f"{len(via_site.body)} bytes, keys sorted, faq alias, HEAD empty"

    def check_version(self) -> str:
        reply = http("GET", f"{self.site}/api/version")
        expect(reply.status == 200, f"status {reply.status}")
        expect(reply.header("Cache-Control") is None, "unexpected Cache-Control")
        data = reply.json()
        keys = list(json.loads(reply.body, object_pairs_hook=lambda pairs: [key for key, _ in pairs]))
        expect(keys == ["ai_configured", "commit", "knowledge_mode", "knowledge_updated", "model", "version"],
               f"keys {keys}")
        version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
        model = load_jsonc(ROOT / "api" / "wrangler.jsonc")["vars"]["OPENAI_MODEL"]
        updated = json.loads((ROOT / "static" / "data" / "profile.json").read_text(encoding="utf-8"))["updated_at"]
        expect(data["version"] == version, f"version {data['version']} != {version}")
        expect(data["model"] == model, f"model {data['model']} != {model}")
        expect(data["ai_configured"] is True, "ai_configured is false")
        expect(data["knowledge_mode"] == "full_context", "knowledge_mode")
        expect(data["knowledge_updated"] == updated, f"knowledge_updated {data['knowledge_updated']} != {updated}")
        expect(data["commit"] == "unknown" or re.fullmatch(r"[0-9a-f]{40}", data["commit"]), f"commit {data['commit']}")
        return f"v{data['version']} {data['model']} commit {data['commit'][:12]}"

    def check_health(self) -> str:
        reply = http("GET", f"{self.site}/api/health")
        expect(reply.status == 200, f"status {reply.status}: {reply.text}")
        expect(reply.text == '{"ok":true,"ai_configured":true,"ledger":"ok","storage":"ok"}', reply.text)
        expect(reply.header("Cache-Control") == "no-store", "Cache-Control is not no-store")
        return reply.text

    def check_methods_and_paths(self) -> str:
        cases = [
            ("GET", "/api/ai", 405, "POST"),
            ("OPTIONS", "/api/ai", 405, "POST"),
            ("GET", "/api/log/command", 405, "POST"),
            ("POST", "/api/data", 405, "GET,HEAD"),
            ("OPTIONS", "/api/version", 405, "GET,HEAD"),
            ("DELETE", "/api/health", 405, "GET,HEAD"),
        ]
        for method, path, status, allow in cases:
            reply = http(method, f"{self.site}{path}", raw=b"" if method in ("POST", "DELETE") else None)
            expect(reply.status == status, f"{method} {path}: {reply.status}")
            expect(reply.header("Allow") == allow, f"{method} {path}: Allow {reply.header('Allow')}")
            expect(reply.body == b"", f"{method} {path}: body not empty")
            expect(reply.header("Strict-Transport-Security") == HSTS, f"{method} {path}: HSTS missing")
        query = http("GET", f"{self.site}/api/version?debug=1")
        expect(query.status == 200, f"the query string must be ignored: {query.status}")
        for method, path in (("GET", "/api/unknown"), ("GET", "/api/data/"), ("POST", "/api/unknown"),
                             ("POST", "/api/admin/stats")):
            reply = http(method, f"{self.site}{path}", raw=b"{}" if method == "POST" else None)
            expect(reply.status == 404, f"{method} {path}: {reply.status}")
            expect(reply.header("Cache-Control") == "no-store", f"{method} {path}: Cache-Control")
            expect(reply.body == b"", f"{method} {path}: body not empty")
        bare = http("GET", f"{self.site}/api")
        expect(bare.status == 404 and b"Page not found" in bare.body, f"bare /api: {bare.status}")
        www = http("GET", f"{self.site}/api/version", headers={"x-test-host": "www.zqsdev.com"})
        expect(www.status == 301 and www.header("Location") == "https://zqsdev.com/api/version",
               f"www: {www.status} {www.header('Location')}")
        return "405 + Allow, 404 no-store, query ignored, bare /api and www rules unchanged"

    # -- AI

    def check_ai_answer_and_visitor_ip(self) -> str:
        before = self.stats()
        stub_before = self.stub_stats()["requests"]
        marker = uuid.uuid4().hex[:12]
        reply = self.ask(f"Which technologies power the ZQSDev Terminal project? [{marker}]")
        expect(reply.status == 200, f"status {reply.status}: {reply.text[:300]}")
        data = reply.json()
        expect(list(data) == ["answer", "ai_enabled", "reason", "model", "sources", "knowledge_updated"],
               f"field order {list(data)}")
        expect(data["ai_enabled"] is True and data["reason"] is None, f"fallback: {data}")
        expect(data["sources"] == ["projects"], f"sources {data['sources']}")
        expect(data["model"] == "gpt-6-luna", f"model {data['model']}")
        expect(reply.header("Content-Type") == "application/json", "content type")
        stub = self.stub_stats()
        expect(stub["requests"] == stub_before + 1, "expected exactly one provider call")
        expect(stub["with_bearer"] == stub["requests"], "a provider call lacked the bearer key")
        expect(stub.get("foreign_keys", 0) == 0, "the API sent a key other than the local placeholder")
        lines = self.export("questions", after=str(before["next_seq"] - 1))
        question = [line for line in lines if line["entry_type"] == "ai_question" and marker in (line["question"] or "")]
        expect(len(question) == 1, f"question row not exported: {lines}")
        ip = question[0]["ip"]
        expect(ip and ip != "unknown", f"stored ip {ip!r}")
        ip_address(ip)
        usage = [line for line in lines if line["entry_type"] == "ai_usage" and line["question_id"] == question[0]["question_id"]]
        expect(len(usage) == 1 and usage[0]["cost_usd"] > 0 and usage[0]["usage"]["prompt_tokens"] > 0,
               f"usage line {usage}")
        after = self.stats()
        expect(after["inflight"] == [], f"inflight {after['inflight']}")
        expect(after["unknown_ip_questions_24h"] == 0, "unknown_ip_questions_24h is not 0")
        return f"answered, 1 provider call, settled at ${usage[0]['cost_usd']}, visitor ip {ip} through the binding"

    def check_ai_validation(self) -> str:
        stub_before = self.stub_stats()["requests"]
        cases = [
            ("wrong content type", dict(raw=b'{"question":"hi"}', content_type="text/plain")),
            ("unknown field", dict(body={"question": "hi", "extra": 1})),
            ("odd history", dict(body={"question": "hi", "history": [{"role": "user", "content": "x"}]})),
            ("system role", dict(body={"question": "hi", "history": [{"role": "system", "content": "x"},
                                                                     {"role": "assistant", "content": "y"}]})),
            ("1,201 characters", dict(body={"question": "é" * 1201})),
            ("blank question", dict(body={"question": "   "})),
            ("null history", dict(body={"question": "hi", "history": None})),
            ("over 64 KiB", dict(raw=b'{"question":"' + b"x" * 65_536 + b'"}')),
        ]
        for label, kwargs in cases:
            reply = http("POST", f"{self.site}/api/ai", headers={"CF-Connecting-IP": self.fake_ip()}, **kwargs)
            expect(reply.status == 400, f"{label}: {reply.status}")
            data = reply.json()
            expect(data["reason"] == "invalid_request" and data["answer"] == FALLBACK_ANSWER, f"{label}: {data}")
        expect(self.stub_stats()["requests"] == stub_before, "an invalid request reached the provider")
        trailing = http("POST", f"{self.site}/api/ai", raw=b'{"question":"Micro Mages?"} trailing',
                        headers={"CF-Connecting-IP": self.fake_ip()})
        expect(trailing.status == 200, f"trailing bytes: {trailing.status}")
        return f"{len(cases)} invalid requests -> 400 invalid_request without a provider call; trailing bytes accepted"

    def check_provider_failures(self) -> str:
        before = self.stats()
        outcomes = {}
        reservation = bad_cost = None
        for magic, error in (("__429__", "http_429"), ("__badsource__", "unknown_source"), ("__big__", "too_large"),
                             ("__redirect__", "http_307")):
            reply = self.ask(f"{magic} {uuid.uuid4().hex[:8]}", ip=self.fake_ip())
            expect(reply.status == 503, f"{magic}: {reply.status}")
            expect(reply.json()["reason"] == "provider_unavailable", f"{magic}: {reply.text}")
            outcomes[magic] = error
        lines = self.export("questions", after=str(before["next_seq"] - 1))
        failures = {line["question_id"]: line for line in lines if line["entry_type"] == "ai_failure"}
        questions = [line for line in lines if line["entry_type"] == "ai_question"]
        for line in questions:
            magic = next((m for m in outcomes if m in (line["question"] or "")), None)
            if magic is None:
                continue
            failure = failures.get(line["question_id"])
            expect(failure is not None, f"{magic}: no ai_failure line")
            expect(failure["reason"] == "provider_unavailable", f"{magic}: {failure}")
            expect(failure["error"] == outcomes[magic], f"{magic}: error {failure['error']}")
            if magic == "__429__":
                reservation = failure["cost_usd"]
            if magic == "__badsource__":
                expect(failure["usage"] and failure["usage"]["prompt_tokens"] > 0, "badsource: usage not kept")
                bad_cost = failure["cost_usd"]
        expect(reservation is not None and reservation > 0, "429: reservation missing")
        expect(bad_cost is not None and 0 < bad_cost < reservation, f"badsource charged {bad_cost}, reservation {reservation}")
        expect(self.stats()["inflight"] == [], "inflight not empty")
        # A 307 from the provider is an answer, never a hop: the bearer key stays with the stub.
        followed = self.stub_stats()["redirect_followed"]
        expect(followed == 0, f"the API followed the provider's redirect {followed} time(s)")
        return (f"429 / unknown source / 100 KB reply / 307 -> 503 provider_unavailable; errors stored; "
                f"redirect not followed; bad source charged ${bad_cost} instead of the ${reservation} reservation")

    def check_slots_timeout_and_disconnect(self) -> str:
        before = self.stats()
        marker = uuid.uuid4().hex[:8]
        results = {}

        def slow(label: str, timeout: float) -> None:
            try:
                results[label] = self.ask(f"__slow__ {label} {marker}", ip=self.fake_ip(), timeout=timeout)
            except (TimeoutError, socket.timeout, urllib.error.URLError) as error:
                results[label] = error

        waiting = threading.Thread(target=slow, args=("waits", 30.0))
        leaving = threading.Thread(target=slow, args=("leaves", 1.0))
        waiting.start()
        leaving.start()
        time.sleep(2.0)
        busy = self.ask(f"third call while both slots are leased {marker}", ip=self.fake_ip())
        expect(busy.status == 429 and busy.json()["reason"] == "busy", f"third call: {busy.status} {busy.text[:200]}")
        waiting.join(40)
        leaving.join(5)
        waited = results.get("waits")
        expect(isinstance(waited, Reply), f"waiting client: {waited!r}")
        expect(waited.status == 503 and waited.json()["reason"] == "provider_unavailable",
               f"slow call: {waited.status} {waited.text[:200]}")
        expect(14.0 <= waited.seconds <= 17.5, f"slow call answered after {waited.seconds:.1f}s")
        expect(not isinstance(results.get("leaves"), Reply), "the 1 s client did not time out")
        # The disconnected call must still be settled (site waitUntil), not left in flight.
        deadline = time.monotonic() + 25
        while True:
            lines = self.export("questions", after=str(before["next_seq"] - 1))
            ids = {line["question_id"]: line["question"] for line in lines if line["entry_type"] == "ai_question"}
            failures = [line for line in lines if line["entry_type"] == "ai_failure" and marker in (ids.get(line["question_id"]) or "")]
            if len(failures) == 2 or time.monotonic() > deadline:
                break
            time.sleep(1.0)
        expect(len(failures) == 2, f"settled slow calls: {failures}")
        for failure in failures:
            expect(failure["reason"] == "provider_unavailable" and failure["error"] == "timeout", f"{failure}")
        expect(self.stats()["inflight"] == [], "inflight not empty")
        return (f"2 slots leased -> busy; 15 s provider cap -> 503 after {waited.seconds:.1f}s; "
                "the call whose client left after 1 s was still settled")

    def check_request_limit(self) -> str:
        ip = self.fake_ip()
        statuses = []
        started = time.monotonic()
        # Up to 4 per second, 8 per minute: denials are not recorded, so keep asking until 8 are in.
        while sum(status == 200 for status, _ in statuses) < 8 and len(statuses) < 40:
            reply = self.ask(f"limit {len(statuses)}", ip=ip)
            statuses.append((reply.status, reply.json()["reason"]))
        expect(time.monotonic() - started < 50, "too slow to test the minute window")
        expect(statuses[:4] == [(200, None)] * 4, f"first calls {statuses[:4]}")
        denied = {reason for status, reason in statuses if status != 200}
        expect(denied <= {"request_limit"}, f"unexpected denials {denied}")
        ninth = self.ask("limit after 8", ip=ip)
        expect((ninth.status, ninth.json()["reason"]) == (429, "request_limit"), f"9th admission: {ninth.status}")
        return f"8 admitted ({len(statuses) - 8} refused by the 4/s window), then 429 request_limit for the minute"

    # -- transactionSync rollback (fault injection)

    def check_admission_rollback(self) -> str:
        before = self.stats()
        stub_before = self.stub_stats()["requests"]
        ip = self.fake_ip()
        reply = self.ask(f"rollback {uuid.uuid4().hex[:8]}", ip=ip, headers={FAIL_AFTER_HEADER: "2"})
        if reply.status == 200:
            raise CheckFailed("fault injection is not compiled in: start the API with ZQS_FAULT_INJECTION=1")
        expect(reply.status == 429 and reply.json()["reason"] == "spending_limit",
               f"failed admission commit: {reply.status} {reply.text[:200]}")
        after = self.stats()
        for key in ("next_seq", "calls_today", "ip_count", "inflight", "text_bytes"):
            expect(after[key] == before[key], f"{key} changed: {before[key]} -> {after[key]}")
        # The minute sum may lose an older charge between the two calls; longer windows cannot.
        for window in ("hour", "day", "month"):
            expect(after["sums_usd"][window] == before["sums_usd"][window], f"{window} sum changed")
        expect(self.export("questions", after=str(before["next_seq"] - 1)) == [], "a question row survived the rollback")
        expect(self.stub_stats()["requests"] == stub_before, "the provider was called after a failed admission")
        again = self.ask("after the rollback", ip=ip)
        expect(again.status == 200, f"next call after the rollback: {again.status}")
        return "3rd write failed: question row and IP hit rolled back, meta unchanged, 429 spending_limit, next call ok"

    def check_command_rollback(self) -> str:
        before = self.stats()
        marker = f"rollback-{uuid.uuid4().hex[:8]}"
        reply = http("POST", f"{self.site}/api/log/command", {"command": marker, "mode": "classic"},
                     headers={FAIL_AFTER_HEADER: "1"})
        expect(reply.status == 500 and reply.body == b"", f"failed command commit: {reply.status}")
        after = self.stats()
        for key in ("cmd_next_seq", "cmd_today", "text_bytes"):
            expect(after[key] == before[key], f"{key} changed: {before[key]} -> {after[key]}")
        lines = self.export("commands", after=str(before["cmd_next_seq"] - 1))
        expect(not any(line["command"] == marker for line in lines), "the command row survived the rollback")
        again = http("POST", f"{self.site}/api/log/command", {"command": marker, "mode": "classic"})
        expect(again.status == 204, f"retry: {again.status}")
        lines = self.export("commands", after=str(before["cmd_next_seq"] - 1))
        expect([line["command"] for line in lines] == [marker], f"commands after retry: {lines}")
        return "2nd write failed: command row rolled back, 500; retry stored once"

    def check_settlement_retry(self) -> str:
        before = self.stats()
        marker = uuid.uuid4().hex[:8]
        # "finish:1": only the inline settlement fails (after its first write); the retry that the
        # API schedules with ctx.wait_until goes through.
        reply = self.ask(f"settlement retry {marker}", ip=self.fake_ip(), headers={FAIL_AFTER_HEADER: "finish:1"})
        expect(reply.status == 200 and reply.json()["ai_enabled"] is True,
               f"the answer must be sent despite the failed settlement: {reply.status} {reply.text[:200]}")
        leased = self.stats()["inflight"]
        expect(len(leased) == 1, f"the inline settlement should have failed and kept the lease: {leased}")
        deadline = time.monotonic() + 10
        while True:
            lines = self.export("questions", after=str(before["next_seq"] - 1))
            ids = {line["question_id"] for line in lines
                   if line["entry_type"] == "ai_question" and marker in (line["question"] or "")}
            kinds = sorted(line["entry_type"] for line in lines if line["question_id"] in ids)
            if kinds == ["ai_answer", "ai_question", "ai_usage"] or time.monotonic() > deadline:
                break
            time.sleep(0.5)
        expect(kinds == ["ai_answer", "ai_question", "ai_usage"], f"not settled by the retry: {kinds}")
        expect(self.stats()["inflight"] == [], "the slot is still leased after the retry")
        return "inline settlement rolled back (slot still leased), the waitUntil retry settled the row 1 s later"

    # -- command log

    def check_command_log(self) -> str:
        before = self.stats()
        url = f"{self.site}/api/log/command"
        stored = []
        for body, status in (({"command": "projects", "mode": "classic"}, 204),
                             ({"command": "help", "mode": " ai "}, 204),
                             ({"command": "  about  "}, 204),
                             ({"command": "faq", "mode": None, "extra": True}, 204),
                             ({"command": "x" * 250}, 204),
                             ({"command": "   "}, 400),
                             ({"command": "x" * 2001}, 400),
                             ({"command": "help", "mode": "AI"}, 400)):
            reply = http("POST", url, body)
            expect(reply.status == status and reply.body == b"", f"{body}: {reply.status}")
        texts = [
            (http("POST", url, raw=b'{"command":"x"}', content_type="text/plain"),
             415, "Expected request with `Content-Type: application/json`"),
            (http("POST", url, raw=b'{"command":"' + b"x" * 65_536 + b'"}'),
             413, "Failed to buffer the request body: length limit exceeded"),
            (http("POST", url, raw=b'{"mode":"ai"}'),
             422, "Failed to deserialize the JSON body into the target type: missing field `command` at line 1 column 13"),
            (http("POST", url, raw=b'{"command":'),
             400, "Failed to parse the request body as JSON: command: EOF while parsing a value at line 1 column 11"),
        ]
        for reply, status, text in texts:
            expect(reply.status == status and reply.text == text, f"{status}: {reply.status} {reply.text!r}")
            expect(reply.header("Content-Type") == "text/plain; charset=utf-8", f"{status}: content type")
        trailing = http("POST", url, raw=b'{"command":"trailing"} garbage')
        expect(trailing.status == 204, f"trailing bytes: {trailing.status}")
        lines = self.export("commands", after=str(before["cmd_next_seq"] - 1))
        stored = [(line["command"], line["command_len"], line["mode"]) for line in lines]
        expected = [("projects", 8, "classic"), ("help", 4, "ai"), ("about", 5, "classic"), ("faq", 3, "classic"),
                    ("x" * 200 + " [truncated 50 chars]", 250, "classic"), ("trailing", 8, "classic")]
        expect(stored == expected, f"stored {stored}")
        expect(all(line["ip"] not in ("", "unknown") for line in lines), "a command lost the visitor ip")
        return "204/400 rules, axum 415/413/422/400 texts, mode trimmed, 200-character cap, trailing bytes"

    # -- admin

    def check_admin_rejections(self) -> str:
        for label, reply in (
            ("wrong token", self.admin("stats", token="x" * 43)),
            ("missing token", http("POST", f"{self.site}/api/admin/stats", {})),
            ("GET", http("GET", f"{self.site}/api/admin/stats")),
            ("unknown op", self.admin("unknown")),
            ("text/plain", http("POST", f"{self.site}/api/admin/stats", raw=json.dumps({"token": self.token}).encode(),
                                content_type="text/plain")),
        ):
            expect(reply.status == 404 and reply.body == b"", f"{label}: {reply.status}")
            expect(reply.header("Cache-Control") == "no-store", f"{label}: Cache-Control")
        bad = self.admin("export", {"table": "nope"})
        expect(bad.status == 400 and bad.json() == {"error": "invalid_table"}, f"bad table: {bad.status} {bad.text}")
        return "wrong or missing token, GET, unknown op, wrong content type -> 404 no-store; bad input -> 400"

    def check_admin_import_export(self) -> str:
        now = int(time.time())
        prefix = f"local-test-{uuid.uuid4().hex[:8]}"
        recent, old = f"{prefix}-recent", f"{prefix}-old"
        before = self.stats()
        charges = [{"id": recent, "at": now - 60, "usd": 0.001}, {"id": old, "at": now - 40 * 86400, "usd": 0.5}]
        first = self.admin("import-ledger", {"charges": charges})
        expect(first.status == 200, f"import: {first.status} {first.text}")
        expect(first.json() == {"status": "imported", "inserted": 1, "updated": 0, "skipped": 1}, first.text)
        charges[0]["usd"] = 0.002
        second = self.admin("import-ledger", {"charges": charges})
        expect(second.json() == {"status": "imported", "inserted": 0, "updated": 1, "skipped": 1}, second.text)
        invalid = self.admin("import-ledger", {"charges": [{"id": "", "at": now, "usd": 1}]})
        expect(invalid.status == 400 and invalid.json() == {"error": "invalid_charge:0"}, invalid.text)
        after = self.stats()
        delta = after["sums_usd"]["month"] - before["sums_usd"]["month"]
        expect(abs(delta - 0.002) < 1e-9, f"month sum moved by {delta}")
        ledger = self.export("ledger")
        entry = [line for line in ledger if line["id"] == recent]
        expect(entry == [{"id": recent, "at": now - 60, "usd": 0.002}], f"ledger export {entry}")
        expect(not any(line["id"] == old for line in ledger), "a charge older than 30 days was imported")
        questions = self.export("questions", after=str(before["next_seq"] - 1))
        expect(questions == [], "imported charges must not produce question lines")
        # A full chunk (25 charges) goes through the real Durable Object; one more is refused.
        full = [{"id": f"{prefix}-chunk-{i}", "at": now - 120, "usd": 0} for i in range(26)]
        refused = self.admin("import-ledger", {"charges": full})
        expect(refused.status == 400 and refused.json() == {"error": "too_many_charges"},
               f"26 charges: {refused.status} {refused.text}")
        accepted = self.admin("import-ledger", {"charges": full[:25]})
        expect(accepted.status == 200
               and accepted.json() == {"status": "imported", "inserted": 25, "updated": 0, "skipped": 0},
               f"25 charges: {accepted.status} {accepted.text}")
        paged = self.export("commands", limit=1)
        whole = self.export("commands")
        expect(paged == whole and len(whole) > 1, f"paged command export: {len(paged)} vs {len(whole)} lines")
        return ("import inserted/updated/skipped by id, invalid chunk refused, 25-charge chunk ok and 26 refused, "
                "ledger export, 1-row pages followed")

    def check_admin_rebuild_purge_close(self) -> str:
        before = self.stats()
        rebuilt = self.admin("rebuild")
        expect(rebuilt.status == 200 and rebuilt.json() == {"status": "done"}, f"rebuild: {rebuilt.text}")
        after = self.stats()
        # The minute window may move between the calls; the longer ones must match exactly.
        for window in ("hour", "day", "month"):
            expect(abs(after["sums_usd"][window] - before["sums_usd"][window]) < 1e-9,
                   f"{window}: {before['sums_usd'][window]} -> {after['sums_usd'][window]}")
        for key in ("next_seq", "cmd_next_seq", "ip_count", "calls_today", "cmd_today"):
            expect(after[key] == before[key], f"rebuild changed {key}: {before[key]} -> {after[key]}")
        kept = self.admin("purge", {"table": "questions", "before": int(time.time())})
        expect(kept.json() == {"status": "purged", "deleted": 0, "more": False},
               f"questions inside the 30-day window were purged: {kept.text}")
        deleted = 0
        while True:
            purge = self.admin("purge", {"table": "commands", "before": int(time.time()) + 1})
            expect(purge.status == 200, f"purge: {purge.text}")
            reply = purge.json()
            expect(not reply.get("budget_exhausted"), f"the daily delete budget ran out: {reply}")
            deleted += reply["deleted"]
            if not reply["more"]:
                break
        expect(deleted > 0, "no command was purged")
        expect(self.export("commands") == [], "commands remain after the purge")
        # Every deleted command is one row written against the daily delete budget.
        budget = load_jsonc(ROOT / "api" / "wrangler.jsonc")["vars"]["LOG_DELETE_DAILY_ROWS"]
        counted = self.stats()
        expect(counted["delete_rows_today"] - before["delete_rows_today"] == deleted,
               f"delete budget counted {counted['delete_rows_today'] - before['delete_rows_today']} rows for {deleted}")
        expect(counted["delete_daily_rows"] == int(budget), f"delete budget {counted['delete_daily_rows']}")
        closed = self.admin("import-ledger/close")
        expect(closed.json() == {"status": "done"}, f"close: {closed.text}")
        refused = self.admin("import-ledger", {"charges": []})
        expect(refused.status == 400 and refused.json() == {"error": "import_closed"}, f"after close: {refused.text}")
        expect(self.stats()["import_closed"] is True, "import_closed not reported")
        return (f"rebuild kept every sum, questions within 30 days kept, {deleted} commands purged and counted "
                "against the daily delete budget, import closed")

    def check_disconnected_binding(self) -> str:
        if self.stop_api is None:
            return "skipped (--no-start: stop the API session yourself and expect 502 Bad gateway)"
        self.stop_api()
        reply = http("GET", f"{self.site}/api/version", timeout=30)
        expect(reply.status == 502 and reply.text == "Bad gateway", f"{reply.status} {reply.text[:100]}")
        expect(reply.header("Strict-Transport-Security") == HSTS, "502 without HSTS")
        ai = http("POST", f"{self.site}/api/ai", {"question": "hello"}, timeout=30)
        expect(ai.status == 502, f"/api/ai: {ai.status}")
        return "API stopped: the site answers 502 Bad gateway with HSTS"


# ---------------------------------------------------------------- main

def parse_args(argv=None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Local end-to-end test of zqsdev-api behind the zqsdev site Worker.")
    parser.add_argument("--site-port", type=int, default=8787)
    parser.add_argument("--api-port", type=int, default=8788)
    parser.add_argument("--stub-port", type=int, default=9911)
    parser.add_argument("--site-inspector-port", type=int, default=9787)
    parser.add_argument("--api-inspector-port", type=int, default=9788)
    parser.add_argument("--wrangler", default="npx wrangler", help="Command that runs wrangler (default: %(default)s)")
    parser.add_argument("--start-timeout", type=float, default=900.0, help="Seconds to wait for the first API build")
    parser.add_argument("--no-start", action="store_true", help="Test servers that are already running")
    parser.add_argument("--keep-temp", action="store_true", help="Keep the temporary directory (logs, local state)")
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    site = f"http://127.0.0.1:{args.site_port}"
    api = f"http://127.0.0.1:{args.api_port}"
    stub = f"http://127.0.0.1:{args.stub_port}"
    processes: list[Process] = []
    temp: Optional[Path] = None
    failed = True
    try:
        if args.no_start:
            token = os.environ.get("ZQS_ADMIN_TOKEN", "")
            if len(token) < 32:
                print("ZQS_ADMIN_TOKEN must hold the ADMIN_TOKEN of api/.dev.vars (32+ characters).", file=sys.stderr)
                return 2
            suite = Suite(site, api, stub, token, managed=False)
        else:
            busy = [port for port in (args.site_port, args.api_port, args.stub_port,
                                      args.site_inspector_port, args.api_inspector_port) if port_in_use(port)]
            if busy:
                print(f"Ports already in use: {busy}. Stop those servers or pick other ports.", file=sys.stderr)
                return 2
            temp = Path(tempfile.mkdtemp(prefix="zqs-"))
            # Local Durable Object files live at <persist>/v3/do/zqsdev-api-Ledger/<64 hex>.sqlite.
            if os.name == "nt" and len(str(temp)) + 100 > 259:
                print(f"{temp} is too long for Windows paths of local Durable Object storage; "
                      "set TEMP to a shorter directory.", file=sys.stderr)
                shutil.rmtree(temp, ignore_errors=True)
                temp = None
                return 2
            token = secrets.token_urlsafe(32)
            env_file = temp / "api.env"
            env_file.write_text(f"OPENAI_API_KEY=sk-local-placeholder\nADMIN_TOKEN={token}\n", encoding="utf-8")
            # wrangler lets process.env override the env file: drop real credentials so the local
            # Workers only ever see the throwaway values above (and never reach Cloudflare).
            env = {key: value for key, value in os.environ.items() if key.upper() not in SECRET_ENV}
            env["WRANGLER_SEND_METRICS"] = "false"
            env.pop("ZQS_FAULT_INJECTION", None)
            wrangler = args.wrangler.split()
            if wrangler and shutil.which(wrangler[0]):
                wrangler[0] = shutil.which(wrangler[0])
            print(f"Temporary directory: {temp}", flush=True)

            processes.append(Process("openai stub", [sys.executable, str(ROOT / "scripts" / "openai_stub.py"),
                                                     "--port", str(args.stub_port), "--quiet",
                                                     "--require-key-prefix", "sk-local-"],
                                     ROOT, env, temp / "stub.log"))
            stub_process = processes[-1]
            wait_for("the OpenAI stub", lambda: http("GET", f"{stub}/__stats", timeout=2).status == 200, 30, stub_process)

            api_env = dict(env, ZQS_FAULT_INJECTION="1")
            api_process = Process("API wrangler dev", wrangler + [
                "dev", "--local", "--port", str(args.api_port), "--inspector-port", str(args.api_inspector_port),
                "--env-file", str(env_file), "--persist-to", str(temp / "a"),
                "--var", f"OPENAI_BASE_URL:{stub}/v1", "--show-interactive-dev-session=false",
            ], ROOT / "api", api_env, temp / "api.log")
            processes.append(api_process)
            print("Building and starting the API Worker (first build: a few minutes)...", flush=True)
            wait_for("the API Worker", lambda: http("GET", f"{api}/api/version", timeout=5).status == 200,
                     args.start_timeout, api_process)

            site_process = Process("site wrangler dev", wrangler + [
                "dev", "--local", "--port", str(args.site_port), "--inspector-port", str(args.site_inspector_port),
                "--persist-to", str(temp / "s"), "--infer-origin-from-routes=false",
                "--show-interactive-dev-session=false",
            ], ROOT, env, temp / "site.log")
            processes.append(site_process)
            wait_for("the site Worker and its API binding",
                     lambda: http("GET", f"{site}/api/version", timeout=10).status == 200, 180, site_process)
            suite = Suite(site, api, stub, token, managed=True, stop_api=api_process.stop)

        checks = [
            ("/api/data through the site", suite.check_data),
            ("/api/version", suite.check_version),
            ("/api/health", suite.check_health),
            ("Methods, unknown paths, site rules", suite.check_methods_and_paths),
            ("AI answer and visitor IP", suite.check_ai_answer_and_visitor_ip),
            ("AI request validation", suite.check_ai_validation),
            ("Provider failures", suite.check_provider_failures),
            ("Slots, timeout and disconnect", suite.check_slots_timeout_and_disconnect),
            ("Per-IP request limit", suite.check_request_limit),
            ("Admission rollback (transactionSync)", suite.check_admission_rollback),
            ("Command rollback (transactionSync)", suite.check_command_rollback),
            ("Settlement retry (waitUntil)", suite.check_settlement_retry),
            ("Command log", suite.check_command_log),
            ("Admin rejections", suite.check_admin_rejections),
            ("Admin import and export", suite.check_admin_import_export),
            ("Admin rebuild, purge, close", suite.check_admin_rebuild_purge_close),
            ("Disconnected binding", suite.check_disconnected_binding),
        ]
        for name, check in checks:
            suite.run(name, check)
        passed = sum(result.status == "PASS" for result in suite.results)
        failures = sum(result.status == "FAIL" for result in suite.results)
        skipped = sum(result.status == "SKIP" for result in suite.results)
        print(f"Totals: {passed} passed, {failures} failed, {skipped} skipped")
        failed = failures > 0
        return 1 if failed else 0
    except RuntimeError as error:
        print(f"Setup failed: {error}", file=sys.stderr)
        return 2
    finally:
        for process in reversed(processes):
            process.stop()
        if temp is not None:
            if failed or args.keep_temp:
                print(f"Logs and local state kept in {temp}")
            else:
                shutil.rmtree(temp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
