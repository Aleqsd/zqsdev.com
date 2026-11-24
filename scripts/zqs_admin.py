#!/usr/bin/env python3
"""Admin client for the zqsdev-api ledger (POST /api/admin/*).

    python3 scripts/zqs_admin.py stats
    python3 scripts/zqs_admin.py export --table questions --last 50
    python3 scripts/zqs_admin.py export --table commands --out ~/zqs-exports/commands.ndjson
    python3 scripts/zqs_admin.py export --table ledger --vps-json --out ai-budget.json
    python3 scripts/zqs_admin.py import-ledger --file ai-budget.snapshot.json
    python3 scripts/zqs_admin.py import-ledger --file ai-budget.final.json --close
    python3 scripts/zqs_admin.py purge --table commands --before 2026-01-01
    python3 scripts/zqs_admin.py rebuild

The admin token comes from the ZQS_ADMIN_TOKEN environment variable, or is asked for without
echo. It travels only in the JSON body: never in a URL, a header or a command-line argument, and
it is never printed. --api selects the base URL (default https://zqsdev.com; the canary is
https://zqsdev-api.aleqsd.workers.dev); it must be https://, plain http:// only for this machine.

Exports hold visitor IPs, questions, answers and commands, and the repository is public: --out
refuses a path that git would track (write exports outside the repository; *.ndjson and
ai-budget*.json are git-ignored anyway) and creates the file readable by its owner only.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

USER_AGENT = "zqsdev-admin/1.0"
LOOPBACK_HOSTS = ("127.0.0.1", "localhost", "::1")
# Charges per import request: the API refuses more (each charge costs Durable Object CPU).
CHUNK_MAX = 25
# Deletes count as Durable Object rows written (Free: 100,000 per day for the whole account). The
# API stops purging once its daily delete budget is used; this is a second bound per run.
PURGE_MAX_ROWS = 20_000


class AdminError(Exception):
    def __init__(self, message: str, code: str = "") -> None:
        super().__init__(message)
        self.code = code


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect would drop the POST body (and the token with it): report it instead."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def checked_base_url(url: str) -> str:
    """The token travels in the request body: https only, or plain http to this machine."""
    parts = urllib.parse.urlsplit(url.strip())
    host = (parts.hostname or "").lower()
    if host and (parts.scheme == "https" or (parts.scheme == "http" and host in LOOPBACK_HOSTS)):
        return url.strip().rstrip("/")
    raise AdminError(f"--api must be an https:// URL (plain http:// only for 127.0.0.1, localhost or ::1), "
                     f"not {url!r}: the admin token would cross the network in clear text")


def private_output(out: str) -> Path:
    """Where an export may go: never a path that git would track, since the repository is public.
    Missing directories are created (owner only) once the path is accepted."""
    target = Path(out).expanduser().resolve()
    anchor = next((folder for folder in target.parents if folder.is_dir()), None)
    tracked_area = ignored = False
    if anchor is not None:
        try:
            inside = subprocess.run(["git", "-C", str(anchor), "rev-parse", "--is-inside-work-tree"],
                                    capture_output=True, text=True, timeout=10)
            tracked_area = inside.returncode == 0 and inside.stdout.strip() == "true"
            ignored = tracked_area and subprocess.run(
                ["git", "-C", str(anchor), "check-ignore", "-q", "--", str(target)],
                capture_output=True, timeout=10).returncode == 0
        except (OSError, subprocess.SubprocessError):
            tracked_area = False  # no git to commit it with
    if tracked_area and not ignored:
        raise AdminError(f"{out} is inside a git work tree and not git-ignored: exports hold visitor IPs and "
                         "free text, so write them outside the repository (or name them *.ndjson)")
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    return target


def write_private(path: Path, text: str) -> None:
    """Creates or replaces the file, readable and writable by its owner only."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    try:
        os.chmod(path, 0o600)  # a file that already existed keeps its old mode otherwise
    except OSError:
        pass


