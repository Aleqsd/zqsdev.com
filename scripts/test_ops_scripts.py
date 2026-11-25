"""Operations scripts (admin client, cut-over check, live smoke test); no network access."""

import argparse
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import live_smoke_test  # noqa: E402
import verify_cutover  # noqa: E402
import zqs_admin  # noqa: E402


class FakeClient:
    """Scripted admin replies; an exception in the script is raised instead of returned."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = []

    def json(self, op, body=None):
        self.calls.append((op, body))
        reply = self.replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return reply


def run_quietly(function, *args):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        result = function(*args)
    return result, out.getvalue(), err.getvalue()


class AdminTokenTransport(unittest.TestCase):
    """The admin token only travels over https, or plain http to this machine."""

    ACCEPTED = ("https://zqsdev.com", "https://zqsdev-api.aleqsd.workers.dev/", "http://127.0.0.1:8732",
                "http://localhost:8787", "http://[::1]:8732")
    REFUSED = ("http://zqsdev.com", "http://zqsdev-api.aleqsd.workers.dev", "zqsdev.com", "ftp://zqsdev.com",
               "http://127.0.0.1.evil.example", "http://localhost.evil.example:80", "https://")

    def test_https_or_loopback_only(self):
        for url in self.ACCEPTED:
            self.assertEqual(zqs_admin.checked_base_url(url), url.rstrip("/"))
            self.assertEqual(verify_cutover.checked_base_url(url), url.rstrip("/"))
        for url in self.REFUSED:
            with self.assertRaises(zqs_admin.AdminError, msg=url):
                zqs_admin.checked_base_url(url)
            with self.assertRaises(verify_cutover.CheckFailed, msg=url):
                verify_cutover.checked_base_url(url)

    def test_a_refused_url_never_asks_for_the_token(self):
        environ = {key: value for key, value in os.environ.items() if key != "ZQS_ADMIN_TOKEN"}
        prompt = mock.Mock(side_effect=AssertionError("the token was asked for"))
        with mock.patch.dict(os.environ, environ, clear=True), mock.patch("getpass.getpass", prompt):
            status, _, err = run_quietly(zqs_admin.main, ["--api", "http://zqsdev.com", "stats"])
            self.assertEqual(status, 2)
            self.assertIn("https://", err)
            status, out, _ = run_quietly(verify_cutover.main, ["--base-url", "http://zqsdev.com"])
            self.assertEqual(status, 2)
            self.assertIn("FAIL", out)
        prompt.assert_not_called()


class ExportFiles(unittest.TestCase):
    """Exports hold visitor IPs and free text: never a path git would track."""

    def test_tracked_paths_in_the_repository_are_refused(self):
        for name in ("commands.json", "static/data/export.json", "notes-export/commands.txt"):
            with self.assertRaises(zqs_admin.AdminError, msg=name) as caught:
                zqs_admin.private_output(str(ROOT / name))
            self.assertIn("git-ignored", str(caught.exception))
        self.assertFalse((ROOT / "notes-export").exists(), "a refused export created a directory")

    def test_ignored_names_and_paths_outside_are_accepted(self):
        for name in ("commands.ndjson", "ai-budget.final.json"):
            self.assertEqual(zqs_admin.private_output(str(ROOT / name)), (ROOT / name).resolve())
        existed = (ROOT / "exports").exists()
        try:
            self.assertEqual(zqs_admin.private_output(str(ROOT / "exports" / "q.json")),
                             (ROOT / "exports" / "q.json").resolve())
        finally:
            if not existed and (ROOT / "exports").is_dir():
                (ROOT / "exports").rmdir()
        with tempfile.TemporaryDirectory() as folder:
            target = zqs_admin.private_output(os.path.join(folder, "zqs-exports", "commands.ndjson"))
            self.assertTrue(target.parent.is_dir())
            zqs_admin.write_private(target, '{"entry_type":"command"}\n')
            self.assertEqual(target.read_text(encoding="utf-8"), '{"entry_type":"command"}\n')
            zqs_admin.write_private(target, "")
            self.assertEqual(target.read_text(encoding="utf-8"), "")
            if sys.platform.startswith(("linux", "darwin")):  # NTFS ignores the mode bits
                self.assertEqual(target.stat().st_mode & 0o777, 0o600)

    def test_the_local_secrets_example_disables_admin(self):
        values = dict(line.split("=", 1) for line in (ROOT / "api" / ".dev.vars.example").read_text(
            encoding="utf-8").splitlines() if line and not line.startswith("#"))
        self.assertLess(len(values["ADMIN_TOKEN"].strip()), 32, "the public example must not enable admin")


class Purge(unittest.TestCase):
    """A purge stops at the API's daily delete budget or at --max-rows."""

    def purge(self, replies, max_rows=20_000):
        client = FakeClient(replies)
        args = argparse.Namespace(table="commands", before=1_760_000_000, max_rows=max_rows)
        with mock.patch.object(zqs_admin.time, "sleep"):
            status, out, err = run_quietly(zqs_admin.cmd_purge, client, args)
        self.assertEqual(status, 0)
        return json.loads(out), err, client

    def test_stops_when_the_daily_budget_is_used(self):
        summary, err, client = self.purge([{"deleted": 1000, "more": True},
                                           {"deleted": 500, "more": True, "budget_exhausted": True},
                                           AssertionError("called after the budget ran out")])
        self.assertEqual((summary["deleted"], summary["calls"], summary["stopped"]), (1500, 2, "daily_budget"))
        self.assertIn("00:00 UTC", err)
        self.assertEqual(len(client.calls), 2)

    def test_stops_at_max_rows(self):
        summary, err, _ = self.purge([{"deleted": 1000, "more": True}] * 5, max_rows=2500)
        self.assertEqual((summary["deleted"], summary["calls"], summary["stopped"]), (3000, 3, "max_rows"))
        self.assertIn("--max-rows", err)

    def test_runs_to_completion(self):
        summary, _, _ = self.purge([{"deleted": 1000, "more": True}, {"deleted": 3, "more": False}])
        self.assertEqual((summary["deleted"], summary["stopped"]), (1003, "done"))

    def test_default_max_rows(self):
        args = zqs_admin.parse_args(["purge", "--table", "commands", "--before", "2026-01-01"])
        self.assertEqual(args.max_rows, 20_000)


