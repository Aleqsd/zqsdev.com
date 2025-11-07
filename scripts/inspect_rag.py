#!/usr/bin/env python3
"""Pretty-print stats from static/data/rag_chunks.db."""

from __future__ import annotations

import argparse
import sqlite3
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--db",
        type=Path,
        default=Path("static/data/rag_chunks.db"),
        help="Path to the SQLite bundle (default: %(default)s)",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=3,
        help="Number of random samples to display (default: %(default)s)",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if not args.db.exists():
        raise SystemExit(f"{args.db} is missing; run `make rag` first.")

    conn = sqlite3.connect(args.db)
    try:
        rows = conn.execute("SELECT COUNT(*) FROM rag_chunks").fetchone()[0]
        print(f"rows={rows}")
        for source, count in conn.execute(
            "SELECT source, COUNT(*) FROM rag_chunks GROUP BY source ORDER BY source"
        ):
            print(f"  {source}: {count}")
        if args.limit > 0:
            samples = conn.execute(
                "SELECT id, topic FROM rag_chunks ORDER BY RANDOM() LIMIT ?", (args.limit,)
            ).fetchall()
            print("sample rows:")
            for chunk_id, topic in samples:
                print(f"  {chunk_id} ({topic})")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