class Client:
    def __init__(self, api: str, token: str, timeout: float) -> None:
        self.api = api.rstrip("/")
        self.token = token
        self.timeout = timeout

    def post(self, op: str, body: Optional[dict] = None) -> tuple[int, dict, bytes]:
        payload = dict(body or {})
        payload["token"] = self.token
        request = urllib.request.Request(
            f"{self.api}/api/admin/{op}",
            data=json.dumps(payload).encode("utf-8"),
            headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
            method="POST",
        )
        try:
            with OPENER.open(request, timeout=self.timeout) as response:
                return response.status, dict(response.headers), response.read()
        except urllib.error.HTTPError as error:
            status, headers, data = error.code, dict(error.headers), error.read()
        except urllib.error.URLError as error:
            raise AdminError(f"{op}: cannot reach {self.api}: {error.reason}") from None
        if status == 404:
            raise AdminError(f"{op}: rejected (404): admin disabled, wrong token or unknown route")
        if 300 <= status < 400:
            raise AdminError(f"{op}: redirected ({status}) to {headers.get('Location')}; use the final host")
        try:
            code = str(json.loads(data).get("error", ""))
        except (ValueError, AttributeError):
            code = ""
        detail = code or data[:200].decode("utf-8", errors="replace")
        raise AdminError(f"{op}: HTTP {status} {detail}", code)

    def json(self, op: str, body: Optional[dict] = None) -> dict:
        _, _, data = self.post(op, body)
        return json.loads(data)


def header(headers: dict, name: str) -> Optional[str]:
    for key, value in headers.items():
        if key.lower() == name.lower():
            return value
    return None


def cmd_stats(client: Client, args) -> int:
    print(json.dumps(client.json("stats"), indent=2, sort_keys=False))
    return 0


def export_pages(client: Client, table: str, after: str):
    """Yields the NDJSON lines of every page, following X-Next-After."""
    cursor, pages = after, 0
    while True:
        _, headers, data = client.post("export", {"table": table, "after": cursor})
        pages += 1
        for line in data.decode("utf-8").splitlines():
            if line.strip():
                yield line
        cursor = header(headers, "X-Next-After")
        if not cursor:
            print(f"[export] {table}: {pages} page(s)", file=sys.stderr)
            return


