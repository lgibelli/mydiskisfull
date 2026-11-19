//! Measurements for `docs/WINDOWS.md`, not tests: what a design waits on, asked
//! of a real Windows. Each prints `SPIKE …` lines and is ignored in a normal
//! run; the `windows-spikes` workflow runs them all on a runner:
//!
//! ```text
//! cargo test -p mdif-windows --test spikes --release -- --ignored --nocapture --test-threads=1
//! ```
#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, DuplicateTokenEx, ImpersonateLoggedOnUser,
    LUA_TOKEN, RevertToSelf, SecurityImpersonation, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_QUERY,
    TokenImpersonation,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_EXTD_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, FSCTL_READ_USN_JOURNAL,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, IO_COUNTERS, OpenProcessToken,
};

const MIB: usize = 1 << 20;

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn counters() -> IO_COUNTERS {
    // SAFETY: zeroed is a valid IO_COUNTERS; the pseudo handle needs no closing.
    unsafe {
        let mut io: IO_COUNTERS = std::mem::zeroed();
        assert!(
            GetProcessIoCounters(GetCurrentProcess(), &mut io) != 0,
            "GetProcessIoCounters: error {}",
            GetLastError()
        );
        io
    }
}

fn delta(label: &str, before: &IO_COUNTERS, after: &IO_COUNTERS, sent: usize) {
    let mb = |a: u64, b: u64| (b - a) as f64 / MIB as f64;
    println!(
        "SPIKE io {label}: sent {} MiB -> read {:.1} MiB ({} ops), write {:.1} MiB ({} ops), other {:.1} MiB ({} ops)",
        sent / MIB,
        mb(before.ReadTransferCount, after.ReadTransferCount),
        after.ReadOperationCount - before.ReadOperationCount,
        mb(before.WriteTransferCount, after.WriteTransferCount),
        after.WriteOperationCount - before.WriteOperationCount,
        mb(before.OtherTransferCount, after.OtherTransferCount),
        after.OtherOperationCount - before.OtherOperationCount,
    );
}

/// Does `GetProcessIoCounters` count network and pipe traffic as writes? It
/// decides whether a process "writing" a lot says anything about the disk.
#[test]
#[ignore = "a measurement, run by the windows-spikes workflow"]
fn io_counters_by_kind_of_traffic() {
    let total = 64 * MIB;
    let chunk = vec![0x5Au8; MIB];

    // A file on disk.
    let dir = tempfile::tempdir().unwrap();
    let before = counters();
    let mut f = std::fs::File::create(dir.path().join("big")).unwrap();
    for _ in 0..total / MIB {
        f.write_all(&chunk).unwrap();
    }
    f.sync_all().unwrap();
    delta("file", &before, &counters(), total);
    drop(f);

    // The same bytes over loopback TCP, both ends in this process.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let reader = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut sink = vec![0u8; MIB];
        let mut n = 0;
        while let Ok(k) = s.read(&mut sink) {
            if k == 0 {
                break;
            }
            n += k;
        }
        n
    });
    let before = counters();
    let mut s = TcpStream::connect(addr).unwrap();
    for _ in 0..total / MIB {
        s.write_all(&chunk).unwrap();
    }
    drop(s);
    let got = reader.join().unwrap();
    delta("tcp loopback (both ends here)", &before, &counters(), total);
    assert_eq!(got, total);

    // The same bytes down a pipe to a child process.
    let mut child = Command::new("findstr")
        .arg("zzzz-no-match")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let line: Vec<u8> = std::iter::repeat_n(b'a', 1023).chain(Some(b'\n')).collect();
    let lines = vec![line; 1024];
    let before = counters();
    for _ in 0..total / MIB {
        for l in &lines {
            stdin.write_all(l).unwrap();
        }
    }
    drop(stdin);
    delta("pipe to a child", &before, &counters(), total);
    child.wait().unwrap();

    // Reading the file back.
    let before = counters();
    let mut f = std::fs::File::open(dir.path().join("big")).unwrap();
    let mut buf = vec![0u8; MIB];
    while f.read(&mut buf).unwrap() > 0 {}
    delta("reading the file back", &before, &counters(), total);
}

