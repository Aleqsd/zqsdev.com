#!/usr/bin/env python3
import json
import os
import urllib.request

URL = os.environ.get("VERSION_URL", "https://www.zqsdev.com/api/version")

def main() -> None:
    try:
        with urllib.request.urlopen(URL, timeout=10) as resp:
            data = json.load(resp)
        version = data.get("version", "?")
        commit = data.get("commit", "?")
        print(f"Remote version ({URL}): v{version} (commit {commit})")
    except Exception as exc:  # noqa: BLE001
        print(f"Failed to fetch {URL}: {exc}")
        raise SystemExit(1)

if __name__ == "__main__":
    main()
