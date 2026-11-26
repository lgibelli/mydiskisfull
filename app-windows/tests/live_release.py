"""The published releases the live checks start from (live_update.py,
live_storage_sense.py): the latest, the one before it that has a Windows
installer, and an installer downloaded and checked against the release's
SHA256SUMS-windows.txt. GH_TOKEN is the workflow's token, for the API's limit.
"""
import hashlib, json, os, urllib.request

REPO = 'lgibelli/mydiskisfull'


def api(path):
    req = urllib.request.Request(
        f'https://api.github.com/repos/{REPO}{path}',
        headers={'Accept': 'application/vnd.github+json',
                 'Authorization': f'Bearer {os.environ["GH_TOKEN"]}'})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def version(release):
    return release['tag_name'].removeprefix('v')


def key(v):
    return tuple(int(p) for p in v.split('.'))


def installer(release, arch):
    name = f'MyDiskIsFull_{version(release)}_{arch}-setup.exe'
    return next((a for a in release['assets'] if a['name'] == name), None)


def latest_and_previous(arch):
    """The latest release, and the newest published one before it with an
    installer for `arch`."""
    latest = api('/releases/latest')
    assert installer(latest, arch), f'{latest["tag_name"]} has no {arch} installer'
    assert any(a['name'] == 'latest.json' for a in latest['assets']), \
        f'{latest["tag_name"]} has no latest.json'
    older = [r for r in api('/releases?per_page=50')
             if not r['draft'] and not r['prerelease'] and installer(r, arch)
             and key(version(r)) < key(version(latest))]
    assert older, f'no release before {latest["tag_name"]} has a {arch} installer'
    return latest, max(older, key=lambda r: key(version(r)))


def fetch(url, to):
    with urllib.request.urlopen(url, timeout=300) as r, open(to, 'wb') as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)


def download(release, arch, folder):
    """The release's installer for `arch` in `folder`, its SHA-256 checked."""
    asset = installer(release, arch)
    assert asset, f'{release["tag_name"]} has no {arch} installer'
    path = os.path.join(folder, asset['name'])
    fetch(asset['browser_download_url'], path)
    sums = next(a for a in release['assets'] if a['name'] == 'SHA256SUMS-windows.txt')
    fetch(sums['browser_download_url'], path + '.sums')
    with open(path + '.sums') as f:
        want = {l.split()[1].lstrip('*'): l.split()[0] for l in f if l.strip()}
    with open(path, 'rb') as f:
        got = hashlib.sha256(f.read()).hexdigest()
    assert want.get(asset['name']) == got, f'{asset["name"]}: SHA-256 {got} is not the release\'s'
    print(f'downloaded {asset["name"]} ({got})')
    return path