/// One directory listing by `FileIdExtdDirectoryInfo` on an open handle.
struct Listed {
    name: std::ffi::OsString,
    end_of_file: i64,
    allocation: i64,
}

/// `Err` is the Win32 error of the open or of a listing call that failed for
/// any reason but reaching the end.
fn list_by_handle(dir: &Path) -> Result<Vec<Listed>, u32> {
    let p = wide(dir);
    // SAFETY: `p` is NUL-ended; the handle is checked and closed below.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let mut out = Vec::new();
    let mut buf = vec![0u64; 64 * 1024 / 8];
    let mut class = FileIdExtdDirectoryRestartInfo;
    loop {
        // SAFETY: `buf` is the length passed; the handle is open for listing.
        let ok = unsafe {
            GetFileInformationByHandleEx(h, class, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32)
        };
        class = FileIdExtdDirectoryInfo;
        if ok == 0 {
            // SAFETY: plain call.
            let e = unsafe { GetLastError() };
            if e != ERROR_NO_MORE_FILES {
                // SAFETY: the handle is ours, closed once.
                unsafe { CloseHandle(h) };
                return Err(e);
            }
            break;
        }
        let base = buf.as_ptr().cast::<u8>();
        let mut at = 0usize;
        loop {
            // SAFETY: the kernel filled whole records, each at an 8-byte offset.
            let e = unsafe { &*(base.add(at).cast::<FILE_ID_EXTD_DIR_INFO>()) };
            let n = e.FileNameLength as usize / 2;
            let name_at = at + std::mem::offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
            assert!(name_at + n * 2 <= buf.len() * 8, "a name past the buffer");
            // SAFETY: from the buffer itself (not from the one-unit field),
            // and the range was just checked to lie inside it.
            let name = std::ffi::OsString::from_wide(unsafe {
                std::slice::from_raw_parts(base.add(name_at).cast::<u16>(), n)
            });
            if name != "." && name != ".." {
                out.push(Listed {
                    name,
                    end_of_file: e.EndOfFile,
                    allocation: e.AllocationSize,
                });
            }
            if e.NextEntryOffset == 0 {
                break;
            }
            at += e.NextEntryOffset as usize;
        }
    }
    // SAFETY: the handle is ours, closed once.
    unsafe { CloseHandle(h) };
    Ok(out)
}

/// A folder that isn't a link (REPARSE_POINT) or a cloud placeholder (OFFLINE,
/// RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS): what the backend would enter.
fn enterable(attributes: u32) -> bool {
    attributes & 0x10 != 0 && attributes & (0x400 | 0x1000 | 0x4_0000 | 0x40_0000) == 0
}

/// The folders under `root` that would be entered, up to `max`, by `std`.
fn folders_under(root: &Path, max: usize) -> Vec<PathBuf> {
    let (mut found, mut todo) = (Vec::new(), vec![root.to_path_buf()]);
    while let Some(d) = todo.pop() {
        if found.len() >= max {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        found.push(d);
        for e in rd.flatten() {
            if let Ok(m) = e.metadata()
                && enterable(m.file_attributes())
            {
                todo.push(e.path());
            }
        }
    }
    found
}

/// Entries listed and time taken, listing each of `dirs` once with `std`
/// (a `DirEntry` and its metadata).
fn list_all_by_std(dirs: &[PathBuf]) -> (usize, Duration) {
    let (t, mut n) = (Instant::now(), 0);
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                n += 1;
                let _ = e.metadata();
            }
        }
    }
    (n, t.elapsed())
}

/// The same, from an open handle with `FileIdExtdDirectoryInfo`.
fn list_all_by_handle(dirs: &[PathBuf]) -> (usize, Duration) {
    let (t, mut n) = (Instant::now(), 0);
    for d in dirs {
        if let Ok(list) = list_by_handle(d) {
            n += list.len();
        }
    }
    (n, t.elapsed())
}

