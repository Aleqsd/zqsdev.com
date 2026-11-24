#!/usr/bin/env python3
"""Admin client for the zqsdev-api ledger (POST /api/admin/*).

    python3 scripts/zqs_admin.py stats
    python3 scripts/zqs_admin.py export --table questions --last 50
    python3 scripts/zqs_admin.py export --table commands --out commands.ndjson
    python3 scripts/zqs_admin.py export --table ledger --vps-json --out ai-budget.json
    python3 scripts/zqs_admin.py import-ledger --file ai-budget.snapshot.json
    python3 scripts/zqs_admin.py import-ledger --file ai-budget.final.json --close
    python3 scripts/zqs_admin.py purge --table commands --before 2026-01-01
    python3 scripts/zqs_admin.py rebuild

The admin token comes from the ZQS_ADMIN_TOKEN environment variable, or is asked for without
echo. It travels only in the JSON body: never in a URL, a header or a command-line argument, and
it is never printed. --api selects the base URL (default https://zqsdev.com; the canary is
https://zqsdev-api.aleqsd.workers.dev).
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

USER_AGENT = "zqsdev-admin/1.0"
CHUNK_MAX = 250


class AdminError(Exception):
    def __init__(self, message: str, code: str = "") -> None:
        super().__init__(message)
        self.code = code


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect would drop the POST body (and the token with it): report it instead."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


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
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8", newline="\n")
        print(f"[export] {len(lines)} line(s) written to {args.out}", file=sys.stderr)
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
    deleted, calls = 0, 0
    while True:
        reply = client.json("purge", {"table": args.table, "before": args.before})
        deleted += reply.get("deleted", 0)
        calls += 1
        if not reply.get("more"):
            break
        time.sleep(0.2)
    print(json.dumps({"table": args.table, "before": args.before, "deleted": deleted, "calls": calls}))
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
    export.add_argument("--out", help="Write to this file instead of stdout")
    export.add_argument("--vps-json", action="store_true",
                        help="Ledger only: one JSON array in the VPS ai-budget.json format")

    imp = sub.add_parser("import-ledger", help="Import the VPS ai-budget.json (idempotent, by charge id)")
    imp.add_argument("--file", help="The VPS ledger file: a JSON array of {id, at, usd}")
    imp.add_argument("--chunk", type=int, default=CHUNK_MAX, help="Charges per request (default: %(default)s)")
    imp.add_argument("--close", action="store_true", help="Close the import afterwards (final import only)")

    purge = sub.add_parser("purge", help="Delete old questions or commands (bounded batches)")
    purge.add_argument("--table", required=True, choices=("questions", "commands"))
    purge.add_argument("--before", required=True, type=parse_before,
                       help="Unix seconds or YYYY-MM-DD; questions inside the 30-day window are always kept")

    sub.add_parser("rebuild", help="Recompute the window sums and counters from the rows")
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    token = os.environ.get("ZQS_ADMIN_TOKEN") or getpass.getpass("Admin token: ")
    if len(token) < 32:
        print("The admin token has at least 32 characters.", file=sys.stderr)
        return 2
    client = Client(args.api, token, args.timeout)
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
