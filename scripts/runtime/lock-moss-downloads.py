"""Refresh the checked-in, SHA-256-pinned macOS ARM64 MOSS download manifest."""
import concurrent.futures, json, pathlib, urllib.request
from packaging.tags import cpython_tags, compatible_tags, mac_platforms
from packaging.utils import parse_wheel_filename

root = pathlib.Path(__file__).resolve().parents[2]
platforms = list(mac_platforms((14, 0), 'arm64'))
tags = list(cpython_tags((3, 12), platforms=platforms)) + list(compatible_tags((3, 12), interpreter='cp312', platforms=platforms))
rank = {tag: i for i, tag in reversed(list(enumerate(tags)))}
def get(url):
    with urllib.request.urlopen(url, timeout=45) as response:
        return json.load(response)
def wheel(line):
    name, version = line.split('==')
    release = get(f'https://pypi.org/pypi/{name}/{version}/json')
    candidates = []
    for item in release['urls']:
        if item['packagetype'] != 'bdist_wheel' or item.get('yanked'): continue
        matching = [rank[t] for t in parse_wheel_filename(item['filename'])[3] if t in rank]
        if matching: candidates.append((min(matching), item))
    if not candidates: raise RuntimeError(f'No macOS 14 ARM64 Python 3.12 wheel: {line}')
    item = min(candidates, key=lambda c: c[0])[1]
    return dict(name=item['filename'], url=item['url'], sha256=item['digests']['sha256'], size=item['size'])
lines = [s.strip() for s in (root/'resources/moss-runtime.lock.txt').read_text().splitlines() if '==' in s and not s.startswith(('pip==', 'setuptools=='))]
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    wheels = list(pool.map(wheel, lines))
release = get('https://api.github.com/repos/astral-sh/python-build-standalone/releases/tags/20250712')
name = 'cpython-3.12.11+20250712-aarch64-apple-darwin-install_only_stripped.tar.gz'
asset = next(a for a in release['assets'] if a['name'] == name)
digest = asset.get('digest', '')
if not digest.startswith('sha256:'): raise RuntimeError('Python release is missing its SHA-256 digest')
manifest = dict(version='moss-macos-arm64-v1', python=dict(name=name, url=asset['browser_download_url'], sha256=digest[7:], size=asset['size']), wheels=wheels)
(root/'resources/moss-runtime-downloads.lock.json').write_text(json.dumps(manifest, indent=2)+'\n')
print(f'Pinned Python + {len(wheels)} wheels; {sum(a["size"] for a in [manifest["python"], *wheels])/1e6:.1f} MB download')
