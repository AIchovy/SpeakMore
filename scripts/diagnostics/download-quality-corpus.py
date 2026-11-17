"""Download only pinned public development splits for offline diagnostics.

Never called by the app. Dataset licenses/attribution are in the source lock;
the downloaded data are not application assets or the release acceptance set.
"""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / ".cache/asr-quality/downloads"
LOCK = ROOT / "resources/asr-quality-sources.lock.json"


def verify(path, record):
    if path.is_symlink() or not path.is_file():
        return False
    if path.stat().st_size != record["size_bytes"]:
        return False
    sha = hashlib.sha256()
    blob = hashlib.sha1(f"blob {record['size_bytes']}\0".encode())
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            sha.update(chunk)
            blob.update(chunk)
    return (sha.hexdigest() == record["sha256"] if "sha256" in record
            else blob.hexdigest() == record["git_blob_sha1"])


def main():
    if any(p.is_symlink() for p in [CACHE, *CACHE.parents]):
        raise ValueError("Cache must not traverse a symbolic link")
    CACHE.mkdir(parents=True, exist_ok=True)
    for record in json.loads(LOCK.read_text())["files"]:
        name = record["filename"]
        if Path(name).name != name:
            raise ValueError("Unexpected lock filename")
        dest = CACHE / name
        if verify(dest, record):
            print(f"Verified cached {name}", flush=True)
            continue
        if dest.exists() or dest.is_symlink():
            raise ValueError(f"Existing data do not match lock: {name}")
        with tempfile.NamedTemporaryFile(dir=CACHE, suffix=".part", delete=False) as target:
            part = Path(target.name)
            try:
                print(f"Downloading {name} ({record['size_bytes']} bytes)", flush=True)
                request = urllib.request.Request(record["url"], headers={
                    "User-Agent": "SpeakMore-public-corpus-diagnostic/1"
                })
                with urllib.request.urlopen(request, timeout=60) as response:
                    if response.status != 200:
                        raise ValueError("Unexpected HTTP status")
                    total = 0
                    while chunk := response.read(1024 * 1024):
                        total += len(chunk)
                        if total > record["size_bytes"]:
                            raise ValueError("Response exceeds locked size")
                        target.write(chunk)
                target.flush()
                os.fsync(target.fileno())
                target.close()
                if not verify(part, record):
                    raise ValueError(f"Integrity mismatch: {name}")
                os.replace(part, dest)
                print(f"Verified {name}", flush=True)
            finally:
                part.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
