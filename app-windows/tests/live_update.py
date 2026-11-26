"""The app updating itself, as a person would have it: the release before the
latest is installed, the app is opened, its welcome's Start and Get Started
pressed, and its own Check Now
and Install and Restart are pressed in its window, through UI Automation,
after a look at each page (a screenshot of each, none saying it couldn't). The live feed is
read, the latest release is downloaded, checked and installed, and then it
must be the latest everywhere: mdif, the app opened again, the watcher back,
nothing left staged.

    python app-windows/tests/live_update.py x64|arm64

The live workflow (windows-live.yml) runs it on its runners: it installs for
the user running it and changes what starts at sign-in, so not on a PC of
your own.
"""
import json, os, subprocess, sys, time
import live_ui as ui
from live_release import download, latest_and_previous, version

arch = sys.argv[1]
latest, previous = latest_and_previous(arch)
target, start = version(latest), version(previous)
print(f'updating {start} -> {target} ({arch})')

home = os.path.join(os.environ['LOCALAPPDATA'], 'MyDiskIsFull')
exe = os.path.join(home, 'MyDiskIsFull.exe')
mdif = os.path.join(home, 'mdif.exe')
staging = os.path.join(os.environ['LOCALAPPDATA'], 'io.github.lgibelli.mydiskisfull', 'updates')
announced = os.path.join(os.environ['APPDATA'], 'io.github.lgibelli.mydiskisfull', 'announced.json')


def wait(what, test, secs=120):
    end = time.time() + secs
    while True:
        try:
            if test():
                return
        except Exception as e:  # not there yet
            last = e
        else:
            last = None
        if time.time() > end:
            sys.exit(f'timed out waiting for {what}' + (f' ({last})' if last else ''))
        time.sleep(2)


def mdif_version():
    return subprocess.check_output([mdif, '--version'], text=True).split()[-1]


def agent():
    return json.loads(subprocess.check_output([mdif, 'agent', 'status', '--json']))


def running(image):
    out = subprocess.check_output(
        ['tasklist', '/FI', f'IMAGENAME eq {image}', '/FO', 'CSV', '/NH'], text=True)
    return any(l.startswith(f'"{image}"') for l in out.splitlines())


setup = download(previous, arch, os.environ['RUNNER_TEMP'])
subprocess.run([setup, '/S'], check=True, timeout=300)
assert mdif_version() == start, f'mdif {mdif_version()} after installing {start}'

# The app opened as a person opens it, and its window used through UI
# Automation (WebView2 shows the page's buttons to it by their names).
subprocess.Popen([exe])
wait('the app to read the disk', lambda: os.path.isfile(announced))
shots = os.path.join(os.environ['RUNNER_TEMP'], 'shots')
os.makedirs(shots, exist_ok=True)
window = ui.wait_window('MyDiskIsFull')
ui.shot(os.path.join(shots, f'{start}-welcome.png'))
# The welcome: the watcher started from it, then the pages.
ui.press(window, 'Start')
wait('the watcher', lambda: agent()['running'])
ui.press(window, 'Get Started')
# Each page, as the sidebar opens it: none may say it couldn't do something.
for page in ['Overview', 'Storage', 'Old Files', 'History', 'Prevention', 'Settings']:
    ui.press(window, page)
    time.sleep(4)
    ui.shot(os.path.join(shots, f'{start}-{page}.png'))
    failed = ui.named(window, lambda n: n.startswith("Couldn't"))
    assert not failed, f'{page}: {failed}'
# Settings → Updates: Check Now, then Install and Restart.
ui.press(window, 'Check Now')
ui.find(window, 'Install and Restart', secs=120)
ui.shot(os.path.join(shots, f'{start}-offered.png'))
# The app ends once the installer has started. It says it can't while the
# first measurement of the disk runs (ten minutes and more on a runner):
# pressed again until it can, for up to 20 minutes.
until = time.time() + 1200
attempt = 0
while True:
    attempt += 1
    ui.press(window, 'Install and Restart', secs=180)
    end = time.time() + 90
    while running('MyDiskIsFull.exe') and time.time() < end:
        time.sleep(3)
    if not running('MyDiskIsFull.exe'):
        break
    if not ui.alive(window):
        # Its window hidden: the app is stopping mdif and starting the installer.
        wait('the app to close for the installer', lambda: not running('MyDiskIsFull.exe'), 300)
        break
    said = ui.named(window, lambda n: n.startswith(("Couldn't", 'Not installed', 'A clean-up',
                                                   'mdif', 'The update')))
    if not running('MyDiskIsFull.exe'):
        break  # closing for the installer just then
    ui.shot(os.path.join(shots, f'{start}-install-{attempt}.png'))
    print(f'still open after Install and Restart ({attempt}): {sorted(set(said))}')
    if time.time() > until:
        sys.exit('the app never closed for the installer')
wait(f'mdif {target}', lambda: mdif_version() == target, secs=600)
wait('the app to open again', lambda: running('MyDiskIsFull.exe'), secs=180)
ui.shot(os.path.join(shots, f'{target}-after.png'))
wait('the watcher to be back', lambda: (a := agent())['running'] and not a['stopped']
     and not a['stopped_by_installer'], secs=180)
wait('the staged installer to be tidied', lambda: not any(
    '-updater-' in n for n in (os.listdir(staging) if os.path.isdir(staging) else [])), secs=180)
print(json.dumps(agent(), indent=2))
print(f'updated {start} -> {target}: mdif, the app and the watcher are {target}')