class ImportLedger(unittest.TestCase):
    """At most 25 charges per request; a 503 suggests a smaller chunk."""

    def charges_file(self, folder, count):
        path = Path(folder) / "ai-budget.json"
        path.write_text(json.dumps([{"id": f"c{i}", "at": 1_795_000_000, "usd": 0.001} for i in range(count)]),
                        encoding="utf-8")
        return str(path)

    def test_chunks_of_25(self):
        self.assertEqual(zqs_admin.parse_args(["import-ledger", "--file", "x"]).chunk, 25)
        with tempfile.TemporaryDirectory() as folder:
            path = self.charges_file(folder, 30)
            with self.assertRaises(zqs_admin.AdminError):
                zqs_admin.cmd_import(FakeClient([]), argparse.Namespace(file=path, chunk=26, close=False))
            reply = {"status": "imported", "inserted": 25, "updated": 0, "skipped": 0}
            client = FakeClient([reply, dict(reply, inserted=5)])
            status, out, _ = run_quietly(zqs_admin.cmd_import, client,
                                         argparse.Namespace(file=path, chunk=25, close=False))
        self.assertEqual(status, 0)
        self.assertEqual([len(body["charges"]) for _, body in client.calls], [25, 5])
        self.assertEqual(json.loads(out)["inserted"], 30)

    def test_unavailable_ledger_suggests_a_smaller_chunk(self):
        with tempfile.TemporaryDirectory() as folder:
            path = self.charges_file(folder, 30)
            client = FakeClient([zqs_admin.AdminError("import-ledger: HTTP 503 ledger_unavailable",
                                                      "ledger_unavailable")])
            with self.assertRaises(zqs_admin.AdminError) as caught:
                run_quietly(zqs_admin.cmd_import, client, argparse.Namespace(file=path, chunk=25, close=False))
        self.assertIn("--chunk 12", str(caught.exception))
        self.assertIn("idempotent", str(caught.exception))


class SmokeModel(unittest.TestCase):
    """The AI checks know the expected model even when the version check fails."""

    def make_tester(self):
        return live_smoke_test.LiveSmokeTester(base_url="https://zqsdev.com", ai_question="q", timeout=1.0,
                                               fail_fast=False)

    def test_a_version_mismatch_still_records_the_model(self):
        tester = self.make_tester()
        response = mock.Mock(status_code=200)
        response.json.return_value = {"version": "0.0.0", "commit": "abc", "ai_configured": True,
                                      "model": "gpt-6-luna-2026-11-01"}
        tester.session = mock.Mock()
        tester.session.get.return_value = response
        with self.assertRaises(AssertionError):
            tester.test_backend_version_endpoint()
        self.assertEqual(tester._expected_model(), "gpt-6-luna-2026-11-01")

    def test_without_a_version_reply_the_configured_model_is_expected(self):
        configured = live_smoke_test.configured_model()
        vars_line = next(line for line in (ROOT / "api" / "wrangler.jsonc").read_text(encoding="utf-8").splitlines()
                         if '"OPENAI_MODEL"' in line and not line.lstrip().startswith("//"))
        self.assertIn(f'"{configured}"', vars_line)
        tester = self.make_tester()
        self.assertEqual(tester._expected_model(), configured)
        reply = mock.Mock(status_code=200)
        reply.json.return_value = {"ai_enabled": True, "model": f"{configured}-2026-11-01", "sources": ["experiences"],
                                   "answer": "Alexandre leads agentic engineering at Studi, among other things."}
        tester.session = mock.Mock()
        tester.session.post.return_value = reply
        self.assertIn("Studi", tester.test_ai_endpoint())


if __name__ == "__main__":
    unittest.main()
