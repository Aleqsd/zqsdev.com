#!/usr/bin/env python3
"""Local stand-in for OpenAI Chat Completions, for `wrangler dev` of the zqsdev-api Worker.

    python3 scripts/openai_stub.py --port 9911
    cd api && npx wrangler dev --port 8788 --var OPENAI_BASE_URL:http://127.0.0.1:9911/v1

POST /v1/chat/completions answers like OpenAI: a JSON-object answer citing "projects", the
requested model echoed back and a `usage` block, so nothing is paid and no real key is needed
(the Worker only accepts this base URL on 127.0.0.1 or localhost). The question, the last
message, can trigger failures:

    __429__        HTTP 429, as when OpenAI rate-limits
    __slow__       a normal answer after 17 s (the API stops waiting after 15 s)
    __badsource__  a 2xx answer citing an unknown source ID
    __big__        a 2xx body of about 100 KB (the API refuses replies above 64 KiB)
    __redirect__   HTTP 307 to /v1/redirected on this stub (the API must not follow it)

GET /__stats returns the number of completion requests, how many carried a bearer token,
with --require-key-prefix how many carried another key (refused with 401), and how many requests
followed a __redirect__ answer to /v1/redirected (it must stay 0: a followed redirect would carry
the bearer key to whatever host the Location names). The token itself is never stored or printed.
"""

from __future__ import annotations

import argparse
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SLOW_SECONDS = 17
BIG_PADDING_BYTES = 100_000
COMPLETION_TOKENS = 24
STUB_ANSWER = (
    "Local stub answer: Alexandre currently works at Studi. The ZQSDev Terminal is written in "
    "Rust, compiled to WebAssembly, and its AI concierge uses OpenAI GPT-6 Luna."
)


class Stats:
    lock = threading.Lock()
    requests = 0
    with_bearer = 0
    foreign_keys = 0
    redirect_followed = 0
    magic: dict[str, int] = {}

    @classmethod
    def record(cls, bearer: bool, magic: str | None) -> None:
        with cls.lock:
            cls.requests += 1
            cls.with_bearer += int(bearer)
            if magic:
                cls.magic[magic] = cls.magic.get(magic, 0) + 1

    @classmethod
    def count_foreign_key(cls) -> None:
        with cls.lock:
            cls.foreign_keys += 1

    @classmethod
    def count_redirect_followed(cls) -> None:
        with cls.lock:
            cls.redirect_followed += 1

    @classmethod
    def snapshot(cls) -> dict:
        with cls.lock:
            return {"requests": cls.requests, "with_bearer": cls.with_bearer,
                    "foreign_keys": cls.foreign_keys, "redirect_followed": cls.redirect_followed,
                    "magic": dict(cls.magic)}


def completion(model: str, content: str, prompt_tokens: int) -> dict:
    return {
        "id": f"chatcmpl-stub-{time.time_ns()}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": content, "refusal": None},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": COMPLETION_TOKENS,
            "total_tokens": prompt_tokens + COMPLETION_TOKENS,
            "prompt_tokens_details": {"cached_tokens": 0, "audio_tokens": 0},
            "completion_tokens_details": {"reasoning_tokens": 0, "audio_tokens": 0},
        },
    }


def magic_of(question: str) -> str | None:
    for magic in ("__429__", "__slow__", "__badsource__", "__big__", "__redirect__"):
        if magic in question:
            return magic
    return None


class Handler(BaseHTTPRequestHandler):
    server_version = "openai-stub/1.0"
    quiet = False
    required_key_prefix = ""

    def log_message(self, fmt: str, *args) -> None:  # noqa: D401 - http.server API
        if not self.quiet:
            sys.stderr.write("openai-stub: " + (fmt % args) + "\n")

    def do_GET(self) -> None:  # noqa: N802 - http.server API
        if self.path == "/__stats":
            self.reply(200, Stats.snapshot())
        else:
            self.reply(404, {"error": {"message": "Not found", "type": "invalid_request_error"}})

    def do_POST(self) -> None:  # noqa: N802 - http.server API
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length > 0 else b""
        if self.path == "/v1/redirected":
            # Only a client that followed a __redirect__ answer gets here.
            Stats.count_redirect_followed()
            content = json.dumps({"answer": STUB_ANSWER, "sources": ["projects"]})
            self.reply(200, completion("gpt-6-luna", content, max(1, len(raw) // 4)))
            return
        if self.path != "/v1/chat/completions":
            self.reply(404, {"error": {"message": "Not found", "type": "invalid_request_error"}})
            return
        authorization = self.headers.get("Authorization", "")
        bearer = authorization.startswith("Bearer ") and len(authorization) > len("Bearer ")
        try:
            body = json.loads(raw)
            messages = body.get("messages") or []
            question = str(messages[-1].get("content", "")) if messages else ""
            model = str(body.get("model") or "gpt-6-luna")
        except (ValueError, AttributeError):
            Stats.record(bearer, None)
            self.reply(400, {"error": {"message": "Invalid JSON body", "type": "invalid_request_error"}})
            return
        magic = magic_of(question)
        Stats.record(bearer, magic)
        if not bearer:
            self.reply(401, {"error": {"message": "Missing bearer token", "type": "invalid_request_error"}})
            return
        if self.required_key_prefix and not authorization[len("Bearer "):].startswith(self.required_key_prefix):
            # A real key reached the stub: refuse it, so a local test can never pass with one.
            Stats.count_foreign_key()
            self.reply(401, {"error": {"message": "Unexpected API key for the local stub", "type": "invalid_request_error"}})
            return
        if magic == "__429__":
            self.reply(429, {"error": {"message": "Rate limit reached (stub)", "type": "requests", "code": "rate_limit_exceeded"}})
            return
        if magic == "__redirect__":
            host, port = self.server.server_address[:2]
            self.reply(307, {"error": {"message": "Moved (stub)", "type": "redirect"}},
                       {"Location": f"http://{host}:{port}/v1/redirected"})
            return
        if magic == "__slow__":
            time.sleep(SLOW_SECONDS)
        sources = ["pinecone-fake"] if magic == "__badsource__" else ["projects"]
        content = json.dumps({"answer": STUB_ANSWER, "sources": sources})
        payload = completion(model, content, max(1, len(raw) // 4))
        if magic == "__big__":
            payload["stub_padding"] = "x" * BIG_PADDING_BYTES
        self.reply(200, payload)

    def reply(self, status: int, payload: dict, headers: dict | None = None) -> None:
        data = json.dumps(payload).encode("utf-8")
        try:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            for name, value in (headers or {}).items():
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            # The API stopped waiting (its timer or its reply cap): nothing left to send.
            pass


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Local OpenAI Chat Completions stub for zqsdev-api.")
    parser.add_argument("--host", default="127.0.0.1", help="Address to listen on (default: %(default)s)")
    parser.add_argument("--port", type=int, default=9911, help="Port to listen on (default: %(default)s)")
    parser.add_argument("--quiet", action="store_true", help="Do not log each request")
    parser.add_argument("--require-key-prefix", default="",
                        help="Answer 401 unless the bearer key starts with this (e.g. sk-local-)")
    args = parser.parse_args(argv)
    Handler.quiet = args.quiet
    Handler.required_key_prefix = args.require_key_prefix
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.daemon_threads = True
    print(f"openai-stub listening on http://{args.host}:{args.port}/v1", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