/// How fast is a listing from an open handle, against `std`'s? And does a
/// file still open for appending show its size?
#[test]
#[ignore = "a measurement, run by the windows-spikes workflow"]
fn listing_speed_and_stale_sizes() {
    // The same folders, each listed both ways, in alternating order so
    // neither always has the file cache warmed by the other.
    let dirs = folders_under(Path::new(r"C:\Windows"), 4000);
    println!("SPIKE list: {} folders under C:\\Windows", dirs.len());
    for round in 1..=4 {
        let (std, handle) = if round % 2 == 1 {
            let s = list_all_by_std(&dirs);
            (s, list_all_by_handle(&dirs))
        } else {
            let h = list_all_by_handle(&dirs);
            (list_all_by_std(&dirs), h)
        };
        for (what, (n, t)) in [("std::fs", std), ("handle", handle)] {
            println!(
                "SPIKE list {what} round {round}: {n} entries in {t:.2?} = {:.0}/s",
                n as f64 / t.as_secs_f64()
            );
        }
    }

    // A file being appended to, never closed.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("growing.log");
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(&vec![1u8; 4 * MIB]).unwrap();
    let by_std = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .metadata()
        .unwrap()
        .len();
    let by_handle = list_by_handle(dir.path()).unwrap();
    let e = &by_handle[0];
    let truth = std::fs::metadata(&p).unwrap().len();
    println!(
        "SPIKE stale size: written 4 MiB, open for write: std DirEntry says {by_std}, handle listing says EndOfFile {} AllocationSize {}, a stat says {truth} ({})",
        e.end_of_file,
        e.allocation,
        e.name.to_string_lossy()
    );
    f.write_all(&vec![1u8; 4 * MIB]).unwrap();
    let by_handle = list_by_handle(dir.path()).unwrap();
    let by_std = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .metadata()
        .unwrap()
        .len();
    println!(
        "SPIKE stale size: after 4 MiB more: std {by_std}, handle EndOfFile {} AllocationSize {}",
        by_handle[0].end_of_file, by_handle[0].allocation
    );
}

/// What does a link count cost per file, when the listing doesn't give it?
#[test]
#[ignore = "a measurement, run by the windows-spikes workflow"]
fn link_count_cost_per_file() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3000 {
        std::fs::write(dir.path().join(format!("f{i}")), b"x").unwrap();
    }
    let paths: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    // Opening for attributes only, as the backend does.
    let open = |p: &Path| {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | 0x0020_0000 | 0x0010_0000)
            .open(p)
            .unwrap()
    };
    let per = |t: Instant| t.elapsed().as_micros() as f64 / paths.len() as f64;

    let t = Instant::now();
    for p in &paths {
        drop(open(p));
    }
    println!(
        "SPIKE links: open and close alone: {:.1} us per file",
        per(t)
    );

    // The one query that gives the link count (the listing gives the rest).
    let t = Instant::now();
    let mut links = 0u64;
    for p in &paths {
        let f = open(p);
        // SAFETY: zeroed is a valid value of this plain struct; the handle is open.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as HANDLE, &mut info) };
        assert!(ok != 0, "{}: error {}", p.display(), unsafe {
            GetLastError()
        });
        links += u64::from(info.nNumberOfLinks);
    }
    println!(
        "SPIKE links: open + GetFileInformationByHandle: {:.1} us per file over {} files (sum {links})",
        per(t),
        paths.len()
    );
}

/// The volume device `\\.\C:` or its root folder `C:\`, opened with `access`.
fn volume(target: &str, access: u32) -> Result<HANDLE, u32> {
    let p = wide(Path::new(target));
    // SAFETY: `p` is NUL-ended.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        Err(unsafe { GetLastError() })
    } else {
        Ok(h)
    }
}

