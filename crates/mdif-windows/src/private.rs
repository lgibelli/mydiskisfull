//! Files only the user running this may open: what mode 600 is on a Mac, as
//! an access-control list on Windows. For the ChatGPT sign-in's tokens.
//!
//! Made so, a file's list has one entry, the user's, inherited from nothing,
//! and the user owns it. Checked, every entry that lets anyone in must be the
//! user's: a file copied in from elsewhere, or one that took its folder's
//! list (SYSTEM, Administrators), isn't trusted. An administrator can still
//! take a file over, as root can on a Mac.

use std::ffi::c_void;
use std::io;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
    AddAccessAllowedAce, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
    GetLengthSid, GetTokenInformation, InitializeAcl, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::wide;

/// `ACCESS_ALLOWED_ACE_TYPE`: an entry that lets a SID in.
const ALLOWED: u8 = 0;
/// `ACCESS_DENIED_ACE_TYPE`: one that keeps a SID out.
const DENIED: u8 = 1;

/// The user this runs as: the token's `TOKEN_USER`, whose SID points into
/// the same buffer.
struct Me {
    buf: Vec<u64>,
}

impl Me {
    fn get() -> io::Result<Me> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the process pseudo-handle needs no closing; `token` gets a
        // handle closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut needed = 0u32;
        // SAFETY: asks for the size only.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
        let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        // SAFETY: `buf` holds at least `needed` bytes, aligned for TOKEN_USER.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut needed,
            )
        };
        let failed = io::Error::last_os_error();
        // SAFETY: opened above, closed once.
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(failed);
        }
        Ok(Me { buf })
    }

    fn sid(&self) -> PSID {
        // SAFETY: `buf` starts with the TOKEN_USER the call wrote.
        unsafe { (*self.buf.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// The user's SID as text ("S-1-5-21-…"), which names their folder in each
/// drive's Recycle Bin.
pub fn user_sid() -> io::Result<String> {
    let me = Me::get()?;
    let mut text: windows_sys::core::PWSTR = null_mut();
    // SAFETY: a valid SID, from the token; `text` gets a string freed below.
    if unsafe { ConvertSidToStringSidW(me.sid(), &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: NUL-ended text the call wrote, read up to its NUL, then freed once.
    let sid = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        sid
    };
    Ok(sid)
}

/// Makes the file at `path` the user's alone: owned by them, its list one
/// entry (theirs, everything allowed), inherited from nothing. A file moved
/// within its volume keeps it.
pub fn make_private(path: &Path) -> io::Result<()> {
    let me = Me::get()?;
    let sid = me.sid();
    // SAFETY: a valid SID, from the token.
    let sid_len = unsafe { GetLengthSid(sid) } as usize;
    // The list's header and one entry, whose SID starts at its last field.
    let size = (size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len)
        .next_multiple_of(4);
    let mut buf = vec![0u32; size / 4];
    let acl = buf.as_mut_ptr().cast::<ACL>();
    // SAFETY: `buf` is `size` bytes, aligned for an ACL.
    let built = unsafe {
        InitializeAcl(acl, size as u32, ACL_REVISION) != 0
            && AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) != 0
    };
    if !built {
        return Err(io::Error::last_os_error());
    }
    let name = wide(path.as_os_str());
    // SAFETY: `name` is NUL-ended; the SID and the list live for the call.
    let code = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            sid,
            null_mut(),
            acl,
            null(),
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    Ok(())
}

/// Whether only the user running this may open the file at `path`: they own
/// it, and every entry of its list that lets anyone in is theirs (a file
/// with no list lets everyone in).
pub fn private_to_me(path: &Path) -> io::Result<bool> {
    let me = Me::get()?;
    let name = wide(path.as_os_str());
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `name` is NUL-ended; the outputs point into `sd`, freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    // SAFETY: `owner` and `dacl` point into `sd`, alive until it's freed.
    let only = unsafe { only(me.sid(), owner, dacl) };
    // SAFETY: allocated by the call above, freed once.
    unsafe { LocalFree(sd) };
    Ok(only)
}

/// Whether `owner` is `me` and each entry of `dacl` that lets anyone in
/// lets in `me`.
///
/// # Safety
/// `me` is a valid SID; `owner` and `dacl` are null or valid.
unsafe fn only(me: PSID, owner: PSID, dacl: *const ACL) -> bool {
    // SAFETY: the caller's; each entry is read as its header says it is.
    unsafe {
        if owner.is_null() || dacl.is_null() || EqualSid(owner, me) == 0 {
            return false;
        }
        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        if GetAclInformation(
            dacl,
            (&raw mut info).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
        {
            return false;
        }
        (0..info.AceCount).all(|i| {
            let mut ace: *mut c_void = null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                return false;
            }
            match (*ace.cast::<ACE_HEADER>()).AceType {
                DENIED => true,
                ALLOWED => {
                    let entry = ace.cast::<ACCESS_ALLOWED_ACE>();
                    EqualSid((&raw mut (*entry).SidStart).cast(), me) != 0
                }
                // Object, callback and conditional entries: not told apart here.
                _ => false,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_sid_is_the_one_whoami_says() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        let out = std::process::Command::new(r"C:\Windows\System32\whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&out.stdout);
        assert!(line.contains(&format!("\"{sid}\"")), "{line} / {sid}");
    }

    #[test]
    fn a_private_file_is_its_users_alone_and_keeps_it_when_renamed_over() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("secret.json");
        std::fs::write(&f, b"{}").unwrap();
        // As made in the temp folder: its folder's list (SYSTEM, Administrators...).
        assert!(!private_to_me(&f).unwrap());
        make_private(&f).unwrap();
        assert!(private_to_me(&f).unwrap());
        // Still the user's to write and read.
        std::fs::write(&f, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"{\"a\":1}");
        // Saved as the sign-in saves it: a new file made private, renamed over.
        let tmp = dir.path().join(".secret.json.new");
        std::fs::write(&tmp, b"{\"a\":2}").unwrap();
        make_private(&tmp).unwrap();
        std::fs::rename(&tmp, &f).unwrap();
        assert!(private_to_me(&f).unwrap());
        assert_eq!(std::fs::read(&f).unwrap(), b"{\"a\":2}");
        assert!(private_to_me(&dir.path().join("missing")).is_err());
        assert!(make_private(&dir.path().join("missing")).is_err());
    }
}
