#!/usr/bin/env python3
"""Builds the RAG bundle: chunks static data, writes SQLite metadata, and syncs Pinecone."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import requests

OPENAI_EMBEDDING_ENDPOINT = "https://api.openai.com/v1/embeddings"


@dataclasses.dataclass
class DocumentChunk:
    chunk_id: str
    source: str
    topic: str
    body: str
    checksum: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build SQLite + Pinecone RAG assets")
    parser.add_argument(
        "--data-dir",
        type=Path,
        default=Path("static/data"),
        help="Directory that contains the JSON knowledge files.",
    )
    parser.add_argument(
        "--sqlite-path",
        type=Path,
        default=Path("static/data/rag_chunks.db"),
        help="Path to the SQLite file that will store chunk metadata.",
    )
    parser.add_argument(
        "--pinecone-host",
        type=str,
        default=os.getenv("PINECONE_HOST"),
        help="Base Pinecone host URL (e.g. https://index-xxxx.svc.aped-1.pinecone.io).",
    )
    parser.add_argument(
        "--pinecone-namespace",
        type=str,
        default=os.getenv("PINECONE_NAMESPACE"),
        help="Optional Pinecone namespace for the vectors.",
    )
    parser.add_argument(
        "--pinecone-batch-size",
        type=int,
        default=32,
        help="Batch size for Pinecone upserts.",
    )
    parser.add_argument(
        "--chunk-size",
        type=int,
        default=900,
        help="Maximum characters per chunk before splitting.",
    )
    parser.add_argument(
        "--chunk-overlap",
        type=int,
        default=150,
        help="Character overlap between sequential chunks.",
    )
    parser.add_argument(
        "--skip-pinecone",
        action="store_true",
        help="Only refresh the SQLite bundle without calling Pinecone.",
    )
    parser.add_argument(
        "--embedding-model",
        type=str,
        default=os.getenv("OPENAI_EMBEDDING_MODEL", "text-embedding-3-small"),
        help="Embedding model id to use for OpenAI.",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    api_key = os.getenv("OPENAI_API_KEY")
    if not api_key and not args.skip_pinecone:
        sys.exit("OPENAI_API_KEY is required to build embeddings unless --skip-pinecone is set.")

    pinecone_key = os.getenv("PINECONE_API_KEY")
    if not args.skip_pinecone and not pinecone_key:
        sys.exit("PINECONE_API_KEY is required unless --skip-pinecone is set.")
    if not args.skip_pinecone and not args.pinecone_host:
        sys.exit("PINECONE_HOST must be provided (argument or env) unless --skip-pinecone.")

    chunks = build_chunks(args.data_dir, chunk_size=args.chunk_size, overlap=args.chunk_overlap)
    print(f"Discovered {len(chunks)} chunks from {args.data_dir}")

    existing_rows = load_existing_rows(args.sqlite_path)
    to_delete = sorted(set(existing_rows.keys()) - {chunk.chunk_id for chunk in chunks})
    to_refresh = [chunk for chunk in chunks if existing_rows.get(chunk.chunk_id) != chunk.checksum]

    if to_delete:
        print(f"Detected {len(to_delete)} stale chunk(s) to delete.")
    print(f"{len(to_refresh)} chunk(s) need fresh embeddings and upserts.")

    if to_refresh and args.skip_pinecone:
        print("Skipping Pinecone sync; SQLite will still be updated with the new content.")

    embeddings_client = (
        OpenAIEmbeddings(api_key, args.embedding_model) if not args.skip_pinecone else None
    )
    pinecone = (
        PineconeClient(
            base_url=args.pinecone_host,
            api_key=pinecone_key,
            namespace=args.pinecone_namespace,
            batch_size=args.pinecone_batch_size,
        )
        if not args.skip_pinecone
        else None
    )

    if pinecone:
        upsert_chunks(pinecone, embeddings_client, to_refresh)
        if to_delete:
            pinecone.delete(ids=to_delete)

    persist_sqlite(args.sqlite_path, chunks)
    print(f"SQLite bundle updated at {args.sqlite_path.resolve()}")


def build_chunks(data_dir: Path, chunk_size: int, overlap: int) -> List[DocumentChunk]:
    if not data_dir.exists():
        raise SystemExit(f"Data directory {data_dir} does not exist.")

    all_chunks: List[DocumentChunk] = []
    for json_path in sorted(data_dir.glob("*.json")):
        payload = json.loads(json_path.read_text(encoding="utf-8"))
        for base_id, topic, text in generate_documents(json_path.stem, payload):
            for idx, chunk_text in enumerate(split_text(text, chunk_size, overlap), start=1):
                chunk_id = f"{base_id}:{idx}"
                checksum = hashlib.sha256(chunk_text.encode("utf-8")).hexdigest()
                all_chunks.append(
                    DocumentChunk(
                        chunk_id=chunk_id,
                        source=json_path.name,
                        topic=topic,
                        body=chunk_text,
                        checksum=checksum,
                    )
                )
    return all_chunks


def generate_documents(source: str, payload) -> Iterable[Tuple[str, str, str]]:
    """Yield (base_id, topic, body) tuples for each logical document."""

    if isinstance(payload, list):
        for idx, entry in enumerate(payload, start=1):
            topic = guess_label(entry) or f"{source}-{idx}"
            body = render_body(entry)
            base_id = f"{source}-{slugify(topic)}"
            text = f"Source: {source}\nTopic: {topic}\n\n{body}".strip()
            yield base_id, topic, text
        return

    if isinstance(payload, dict):
        for key, value in payload.items():
            topic = str(key)
            body = render_body(value)
            base_id = f"{source}-{slugify(topic)}"
            text = f"Source: {source}\nTopic: {topic}\n\n{body}".strip()
            yield base_id, topic, text
        return

    text = f"Source: {source}\n\n{payload}"
    yield f"{source}-all", source, text


def split_text(text: str, chunk_size: int, overlap: int) -> List[str]:
    if len(text) <= chunk_size:
        return [text.strip()]

    chunks: List[str] = []
    start = 0
    end = chunk_size
    text_len = len(text)
    while start < text_len:
        chunk = text[start:end]
        chunks.append(chunk.strip())
        if end >= text_len:
            break
        start = max(0, end - overlap)
        end = min(text_len, start + chunk_size)
    return [chunk for chunk in chunks if chunk]


def guess_label(entry) -> Optional[str]:
    if isinstance(entry, dict):
        for key in ("title", "company", "name", "question", "label", "role"):
            value = entry.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    return None


def render_body(entry) -> str:
    if isinstance(entry, (dict, list)):
        return json.dumps(entry, ensure_ascii=False, indent=2)
    return str(entry)


def slugify(value: str) -> str:
    import re

    slug = re.sub(r"[^a-zA-Z0-9]+", "-", value).strip("-").lower()
    return slug or "entry"


def load_existing_rows(sqlite_path: Path) -> Dict[str, str]:
    if not sqlite_path.exists():
        return {}
    conn = sqlite3.connect(sqlite_path)
    conn.row_factory = sqlite3.Row
    try:
        cursor = conn.execute("SELECT id, checksum FROM rag_chunks")
        return {row["id"]: row["checksum"] for row in cursor.fetchall()}
    except sqlite3.OperationalError:
        return {}
    finally:
        conn.close()


def persist_sqlite(sqlite_path: Path, chunks: Sequence[DocumentChunk]) -> None:
    sqlite_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(sqlite_path)
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS rag_chunks (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            topic TEXT NOT NULL,
            body TEXT NOT NULL,
            checksum TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        """
    )
    now = datetime.now(timezone.utc).isoformat()
    with conn:
        conn.execute("DELETE FROM rag_chunks")
        conn.executemany(
            """
            INSERT INTO rag_chunks (id, source, topic, body, checksum, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
            """,
            [(chunk.chunk_id, chunk.source, chunk.topic, chunk.body, chunk.checksum, now) for chunk in chunks],
        )
    conn.close()


