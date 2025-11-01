#!/usr/bin/env python3
"""
Simple development HTTP server with friendlier error handling.
"""

from __future__ import annotations

import argparse
import errno
import http.server
import logging
import os
import socketserver
import sys
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parent.parent
LOG_PATH = ROOT_DIR / "server.log"


class ReuseTCPServer(socketserver.TCPServer):
    allow_reuse_address = True


class LoggingHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args) -> None:
        logging.info("%s - %s", self.client_address[0], format % args)

    def log_error(self, format: str, *args) -> None:
        logging.error("%s - %s", self.client_address[0], format % args)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Serve a static directory.")
    parser.add_argument("--host", default="0.0.0.0", help="Interface to bind (default: %(default)s)")
    parser.add_argument("--port", type=int, default=8765, help="Port to bind (default: %(default)s)")
    parser.add_argument(
        "--root",
        default="static",
        help="Directory to serve files from (default: %(default)s)",
    )
    return parser.parse_args()


def configure_logging() -> None:
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
        handlers=[
            logging.FileHandler(LOG_PATH, encoding="utf-8"),
            logging.StreamHandler(sys.stdout),
        ],
    )


def main() -> int:
    configure_logging()
    args = parse_args()

    if not os.path.isdir(args.root):
        logging.error("Directory not found: %s", args.root)
        return 1

    os.chdir(args.root)

    try:
        server = ReuseTCPServer((args.host, args.port), LoggingHTTPRequestHandler)
    except OSError as exc:
        if exc.errno == errno.EADDRINUSE:
            logging.error(
                "Port %s already in use. Stop the other server or re-run with --port.",
                args.port,
            )
            return 1
        raise

    url = f"http://{args.host}:{args.port}/"
    logging.info("Serving static files from %s at %s (Ctrl+C to stop)", os.getcwd(), url)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        logging.info("Stopping server…")
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
