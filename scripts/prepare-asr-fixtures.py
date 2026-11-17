"""Developer-only public fixture download. Never called by the application.

Downloads source-locked model bytes into the ignored cache. This is not the
signed production ModelManager and cannot establish release readiness.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / '.cache' / 'asr-fixtures'


def digest(path):
    hasher = hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(65536):
            hasher.update(block)
    return hasher.hexdigest()


def download(url, filename, size, sha256):
    dest = CACHE / filename
    if dest.exists() and dest.stat().st_size == size and digest(dest) == sha256:
        print(f'Verified cached {filename}', flush=True)
        return
    with tempfile.NamedTemporaryFile(dir=CACHE, suffix='.part', delete=False) as target:
        part = Path(target.name)
        try:
            print(f'Downloading {filename}', flush=True)
            request = urllib.request.Request(url, headers={'User-Agent': 'SpeakMore-fixture-preparation/0.1'})
            with urllib.request.urlopen(request, timeout=30) as response:
                if response.status != 200:
                    raise ValueError('Unexpected download status')
                total = 0
                while block := response.read(65536):
                    total += len(block)
                    if total > size:
                        raise ValueError('Artifact exceeds locked size')
                    target.write(block)
            target.flush()
            os.fsync(target.fileno())
            target.close()  # Windows must release the handle before the rename.
            if total != size or digest(part) != sha256:
                raise ValueError('Artifact integrity mismatch')
            os.replace(part, dest)
            print(f'Verified {filename}: {size} bytes', flush=True)
        finally:
            part.unlink(missing_ok=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--all-models', action='store_true')
    args = parser.parse_args()
    CACHE.mkdir(parents=True, exist_ok=True)
    lock = json.loads((ROOT / 'resources' / 'model-sources.lock.json').read_text())
    for model in lock['models']:
        if model['model_id'] == 'whisper-large-v3-turbo-q5_0' and not args.all_models:
            continue
        download(model['download_urls'][0], model['filename'], model['size_bytes'], model['sha256'])
    fixtures = json.loads((ROOT / 'resources' / 'asr-fixtures.lock.json').read_text())
    for fixture in fixtures['fixtures']:
        download(fixture['url'], fixture['filename'], fixture['size_bytes'], fixture['sha256'])
