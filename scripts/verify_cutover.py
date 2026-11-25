#!/usr/bin/env python3
"""Real-path checks of the API cut-over (4 paid questions, about $0.002).

    python3 scripts/verify_cutover.py --base-url https://zqsdev.com

Through the site Worker and its API service binding, it checks:
  1. visitor IP: the questions are stored with the caller's IP, not "unknown", and the
     ledger counts no unknown-IP question in the last 24 h;
  2. settlement and slots: 3 questions asked one after another all answer (none "busy"), are
     settled at their actual usage cost, and leave no slot in flight;
  3. disconnect: a question whose client gives up after 1 s is still answered and settled
     within 20 s (the site's waitUntil keeps the API call alive), with no slot in flight.
It reads the ledger through the admin API: the token comes from ZQS_ADMIN_TOKEN or a prompt,
is never printed, and is only sent over https (plain http only to this machine).
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from ipaddress import ip_address
from typing import Optional

USER_AGENT = "zqsdev-cutover-check/1.0"
LOOPBACK_HOSTS = ("127.0.0.1", "localhost", "::1")
QUESTION = "In one sentence, what is Alexandre's current role?"


class CheckFailed(Exception):
    pass


def checked_base_url(url: str) -> str:
    """The admin token travels in request bodies: https only, or plain http to this machine."""
    parts = urllib.parse.urlsplit(url.strip())
    host = (parts.hostname or "").lower()
    if host and (parts.scheme == "https" or (parts.scheme == "http" and host in LOOPBACK_HOSTS)):
        return url.strip().rstrip("/")
    raise CheckFailed(f"--base-url must be an https:// URL (plain http:// only for 127.0.0.1, localhost or ::1), "
                      f"not {url!r}: the admin token would cross the network in clear text")


def post(url: str, body: dict, timeout: float):
    request = urllib.request.Request(url, data=json.dumps(body).encode("utf-8"), method="POST",
                                     headers={"Content-Type": "application/json", "User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


class Cutover:
    def __init__(self, base: str, token: str) -> None:
        self.base = base.rstrip("/")
        self.token = token
        self.marker = uuid.uuid4().hex[:10]

    def admin(self, op: str, body: Optional[dict] = None):
        status, headers, data = post(f"{self.base}/api/admin/{op}", dict(body or {}, token=self.token), 30)
        if status != 200:
            raise CheckFailed(f"admin {op} returned {status} (check the token)")
        return headers, data

    def stats(self) -> dict:
        return json.loads(self.admin("stats")[1])

    def rows_since(self, next_seq: int) -> dict:
        """Exported lines of this run's questions, grouped by question id."""
        cursor, lines = str(max(0, next_seq - 1)), []
        while cursor:
            headers, data = self.admin("export", {"table": "questions", "after": cursor})
            lines.extend(json.loads(line) for line in data.decode("utf-8").splitlines() if line.strip())
            cursor = next((v for k, v in headers.items() if k.lower() == "x-next-after"), None)
        rows: dict = {}
        for line in lines:
            if line["entry_type"] == "ai_question" and self.marker in (line.get("question") or ""):
                rows[line["question_id"]] = {"question": line}
        for line in lines:
            if line["question_id"] in rows and line["entry_type"] != "ai_question":
                rows[line["question_id"]][line["entry_type"]] = line
        return rows

    def ask(self, number: int, timeout: float):
        body = {"question": f"{QUESTION} (cutover check {self.marker} #{number})", "history": []}
        return post(f"{self.base}/api/ai", body, timeout)

    def run(self) -> list:
        results = []
        start = self.stats()
        if start["inflight"]:
            raise CheckFailed(f"slots already in flight before the check: {start['inflight']}")

        # 2. Three questions one after another: all answered, none busy.
        began = time.monotonic()
        statuses = []
        for number in (1, 2, 3):
            status, _, data = self.ask(number, 22)
            reply = json.loads(data)
            statuses.append(status)
            if status != 200 or reply.get("ai_enabled") is not True:
                raise CheckFailed(f"question {number}: HTTP {status}, reason {reply.get('reason')}")
        sequential = time.monotonic() - began

        # 3. A client that gives up after 1 s.
        left_early = False
        try:
            self.ask(4, 1.0)
        except (TimeoutError, socket.timeout, urllib.error.URLError):
            left_early = True

        deadline = time.monotonic() + 25
        while True:
            rows = self.rows_since(start["next_seq"])
            settled = [row for row in rows.values() if "ai_answer" in row or "ai_failure" in row]
            if (len(rows) == 4 and len(settled) == 4) or time.monotonic() > deadline:
                break
            time.sleep(2)
        end = self.stats()

        # 1. Visitor IP.
        ips = {row["question"]["ip"] for row in rows.values()}
        bad = [ip for ip in ips if not ip or ip == "unknown"]
        for ip in ips - set(bad):
            ip_address(ip)
        ok_ip = len(rows) == 4 and not bad and end["unknown_ip_questions_24h"] == 0
        results.append(("Visitor IP through the binding", ok_ip,
                        f"stored ips {sorted(ips)}, unknown_ip_questions_24h {end['unknown_ip_questions_24h']}"))

        # 2. Settlement and slots.
        first_three = [row for row in rows.values() if not (row["question"]["question"] or "").endswith("#4)")]
        costs = [row.get("ai_usage", {}).get("cost_usd") for row in first_three]
        ok_settled = (len(first_three) == 3 and all("ai_answer" in row for row in first_three)
                      and all(isinstance(c, (int, float)) and c > 0 for c in costs)
                      and all(row.get("ai_usage", {}).get("usage") for row in first_three)
                      and end["inflight"] == [])
        results.append(("Settlement and slots", ok_settled,
                        f"3 answered in {sequential:.1f}s (statuses {statuses}), costs {costs}, inflight {end['inflight']}"))

        # 3. Disconnect.
        fourth = [row for row in rows.values() if (row["question"]["question"] or "").endswith("#4)")]
        answered = bool(fourth) and "ai_answer" in fourth[0]
        ok_disconnect = left_early and answered and end["inflight"] == []
        detail = ("client left after 1 s; " if left_early else "the answer came back within 1 s; ")
        detail += "answered and settled" if answered else f"not settled: {fourth}"
        results.append(("Disconnect keeps the call", ok_disconnect, detail))
        return results


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Verify the zqsdev-api cut-over on the real path (4 paid questions).")
    parser.add_argument("--base-url", default="https://zqsdev.com", help="Site to check (default: %(default)s)")
    args = parser.parse_args(argv)
    try:
        base = checked_base_url(args.base_url)
    except CheckFailed as error:
        print(f"FAIL  {error}")
        return 2
    token = os.environ.get("ZQS_ADMIN_TOKEN") or getpass.getpass("Admin token: ")
    try:
        results = Cutover(base, token).run()
    except CheckFailed as error:
        print(f"FAIL  {error}")
        return 1
    for name, ok, detail in results:
        print(f"{'PASS' if ok else 'FAIL'}  {name:<32} {detail}")
    return 0 if all(ok for _, ok, _ in results) else 1


if __name__ == "__main__":
    sys.exit(main())
