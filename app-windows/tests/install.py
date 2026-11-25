"""The Windows installer, used as a person would: installed silently for this
user, then the app, the watcher, an update over the running watcher, and the
uninstall, each checked where Windows keeps it.

    python app-windows/tests/install.py MyDiskIsFull_<version>_<arch>-setup.exe

CI runs it on its runners (x64, and the ARM64 installer on Windows on ARM):
it installs for the user running it and changes what starts at sign-in, so
not on a PC of your own.
"""
import json, os, subprocess, sys, time, winreg
setup = os.path.abspath(sys.argv[1])
home = os.path.join(os.environ['LOCALAPPDATA'], 'MyDiskIsFull')
exe = os.path.join(home, 'MyDiskIsFull.exe')
mdif = os.path.join(home, 'mdif.exe')
RUN = r'Software\Microsoft\Windows\CurrentVersion\Run'
APPROVED = r'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'

def value(key, name):
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key) as k:
            return winreg.QueryValueEx(k, name)[0]
    except FileNotFoundError:
        return None

def agent():
    return json.loads(subprocess.check_output([mdif, 'agent', 'status', '--json']))

def watchers():
    out = subprocess.check_output(
        ['tasklist', '/FI', 'IMAGENAME eq mdif-watch.exe', '/FO', 'CSV', '/NH'], text=True)
    return {int(l.split('","')[1]) for l in out.splitlines() if l.startswith('"mdif-watch.exe"')}

def wait(what, test, secs=90):
    end = time.time() + secs
    while not test():
        if time.time() > end:
            sys.exit(f'timed out waiting for {what}')
        time.sleep(1)

def install():
    subprocess.run([setup, '/S'], check=True, timeout=300)

install()
for f in ['MyDiskIsFull.exe', 'mdif.exe', 'mdif-watch.exe', 'uninstall.exe']:
    assert os.path.isfile(os.path.join(home, f)), f'{f} was not installed'
subprocess.run([mdif, '--version'], check=True)

# The app reads the disk with the mdif beside it (what it announced
# is kept after each read), and opens at sign-in only once its
# welcome's Get Started is pressed, which a runner can't: until then
# it has set nothing up. Then the value Get Started writes, for the
# uninstall to remove.
app = subprocess.Popen([exe, '--background'])
announced = os.path.join(os.environ['APPDATA'], 'io.github.lgibelli.mydiskisfull', 'announced.json')
wait('the app to read the disk', lambda: os.path.isfile(announced))
assert value(RUN, 'MyDiskIsFull') is None, 'the app opens at sign-in before it was asked to'
with winreg.CreateKey(winreg.HKEY_CURRENT_USER, RUN) as k:
    winreg.SetValueEx(k, 'MyDiskIsFull', 0, winreg.REG_SZ, f'"{exe}" --background')

# The watcher, from the installed copy, then an update over it: the
# installer stops it to replace its program, and starts it again.
subprocess.run([mdif, 'agent', 'install', '--interval', '60'], check=True)
a = agent()
assert a['running'] and a['at_sign_in'] == 'yes' and home.lower() in a['command'].lower(), a
before = watchers()
assert len(before) == 1, before
install()
app.wait(timeout=60)  # closed by the installer, to be replaced
a = agent()
assert a['running'] and a['at_sign_in'] == 'yes', a
after = watchers()
assert len(after) == 1 and after != before, (before, after)

# Switched off in Task Manager's Startup apps, it stays off through
# an update: stopped to be replaced, and not started again.
with winreg.CreateKey(winreg.HKEY_CURRENT_USER, APPROVED) as k:
    winreg.SetValueEx(k, 'MyDiskIsFull Watcher', 0, winreg.REG_BINARY, bytes([3] + [0] * 11))
install()
a = agent()
assert not a['running'] and a['at_sign_in'] == 'off', a
assert not watchers()
subprocess.run([mdif, 'agent', 'install', '--interval', '60'], check=True)
assert agent()['running']

# Ended though it's set to start at sign-in, the watcher is started
# again by the app, as launchd starts the Mac's: seen ended at the
# app's first look, and still two minutes later.
ended = watchers()
assert len(ended) == 1, ended
subprocess.run(['taskkill', '/F', '/IM', 'mdif-watch.exe'], check=True)
wait('the watcher to end', lambda: not agent()['running'], 30)
assert not agent()['stopped'], 'ended is not stopped on purpose'
app = subprocess.Popen([exe, '--background'])
wait('the app to start the watcher again', lambda: agent()['running'], 300)
again = watchers()
assert len(again) == 1 and again != ended, (ended, again)
app.kill()

# Uninstalled (in place, so this waits for it): nothing starts at
# sign-in, no watcher runs, the programs are gone, the history stays.
subprocess.run([os.path.join(home, 'uninstall.exe'), '/S', '_?=' + home], check=True, timeout=300)
for name in ['MyDiskIsFull', 'MyDiskIsFull Watcher']:
    assert value(RUN, name) is None, f'{name} still starts at sign-in'
    assert value(APPROVED, name) is None, f"{name}'s Task Manager switch was left"
assert not watchers(), 'a watcher still runs'
for f in ['MyDiskIsFull.exe', 'mdif.exe', 'mdif-watch.exe']:
    assert not os.path.exists(os.path.join(home, f)), f'{f} was left'
assert os.path.isfile(os.path.join(home, 'history.sqlite3')), 'the history went with it'
print('installed, updated twice, the watcher started again, uninstalled: all as expected')
