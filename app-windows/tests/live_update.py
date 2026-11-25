"""The app updating itself, as a person would have it: the release before the
latest is installed with its watcher, the app is opened, and its own Check Now
and Install and Restart (the window's `check_updates` and `install_update`)
are run in its window, through WebView2's remote debugging. The live feed is
read, the latest release is downloaded, checked and installed, and then it
must be the latest everywhere: mdif, the app opened again, the watcher back,
nothing left staged.

    python app-windows/tests/live_update.py x64|arm64

The live workflow (windows-live.yml) runs it on its runners: it installs for
the user running it and changes what starts at sign-in, so not on a PC of
your own.
"""
import json, os, subprocess, sys, time, urllib.request
import websocket  # websocket-client, installed by the workflow
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
PORT = 9229


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
subprocess.run([mdif, 'agent', 'install'], check=True)
wait('the watcher', lambda: agent()['running'])

# The app as sign-in opens it, its window's page open to remote debugging.
env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f'--remote-debugging-port={PORT}')
subprocess.Popen([exe, '--background'], env=env)
wait('the app to read the disk', lambda: os.path.isfile(announced))


def page():
    with urllib.request.urlopen(f'http://127.0.0.1:{PORT}/json', timeout=5) as r:
        return next(t for t in json.load(r) if t['type'] == 'page')


wait("the app's window", page)
ws = websocket.create_connection(page()['webSocketDebuggerUrl'], timeout=120)
asked = 0


def run(js):
    global asked
    asked += 1
    ws.send(json.dumps({'id': asked, 'method': 'Runtime.evaluate', 'params': {
        'expression': js, 'awaitPromise': True, 'returnByValue': True}}))
    while True:
        reply = json.loads(ws.recv())
        if reply.get('id') == asked:
            res = reply['result']
            if 'exceptionDetails' in res:
                sys.exit(f'{js}: {res["exceptionDetails"]}')
            return res['result'].get('value')


# Check Now: what the window shows comes from here.
found = run("window.__TAURI__.core.invoke('check_updates')")
print(f'Check Now found: {found}')
assert found and found['version'] == target, f'offered {found}, not {target}'
# Install and Restart. It ends the app once the installer is started, so
# what it returns may never come: an error it gives is kept and read back.
run("window.__TAURI__.core.invoke('install_update', {version: %s})"
    ".catch(e => { window.__installError = String(e); }); 'started'" % json.dumps(target))
end = time.time() + 120
while time.time() < end:
    try:
        err = run('window.__installError || null')
    except Exception:
        break  # the app has gone: the installer runs
    if err:
        sys.exit(f'Install and Restart: {err}')
    time.sleep(2)
else:
    sys.exit('the app was still open two minutes after Install and Restart')
ws.close()

wait(f'mdif {target}', lambda: mdif_version() == target, secs=600)
wait('the app to open again', lambda: running('MyDiskIsFull.exe'), secs=180)
wait('the watcher to be back', lambda: (a := agent())['running'] and not a['stopped']
     and not a['stopped_by_installer'], secs=180)
wait('the staged installer to be tidied', lambda: not any(
    '-updater-' in n for n in (os.listdir(staging) if os.path.isdir(staging) else [])), secs=180)
print(json.dumps(agent(), indent=2))
print(f'updated {start} -> {target}: mdif, the app and the watcher are {target}')
