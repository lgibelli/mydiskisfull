"""The bin tweak doing what it says: with the latest release's mdif,
`recycle-bin-auto-empty` is applied, two files are sent to the Recycle Bin,
one of them dated as deleted 40 days ago, and Settings' Run Storage Sense
now is pressed. The old one must go and the new one stay.

    python app-windows/tests/live_storage_sense.py x64|arm64

The live workflow (windows-live.yml) runs it on its runners: it turns
Storage Sense on and empties part of the Recycle Bin, so not on a PC of your
own.
"""
import datetime, json, os, struct, subprocess, sys, time
import live_ui as ui
from live_release import download, latest_and_previous

arch = sys.argv[1]
latest, _ = latest_and_previous(arch)
setup = download(latest, arch, os.environ['RUNNER_TEMP'])
subprocess.run([setup, '/S'], check=True, timeout=300)
mdif = os.path.join(os.environ['LOCALAPPDATA'], 'MyDiskIsFull', 'mdif.exe')
db = os.path.join(os.environ['RUNNER_TEMP'], 'tweaks.sqlite3')


def ps(cmd):
    r = subprocess.run(['powershell', '-NoProfile', '-Command', cmd], capture_output=True, text=True)
    if r.returncode or r.stderr.strip():
        sys.exit(f'{cmd}\n{r.stdout}\n{r.stderr}')
    return r.stdout


subprocess.run([mdif, 'tweaks', 'apply', 'recycle-bin-auto-empty', '--db', db], check=True)
print(ps(r"Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\StorageSense"
         r"\Parameters\StoragePolicy' | Format-List | Out-String"))

folder = os.path.join(os.environ['USERPROFILE'], 'live-bin')
os.makedirs(folder, exist_ok=True)
files = {}
for name in ('old.bin', 'new.bin'):
    files[name] = os.path.join(folder, name)
    with open(files[name], 'wb') as f:
        f.write(os.urandom(1 << 20))
    ps("Add-Type -AssemblyName Microsoft.VisualBasic; "
       "[Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile("
       f"'{files[name]}', 'OnlyErrorDialogs', 'SendToRecycleBin')")
    assert not os.path.exists(files[name]), f'{name} was not moved to the bin'

sid = ps('[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value').strip()
bin_dir = os.path.join(os.environ['SystemDrive'] + '\\', '$Recycle.Bin', sid)


def binned():
    """Each binned file's original path -> its $I record (version 2: a
    version, the size, the FILETIME it was deleted at, then the path)."""
    out = {}
    for n in os.listdir(bin_dir):
        if n.startswith('$I'):
            with open(os.path.join(bin_dir, n), 'rb') as f:
                raw = f.read()
            if struct.unpack_from('<q', raw)[0] == 2:
                length = struct.unpack_from('<i', raw, 24)[0]
                path = raw[28:28 + 2 * length].decode('utf-16-le').rstrip('\0')
                out[path.lower()] = os.path.join(bin_dir, n)
    return out


now = binned()
assert all(p.lower() in now for p in files.values()), f'not in the bin: {files} vs {now}'
# Deleted 40 days ago, as far as the bin knows.
old = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=40)
filetime = int((old - datetime.datetime(1601, 1, 1, tzinfo=datetime.timezone.utc))
               .total_seconds() * 10_000_000)
with open(now[files['old.bin'].lower()], 'r+b') as f:
    f.seek(16)
    f.write(struct.pack('<q', filetime))

# Run now, as Settings → System → Storage → Storage Sense offers it: its
# scheduled task waits for an idle PC and a sign-out the runner can't give.
shots = os.path.join(os.environ['RUNNER_TEMP'], 'shots')
os.makedirs(shots, exist_ok=True)
# A runner new to Windows 11 shows its first sign-in's privacy screen over
# everything: closed (it runs in WWAHost.exe), and not shown again.
subprocess.run(['taskkill', '/f', '/im', 'WWAHost.exe'], capture_output=True)
subprocess.run(['reg', 'add', r'HKLM\SOFTWARE\Policies\Microsoft\Windows\OOBE', '/v',
                'DisablePrivacyExperience', '/t', 'REG_DWORD', '/d', '1', '/f'], capture_output=True)
# The runner's image turns Windows' Storage Service off, and with it Storage
# Sense ("An administrator has disabled the Storage Service"): on again.
print(ps("Get-Service StorSvc | Format-List Name,Status,StartType | Out-String; "
         r"Get-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\StorageSense' "
         "-ErrorAction SilentlyContinue | Format-List | Out-String"))
ps("Set-Service StorSvc -StartupType Manual; Start-Service StorSvc")
os.startfile('ms-settings:storagepolicies')
window = ui.wait_window('Settings')
time.sleep(5)
ui.shot(os.path.join(shots, 'storage-sense.png'))
# Storage's own page first, where Storage Sense is a row that opens its page.
ui.press(window, 'Storage Sense', secs=120)
time.sleep(3)
ui.shot(os.path.join(shots, 'storage-sense-page.png'))
ui.press(window, 'Run Storage Sense now', secs=60)
end = time.time() + 600
while files['old.bin'].lower() in binned():
    if time.time() > end:
        ui.shot(os.path.join(shots, 'storage-sense-after.png'))
        sys.exit('Storage Sense left the file deleted 40 days ago in the bin')
    time.sleep(5)
ui.shot(os.path.join(shots, 'storage-sense-after.png'))
assert files['new.bin'].lower() in binned(), 'Storage Sense also emptied the file deleted today'
print(json.dumps({'emptied': 'old.bin', 'kept': 'new.bin'}))