def cmd_export(client: Client, args) -> int:
    if args.vps_json and args.table != "ledger":
        raise AdminError("--vps-json only applies to --table ledger")
    out = private_output(args.out) if args.out else None
    after = args.after
    if args.last is not None:
        if args.last <= 0:
            raise AdminError("--last must be positive")
        if args.table in ("questions", "commands"):
            stats = client.json("stats")
            next_seq = stats["next_seq"] if args.table == "questions" else stats["cmd_next_seq"]
            after = str(max(0, next_seq - 1 - args.last))
    lines = list(export_pages(client, args.table, after))
    if args.last is not None and args.table == "ledger":
        lines = lines[-args.last:]
    if args.vps_json:
        # The VPS ledger file: one compact JSON array of {"id","at","usd"}, oldest first.
        text = "[" + ",".join(lines) + "]"
    else:
        text = "".join(line + "\n" for line in lines)
    if out is not None:
        write_private(out, text)
        print(f"[export] {len(lines)} line(s) written to {out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
        if args.vps_json:
            sys.stdout.write("\n")
    return 0


def load_charges(path: str) -> list:
    try:
        charges = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise AdminError(f"cannot read {path}: {error}") from None
    if not isinstance(charges, list) or not all(isinstance(c, dict) for c in charges):
        raise AdminError(f"{path} is not a JSON array of {{\"id\",\"at\",\"usd\"}} objects")
    return charges


def cmd_import(client: Client, args) -> int:
    if not args.file and not args.close:
        raise AdminError("give --file, --close or both")
    if not 1 <= args.chunk <= CHUNK_MAX:
        raise AdminError(f"--chunk must be between 1 and {CHUNK_MAX}")
    totals = {"inserted": 0, "updated": 0, "skipped": 0}
    if args.file:
        charges = load_charges(args.file)
        for start in range(0, len(charges), args.chunk):
            chunk = charges[start:start + args.chunk]
            try:
                reply = client.json("import-ledger", {"charges": chunk})
            except AdminError as error:
                index = error.code.removeprefix("invalid_charge:")
                if error.code.startswith("invalid_charge:") and index.isdigit() and int(index) < len(chunk):
                    raise AdminError(f"charge #{start + int(index)} is invalid ({chunk[int(index)]!r}); "
                                     "nothing of this chunk was written") from None
                if error.code == "ledger_unavailable" and len(chunk) > 1:
                    smaller = max(1, len(chunk) // 2)
                    raise AdminError(f"{error} at charges {start}-{start + len(chunk) - 1}. If it repeats, the "
                                     f"chunk may exceed the Durable Object's CPU limit: run the same command "
                                     f"again with --chunk {smaller} (the import is idempotent).", error.code) from None
                raise
            for key in totals:
                totals[key] += reply.get(key, 0)
            print(f"[import] charges {start}-{start + len(chunk) - 1}: inserted {reply.get('inserted')}, "
                  f"updated {reply.get('updated')}, skipped {reply.get('skipped')}", file=sys.stderr)
        print(json.dumps({"charges": len(charges), **totals}))
    if args.close:
        reply = client.json("import-ledger/close")
        print(f"[import] closed: {reply}", file=sys.stderr)
    return 0


def parse_before(value: str) -> int:
    if value.isdigit():
        return int(value)
    try:
        moment = datetime.strptime(value, "%Y-%m-%d").replace(tzinfo=timezone.utc)
    except ValueError:
        raise argparse.ArgumentTypeError("use unix seconds or YYYY-MM-DD (UTC)") from None
    return int(moment.timestamp())


def cmd_purge(client: Client, args) -> int:
    if args.max_rows <= 0:
        raise AdminError("--max-rows must be positive")
    deleted, calls, stopped = 0, 0, "done"
    while True:
        reply = client.json("purge", {"table": args.table, "before": args.before})
        deleted += reply.get("deleted", 0)
        calls += 1
        if not reply.get("more"):
            break
        if reply.get("budget_exhausted"):
            stopped = "daily_budget"
            print("[purge] the API's daily delete budget (LOG_DELETE_DAILY_ROWS) is used up; run the same "
                  "command again after 00:00 UTC to continue.", file=sys.stderr)
            break
        if deleted >= args.max_rows:
            stopped = "max_rows"
            print(f"[purge] stopped after {deleted} rows (--max-rows {args.max_rows}); run the same command "
                  "again to continue.", file=sys.stderr)
            break
        time.sleep(0.2)
    print(json.dumps({"table": args.table, "before": args.before, "deleted": deleted, "calls": calls,
                      "stopped": stopped}))
    return 0


def cmd_rebuild(client: Client, args) -> int:
    print(json.dumps(client.json("rebuild")))
    return 0


def parse_args(argv=None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Admin client for the zqsdev-api ledger.")
    parser.add_argument("--api", default="https://zqsdev.com", help="Base URL (default: %(default)s)")
    parser.add_argument("--timeout", type=float, default=30.0, help="Seconds per request (default: %(default)s)")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("stats", help="Window sums, slots, counters, sizes, import state")

    export = sub.add_parser("export", help="Export questions, commands or the spend ledger as NDJSON")
    export.add_argument("--table", required=True, choices=("questions", "commands", "ledger"))
    export.add_argument("--after", default="0", help="Cursor to start after (default: the beginning)")
    export.add_argument("--last", type=int, help="Only the last N rows")
    export.add_argument("--out", help="Write to this file instead of stdout (outside the repository, or git-ignored)")
    export.add_argument("--vps-json", action="store_true",
                        help="Ledger only: one JSON array in the VPS ai-budget.json format")

    imp = sub.add_parser("import-ledger", help="Import the VPS ai-budget.json (idempotent, by charge id)")
    imp.add_argument("--file", help="The VPS ledger file: a JSON array of {id, at, usd}")
    imp.add_argument("--chunk", type=int, default=CHUNK_MAX, help="Charges per request (default: %(default)s)")
    imp.add_argument("--close", action="store_true", help="Close the import afterwards (final import only)")

    purge = sub.add_parser("purge", help="Delete old questions or commands (bounded batches, daily budget)")
    purge.add_argument("--table", required=True, choices=("questions", "commands"))
    purge.add_argument("--before", required=True, type=parse_before,
                       help="Unix seconds or YYYY-MM-DD; questions inside the 30-day window are always kept")
    purge.add_argument("--max-rows", type=int, default=PURGE_MAX_ROWS,
                       help="Stop once this many rows are deleted in this run (default: %(default)s); the API "
                            "also stops at its daily delete budget until 00:00 UTC")

    sub.add_parser("rebuild", help="Recompute the window sums and counters from the rows")
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    try:
        api = checked_base_url(args.api)
    except AdminError as error:
        print(f"[zqs-admin] {error}", file=sys.stderr)
        return 2
    token = os.environ.get("ZQS_ADMIN_TOKEN") or getpass.getpass("Admin token: ")
    if len(token) < 32:
        print("The admin token has at least 32 characters.", file=sys.stderr)
        return 2
    client = Client(api, token, args.timeout)
    handlers = {
        "stats": cmd_stats,
        "export": cmd_export,
        "import-ledger": cmd_import,
        "purge": cmd_purge,
        "rebuild": cmd_rebuild,
    }
    try:
        return handlers[args.command](client, args)
    except AdminError as error:
        print(f"[zqs-admin] {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