def upsert_chunks(
    pinecone: "PineconeClient",
    embeddings: Optional["OpenAIEmbeddings"],
    chunks: Sequence[DocumentChunk],
) -> None:
    if not chunks or not embeddings:
        print("No embeddings need to be refreshed.")
        return

    texts = [chunk.body for chunk in chunks]
    vectors = embeddings.embed(texts)
    payload = []
    for chunk, vector in zip(chunks, vectors):
        payload.append(
            {
                "id": chunk.chunk_id,
                "values": vector,
                "metadata": {
                    "source": chunk.source,
                    "topic": chunk.topic,
                    "checksum": chunk.checksum,
                },
            }
        )
    pinecone.upsert(payload)


class OpenAIEmbeddings:
    def __init__(self, api_key: str, model: str, batch_size: int = 32) -> None:
        self.api_key = api_key
        self.model = model
        self.batch_size = batch_size

    def embed(self, texts: Sequence[str]) -> List[List[float]]:
        vectors: List[List[float]] = []
        for batch in chunked(texts, self.batch_size):
            response = requests.post(
                OPENAI_EMBEDDING_ENDPOINT,
                headers={
                    "Authorization": f"Bearer {self.api_key}",
                    "Content-Type": "application/json",
                },
                json={"model": self.model, "input": batch},
                timeout=60,
            )
            if response.status_code >= 300:
                raise SystemExit(
                    f"OpenAI embedding request failed ({response.status_code}): {response.text}"
                )
            data = response.json()["data"]
            vectors.extend(item["embedding"] for item in data)
        return vectors


class PineconeClient:
    def __init__(self, base_url: str, api_key: str, namespace: Optional[str], batch_size: int) -> None:
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.namespace = namespace
        self.batch_size = batch_size

    def upsert(self, vectors: Sequence[Dict]) -> None:
        print(f"Upserting {len(vectors)} vector(s) to Pinecone...")
        for batch in chunked(vectors, self.batch_size):
            payload = {"vectors": batch}
            if self.namespace:
                payload["namespace"] = self.namespace
            response = requests.post(
                f"{self.base_url}/vectors/upsert",
                headers=self._headers(),
                json=payload,
                timeout=60,
            )
            if response.status_code >= 300:
                raise SystemExit(f"Pinecone upsert failed ({response.status_code}): {response.text}")

    def delete(self, ids: Sequence[str]) -> None:
        print(f"Deleting {len(ids)} vector(s) from Pinecone...")
        payload = {"ids": list(ids)}
        if self.namespace:
            payload["namespace"] = self.namespace
        response = requests.post(
            f"{self.base_url}/vectors/delete",
            headers=self._headers(),
            json=payload,
            timeout=60,
        )
        if response.status_code >= 300:
            raise SystemExit(f"Pinecone delete failed ({response.status_code}): {response.text}")

    def _headers(self) -> Dict[str, str]:
        return {
            "Api-Key": self.api_key,
            "Content-Type": "application/json",
        }


def chunked(sequence: Sequence, size: int) -> Iterable[Sequence]:
    for start in range(0, len(sequence), size):
        yield sequence[start : start + size]


if __name__ == "__main__":
    main()
