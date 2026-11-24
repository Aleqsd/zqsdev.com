#!/usr/bin/env python3
"""Post-deploy check of the zqsdev-api Worker, run by `make deploy-api` right after `wrangler deploy`.

    python3 scripts/verify_api_deploy.py --base-url https://zqsdev.com [--no-rollback]

1. Polls GET /api/version every 2 s, for up to 60 s, until it reports the local VERSION and the
   HEAD commit (the release that was just deployed is live).
2. Then requires ai_configured:true, the OPENAI_MODEL of api/wrangler.jsonc, GET /api/health ->
   200 with "ok":true, and GET /api/data -> 200 with the 7 data keys.
On any failure it runs `wrangler rollback` in api/ and exits non-zero; --no-rollback only prints
what failed. No paid call is made.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
USER_AGENT = "zqsdev-deploy-check/1.0"
DATA_KEYS = ("profile", "skills", "experiences", "education", "projects", "testimonials", "faq")


class CheckFailed(Exception):
    pass


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


def get(url: str, timeout: float = 10.0):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def head_commit() -> str:
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, check=True,
                          capture_output=True, text=True).stdout.strip()


def wait_for_release(base: str, version: str, commit: str, timeout: float, interval: float) -> dict:
    deadline = time.monotonic() + timeout
    last = "no answer"
    while True:
        try:
            status, body = get(f"{base}/api/version")
            if status == 200:
                payload = json.loads(body)
                if payload.get("version") == version and payload.get("commit") == commit:
                    return payload
                last = f"version {payload.get('version')} commit {payload.get('commit')}"
            else:
                last = f"HTTP {status}"
        except (OSError, ValueError, urllib.error.URLError) as error:
            last = str(error)
        if time.monotonic() >= deadline:
            raise CheckFailed(f"/api/version never reported {version} at {commit[:12]} ({last})")
        time.sleep(interval)


def verify(base: str, timeout: float, interval: float) -> str:
    version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    commit = head_commit()
    model = load_jsonc(ROOT / "api" / "wrangler.jsonc")["vars"]["OPENAI_MODEL"]
    payload = wait_for_release(base, version, commit, timeout, interval)
    if payload.get("ai_configured") is not True:
        raise CheckFailed("ai_configured is false (missing OPENAI_API_KEY secret or invalid prices)")
    if payload.get("model") != model:
        raise CheckFailed(f"model {payload.get('model')!r}, expected {model!r}")
    status, body = get(f"{base}/api/health")
    try:
        health = json.loads(body)
    except ValueError:
        health = {}
    if status != 200 or health.get("ok") is not True:
        raise CheckFailed(f"/api/health returned {status}: {body[:200].decode('utf-8', errors='replace')}")
    status, body = get(f"{base}/api/data")
    if status != 200:
        raise CheckFailed(f"/api/data returned {status}")
    missing = [key for key in DATA_KEYS if key not in json.loads(body)]
    if missing:
        raise CheckFailed(f"/api/data lacks {missing}")
    return f"version {version}, commit {commit[:12]}, model {model}, health ok, data ok"


def rollback(reason: str) -> int:
    wrangler = shlex.split(os.environ.get("WRANGLER", "npx wrangler"))
    executable = shutil.which(wrangler[0])
    if executable:
        wrangler[0] = executable
    command = wrangler + ["rollback", "--message", f"post-deploy check failed: {reason}"[:100]]
    print(f"[verify-api] rolling back: {' '.join(command[1:])}", file=sys.stderr)
    return subprocess.run(command, cwd=ROOT / "api", check=False).returncode


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Verify a zqsdev-api deploy and roll it back on failure.")
    parser.add_argument("--base-url", default="https://zqsdev.com", help="Where /api/* is served (default: %(default)s)")
    parser.add_argument("--timeout", type=float, default=60.0, help="Seconds to wait for the new release (default: %(default)s)")
    parser.add_argument("--interval", type=float, default=2.0, help="Seconds between polls (default: %(default)s)")
    parser.add_argument("--no-rollback", action="store_true", help="Only report a failure (first deploy)")
    args = parser.parse_args(argv)
    base = args.base_url.rstrip("/")
    try:
        print(f"[verify-api] OK: {verify(base, args.timeout, args.interval)}")
        return 0
    except (CheckFailed, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        reason = str(error)
        print(f"[verify-api] FAILED: {reason}", file=sys.stderr)
        if args.no_rollback:
            print("[verify-api] --no-rollback: the release stays deployed", file=sys.stderr)
        elif rollback(reason) != 0:
            print("[verify-api] the rollback failed too: run `cd api && npx wrangler rollback`", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
