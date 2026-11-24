"""Operations scripts (admin client, cut-over check); no network access."""

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


if __name__ == "__main__":
    unittest.main()
