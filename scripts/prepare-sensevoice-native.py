#!/usr/bin/env python3
"""Prepare verified developer inputs. The desktop product never invokes Python."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / '.cache/sensevoice'


def valid(path, spec):
    if path.is_symlink() or not path.is_file() or path.stat().st_size != spec['size_bytes']:
        return False
    digest = hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest() == spec['sha256']


def private_folder(path):
    if path.is_symlink():
        raise RuntimeError('Refusing symlink in native input directory')
    if path != ROOT:
        private_folder(path.parent)
    path.mkdir(exist_ok=True, mode=0o700)


def download(path, spec, offline):
    if valid(path, spec):
        return
    if offline:
        raise RuntimeError('Missing or invalid cached input: ' + path.name)
    if path.is_symlink():
        raise RuntimeError('Refusing symlink input: ' + path.name)
    private_folder(path.parent)
    fd, temporary = tempfile.mkstemp(prefix='download-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as out, urllib.request.urlopen(spec['url'], timeout=30) as response:
            if response.status != 200 or not response.url.startswith('https://'):
                raise RuntimeError('Expected HTTPS 200 response')
            received = 0
            while block := response.read(1024 * 1024):
                received += len(block)
                if received > spec['size_bytes']:
                    raise RuntimeError('Native input exceeds pinned size')
                out.write(block)
        if not valid(Path(temporary), spec):
            raise RuntimeError('Native input hash or size mismatch: ' + path.name)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def prepare(offline=False, fixtures=False):
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise RuntimeError('SenseVoice native build inputs currently target Apple Silicon macOS')
    spec = json.loads((ROOT / 'resources/sensevoice-native.lock.json').read_text())
    if not valid(ROOT / 'vendor/sherpa-onnx/c-api.h', spec['header']):
        raise RuntimeError('Vendored C API header differs from the pinned native library')
    private_folder(CACHE)
    destination = CACHE / 'native' / spec['directory']
    if not all(valid(destination / 'lib' / item['filename'], item) for item in spec['libraries']):
        archive = CACHE / (spec['directory'] + '.tar.bz2')
        download(archive, spec['archive'], offline)
        private_folder(destination / 'lib')
        with tempfile.TemporaryDirectory(prefix='extract-', dir=CACHE) as temporary:
            expected = {spec['directory'] + '/lib/' + item['filename']: item for item in spec['libraries']}
            found = set()
            with tarfile.open(archive) as tar:
                for member in tar:
                    if member.isdir() and member.name in (spec['directory'], spec['directory'] + '/lib'):
                        continue
                    if not member.isfile() or member.name not in expected or member.name in found:
                        raise RuntimeError('Unexpected native archive entry')
                    item = expected[member.name]
                    if member.size != item['size_bytes']:
                        raise RuntimeError('Archive entry size mismatch')
                    found.add(member.name)
                    output = Path(temporary) / item['filename']
                    with tar.extractfile(member) as source, output.open('wb') as target:
                        shutil.copyfileobj(source, target, 1024 * 1024)
                    if not valid(output, item):
                        raise RuntimeError('Extracted library differs from pinned hash')
            if found != set(expected):
                raise RuntimeError('Missing library in native archive')
            for item in spec['libraries']:
                output = destination / 'lib' / item['filename']
                if output.is_symlink():
                    raise RuntimeError('Refusing symlink destination')
                os.replace(Path(temporary) / item['filename'], output)
    print('Verified sherpa-onnx ' + spec['sherpa_version'] + ' static CPU inputs')
    if fixtures:
        for item in json.loads((ROOT / 'resources/sensevoice-fixtures.lock.json').read_text())['files']:
            download(CACHE / 'model' / item['filename'], item, offline)
        print('Verified public SenseVoice model, tokens and speech fixtures')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--fixtures', action='store_true', help='also prepare the public model and speech files (about 241 MB)')
    args = parser.parse_args()
    prepare(args.offline, args.fixtures)