/// Try the journal as this user, reporting what each step says.
fn try_journal(who: &str) {
    // FILE_TRAVERSE is what lets a non-administrator open the volume for the
    // journal; the unprivileged read then returns what the caller may read.
    const FILE_TRAVERSE: u32 = 0x20;
    let accesses = [
        ("GENERIC_READ", 0x8000_0000u32),
        ("FILE_TRAVERSE", FILE_TRAVERSE),
        (
            "FILE_TRAVERSE | FILE_READ_ATTRIBUTES",
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES,
        ),
        ("FILE_READ_ATTRIBUTES", FILE_READ_ATTRIBUTES),
        ("no access (0)", 0),
    ];
    // The volume device as the control, then the root folder, which a
    // standard user may be allowed to use for the journal when the device is
    // denied.
    let targets = [r"\\.\C:", r"C:\"];
    for (target, (name, access)) in targets
        .into_iter()
        .flat_map(|t| accesses.iter().map(move |a| (t, *a)))
    {
        let h = match volume(target, access) {
            Ok(h) => h,
            Err(e) => {
                println!("SPIKE usn {who}: open {target} with {name}: error {e}");
                continue;
            }
        };
        println!("SPIKE usn {who}: open {target} with {name}: ok");
        let mut data: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
        let mut got = 0u32;
        // SAFETY: the buffers are the sizes passed; the handle is open.
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_QUERY_USN_JOURNAL,
                null(),
                0,
                (&raw mut data).cast(),
                size_of::<USN_JOURNAL_DATA_V0>() as u32,
                &mut got,
                null_mut(),
            )
        };
        if ok == 0 {
            println!(
                "SPIKE usn {who}: {target} {name}: FSCTL_QUERY_USN_JOURNAL error {}",
                unsafe { GetLastError() }
            );
            unsafe { CloseHandle(h) };
            continue;
        }
        println!(
            "SPIKE usn {who}: {target} {name}: query ok, journal id {:#x}, first {} next {} max {} MiB",
            data.UsnJournalID,
            data.FirstUsn,
            data.NextUsn,
            data.MaximumSize / MIB as u64
        );
        for (call, code) in [
            ("READ_USN_JOURNAL", FSCTL_READ_USN_JOURNAL),
            (
                "READ_UNPRIVILEGED_USN_JOURNAL",
                FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
            ),
        ] {
            let req = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: data.FirstUsn,
                ReasonMask: u32::MAX,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: data.UsnJournalID,
            };
            let mut out = vec![0u8; 64 * 1024];
            let mut got = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    h,
                    code,
                    (&raw const req).cast(),
                    size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                    out.as_mut_ptr().cast(),
                    out.len() as u32,
                    &mut got,
                    null_mut(),
                )
            };
            if ok == 0 {
                println!(
                    "SPIKE usn {who}: {target} {name}: {call} error {}",
                    unsafe { GetLastError() }
                );
            } else {
                println!(
                    "SPIKE usn {who}: {target} {name}: {call} ok, {got} bytes (first 8 are the next USN)"
                );
            }
        }
        unsafe { CloseHandle(h) };
    }
}

/// Can the NTFS change journal be read without administrator rights? Tried
/// as the runner's (administrator) user, and as the same user with a
/// restricted ("LUA") token, which is what a standard user's session has.
#[test]
#[ignore = "a measurement, run by the windows-spikes workflow"]
fn usn_journal_as_administrator_and_as_a_restricted_user() {
    try_journal("administrator");

    // SAFETY: plain calls; every handle is closed, and the thread reverts.
    unsafe {
        let mut process: HANDLE = null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut process,
        ) == 0
        {
            panic!("OpenProcessToken failed: error {}", GetLastError());
        }
        let mut restricted: HANDLE = null_mut();
        if CreateRestrictedToken(
            process,
            LUA_TOKEN | DISABLE_MAX_PRIVILEGE,
            0,
            null(),
            0,
            null(),
            0,
            null(),
            &mut restricted,
        ) == 0
        {
            panic!("CreateRestrictedToken failed: error {}", GetLastError());
        }
        let mut impersonation: HANDLE = null_mut();
        if DuplicateTokenEx(
            restricted,
            TOKEN_ALL_ACCESS,
            null(),
            SecurityImpersonation,
            TokenImpersonation,
            &mut impersonation,
        ) == 0
        {
            panic!("DuplicateTokenEx failed: error {}", GetLastError());
        }
        if ImpersonateLoggedOnUser(impersonation) == 0 {
            println!(
                "SPIKE usn: ImpersonateLoggedOnUser error {}",
                GetLastError()
            );
            return;
        }
        try_journal("restricted token");
        RevertToSelf();
        CloseHandle(impersonation);
        CloseHandle(restricted);
        CloseHandle(process);
    }
}
