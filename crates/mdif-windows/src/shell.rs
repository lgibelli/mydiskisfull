//! Moving to the Recycle Bin, as Explorer's Delete does: the shell's
//! `IFileOperation`, through COM. `windows-sys` has the functions and
//! constants but no interfaces, so the three used are declared here as their
//! vtables: `IFileOperation` and `IShellItem` called, and
//! `IFileOperationProgressSink` implemented. Their order is the SDK's
//! (`ShObjIdl_core.h`), as the `windows` crate declares them too.
//!
//! The shell deletes for good, without asking once it's told not to ask,
//! what it can't recycle: an item too big for the bin, a bin set to delete at
//! once, a drive with no bin. The sink sees each item before it goes and
//! refuses one the shell marks to be deleted for good (on a drive with no bin;
//! a folder too big for it, which goes item by item). Not every one is
//! marked: a file too big for the bin is marked to be recycled and then
//! deleted for good (seen on Windows 11), so `fs::trash` refuses what the
//! bin wouldn't take before the shell is asked. Between the two, nothing
//! should be deleted for good here: CI checks it with a file and a folder
//! too big for the bin.

use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{E_ABORT, E_NOINTERFACE, E_POINTER, S_OK};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NO_UI, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation,
    SHCreateItemFromParsingName, SIGDN, SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};

use crate::fs::hresult;

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_IFILE_OPERATION: GUID = GUID::from_u128(0x947aab5f_0a5c_4c13_b4d6_4bf7836fc9f8);
const IID_IFILE_OPERATION_PROGRESS_SINK: GUID =
    GUID::from_u128(0x04b0f1a7_9490_44bc_96e1_4296a31252e2);
const IID_ISHELL_ITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);

/// A COM interface pointer.
type Raw = *mut c_void;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
    add_ref: unsafe extern "system" fn(Raw) -> u32,
    release: unsafe extern "system" fn(Raw) -> u32,
}

/// `IFileOperation`; what isn't called is only held as its place.
#[repr(C)]
struct FileOperationVtbl {
    base: IUnknownVtbl,
    advise: unsafe extern "system" fn(Raw, Raw, *mut u32) -> HRESULT,
    unadvise: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    set_operation_flags: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    _set_progress_message: usize,
    _set_progress_dialog: usize,
    _set_properties: usize,
    _set_owner_window: usize,
    _apply_properties_to_item: usize,
    _apply_properties_to_items: usize,
    _rename_item: usize,
    _rename_items: usize,
    _move_item: usize,
    _move_items: usize,
    _copy_item: usize,
    _copy_items: usize,
    delete_item: unsafe extern "system" fn(Raw, Raw, Raw) -> HRESULT,
    _delete_items: usize,
    _new_item: usize,
    perform_operations: unsafe extern "system" fn(Raw) -> HRESULT,
    get_any_operations_aborted: unsafe extern "system" fn(Raw, *mut i32) -> HRESULT,
}

/// `IShellItem`, as far as `GetDisplayName` (what follows isn't used).
#[repr(C)]
struct ShellItemVtbl {
    base: IUnknownVtbl,
    _bind_to_handler: usize,
    _get_parent: usize,
    get_display_name: unsafe extern "system" fn(Raw, SIGDN, *mut PWSTR) -> HRESULT,
}

/// `IFileOperationProgressSink`, all of it: the shell calls each.
#[repr(C)]
struct SinkVtbl {
    base: IUnknownVtbl,
    start_operations: unsafe extern "system" fn(Raw) -> HRESULT,
    finish_operations: unsafe extern "system" fn(Raw, HRESULT) -> HRESULT,
    pre_rename_item: unsafe extern "system" fn(Raw, u32, Raw, PCWSTR) -> HRESULT,
    post_rename_item: unsafe extern "system" fn(Raw, u32, Raw, PCWSTR, HRESULT, Raw) -> HRESULT,
    pre_move_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, PCWSTR) -> HRESULT,
    post_move_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, PCWSTR, HRESULT, Raw) -> HRESULT,
    pre_copy_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, PCWSTR) -> HRESULT,
    post_copy_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, PCWSTR, HRESULT, Raw) -> HRESULT,
    pre_delete_item: unsafe extern "system" fn(Raw, u32, Raw) -> HRESULT,
    post_delete_item: unsafe extern "system" fn(Raw, u32, Raw, HRESULT, Raw) -> HRESULT,
    pre_new_item: unsafe extern "system" fn(Raw, u32, Raw, PCWSTR) -> HRESULT,
    post_new_item:
        unsafe extern "system" fn(Raw, u32, Raw, PCWSTR, PCWSTR, u32, HRESULT, Raw) -> HRESULT,
    update_progress: unsafe extern "system" fn(Raw, u32, u32) -> HRESULT,
    reset_timer: unsafe extern "system" fn(Raw) -> HRESULT,
    pause_timer: unsafe extern "system" fn(Raw) -> HRESULT,
    resume_timer: unsafe extern "system" fn(Raw) -> HRESULT,
}

/// The progress sink: a COM object (its vtable first), freed when the last
/// reference goes.
#[repr(C)]
struct Sink {
    vtbl: &'static SinkVtbl,
    refs: AtomicU32,
    /// An item the shell was about to delete for good: refused.
    refused: AtomicBool,
    /// An item deleted with nothing left in the bin (never expected: the
    /// refusal comes first).
    destroyed: AtomicBool,
    /// An item that went to the bin, and where, when the shell says.
    moved: AtomicBool,
    recycled: Mutex<Option<PathBuf>>,
}

static SINK_VTBL: SinkVtbl = SinkVtbl {
    base: IUnknownVtbl {
        query_interface,
        add_ref,
        release,
    },
    start_operations: carry_on,
    finish_operations,
    pre_rename_item: carry_on_named,
    post_rename_item,
    pre_move_item: carry_on_to,
    post_move_item: post_move_or_copy,
    pre_copy_item: carry_on_to,
    post_copy_item: post_move_or_copy,
    pre_delete_item,
    post_delete_item,
    pre_new_item: carry_on_named,
    post_new_item,
    update_progress,
    reset_timer: carry_on,
    pause_timer: carry_on,
    resume_timer: carry_on,
};

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// # Safety
/// `this` is a live [`Sink`].
unsafe fn sink<'a>(this: Raw) -> &'a Sink {
    // SAFETY: the caller's.
    unsafe { &*(this as *const Sink) }
}

unsafe extern "system" fn query_interface(this: Raw, iid: *const GUID, out: *mut Raw) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a valid IID and an out pointer it owns.
    unsafe {
        if same(&*iid, &IID_IUNKNOWN) || same(&*iid, &IID_IFILE_OPERATION_PROGRESS_SINK) {
            add_ref(this);
            *out = this;
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn add_ref(this: Raw) -> u32 {
    // SAFETY: COM calls it on a live sink.
    unsafe { sink(this) }.refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn release(this: Raw) -> u32 {
    // SAFETY: COM calls it on a live sink, once per reference it holds.
    let left = unsafe { sink(this) }.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: made by `Box::into_raw` in `recycle_here`; the last
        // reference just went.
        drop(unsafe { Box::from_raw(this as *mut Sink) });
    }
    left
}

unsafe extern "system" fn carry_on(_: Raw) -> HRESULT {
    S_OK
}

unsafe extern "system" fn finish_operations(_: Raw, _: HRESULT) -> HRESULT {
    S_OK
}

unsafe extern "system" fn carry_on_named(_: Raw, _: u32, _: Raw, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn post_rename_item(
    _: Raw,
    _: u32,
    _: Raw,
    _: PCWSTR,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}

unsafe extern "system" fn carry_on_to(_: Raw, _: u32, _: Raw, _: Raw, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn post_move_or_copy(
    _: Raw,
    _: u32,
    _: Raw,
    _: Raw,
    _: PCWSTR,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}

/// Before each item goes: one the shell wouldn't recycle (it would delete it
/// for good) is refused, which stops the operation.
unsafe extern "system" fn pre_delete_item(this: Raw, flags: u32, _: Raw) -> HRESULT {
    if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE as u32 == 0 {
        // SAFETY: the shell calls it on the sink it was given.
        unsafe { sink(this) }.refused.store(true, Ordering::Release);
        return E_ABORT;
    }
    S_OK
}

/// After each item: where it went in the bin (`recycled`, the item as it is
/// there), or, with none, that it was deleted for good.
unsafe extern "system" fn post_delete_item(
    this: Raw,
    _: u32,
    _: Raw,
    hr: HRESULT,
    recycled: Raw,
) -> HRESULT {
    // SAFETY: the shell calls it on the sink it was given.
    let s = unsafe { sink(this) };
    if hr >= 0 {
        if recycled.is_null() {
            s.destroyed.store(true, Ordering::Release);
        } else {
            s.moved.store(true, Ordering::Release);
            // SAFETY: a shell item the shell holds for the call.
            if let Some(path) = unsafe { file_system_path(recycled) } {
                *s.recycled.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
            }
        }
    }
    S_OK
}

// COM's signature.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn post_new_item(
    _: Raw,
    _: u32,
    _: Raw,
    _: PCWSTR,
    _: PCWSTR,
    _: u32,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}

unsafe extern "system" fn update_progress(_: Raw, _: u32, _: u32) -> HRESULT {
    S_OK
}

/// A shell item's file-system path, if it has one.
///
/// # Safety
/// `item` is a live `IShellItem`.
unsafe fn file_system_path(item: Raw) -> Option<PathBuf> {
    let mut name: PWSTR = null_mut();
    // SAFETY: a COM object starts with a pointer to its vtable; `name` is
    // valid for the call to write.
    let hr = unsafe {
        let vtbl = &**(item as *const *const ShellItemVtbl);
        (vtbl.get_display_name)(item, SIGDN_FILESYSPATH, &mut name)
    };
    if hr < 0 || name.is_null() {
        return None;
    }
    // SAFETY: on success the shell gives a NUL-ended string, freed here with
    // the allocator it came from.
    unsafe {
        let len = (0..).take_while(|&i| *name.add(i) != 0).count();
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(name, len)));
        CoTaskMemFree(name as *const c_void);
        Some(path)
    }
}

/// A reference to a COM object held here, let go when dropped.
struct Com(Raw);

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: every COM interface starts with IUnknown's; this
            // reference is ours to let go.
            unsafe { ((**(self.0 as *const *const IUnknownVtbl)).release)(self.0) };
        }
    }
}

fn check(hr: HRESULT) -> io::Result<()> {
    if hr < 0 { Err(hresult(hr)) } else { Ok(()) }
}

/// Moves `path` (absolute, a drive's: `C:\…`) to its drive's Recycle Bin and
/// says where it went there, or refuses what the shell marks to be deleted
/// for good. Not all of it: a file too big for the bin is marked to be
/// recycled, so [`crate::fs::trash`], the only caller outside tests, refuses
/// that first. On a thread of its own, so the caller's COM (if any) is left
/// as it is.
pub(crate) fn recycle(path: &Path) -> io::Result<PathBuf> {
    let text = crate::wide(path.as_os_str());
    // `C:\$Recycle.Bin`, the drive's bin.
    let text_path = path.to_string_lossy();
    let bin = (text_path.as_bytes().get(1) == Some(&b':'))
        .then(|| PathBuf::from(format!(r"{}\$Recycle.Bin", &text_path[..2])));
    std::thread::spawn(move || {
        // SAFETY: plain calls; a successful initialisation is undone below.
        let com = unsafe {
            CoInitializeEx(
                null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        check(com)?;
        // SAFETY: COM is initialised on this thread for the call.
        let done = unsafe { recycle_here(&text) };
        // SAFETY: pairs the successful `CoInitializeEx` above.
        unsafe { CoUninitialize() };
        // Moved, but the shell didn't name the item there: the bin, then.
        done?.map_or_else(
            || bin.ok_or_else(|| io::Error::other("the Recycle Bin didn't say where it went")),
            Ok,
        )
    })
    .join()
    .map_err(|_| io::Error::other("moving to the Recycle Bin failed"))?
}

/// [`recycle`]'s work: `Some(path)` in the bin, or `None` when the shell
/// moved it without naming where.
///
/// # Safety
/// COM is initialised on this thread; `text` is NUL-ended.
unsafe fn recycle_here(text: &[u16]) -> io::Result<Option<PathBuf>> {
    let mut item: Raw = null_mut();
    // SAFETY: `text` is NUL-ended; `item` receives an interface we then hold.
    check(unsafe {
        SHCreateItemFromParsingName(text.as_ptr(), null_mut(), &IID_ISHELL_ITEM, &mut item)
    })?;
    let item = Com(item);
    let mut op: Raw = null_mut();
    // SAFETY: as above, for the shell's file operation object.
    check(unsafe {
        CoCreateInstance(
            &FileOperation,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IFILE_OPERATION,
            &mut op,
        )
    })?;
    let op = Com(op);
    // SAFETY: `op` is a live IFileOperation; its vtable is the one declared.
    let ops = unsafe { &**(op.0 as *const *const FileOperationVtbl) };
    // To the bin (never deleted for good: `fs::trash` and the sink see to
    // it), with no window, question or sound, and stopping at the first
    // failure.
    let flags = FOF_ALLOWUNDO | FOF_NO_UI | FOFX_RECYCLEONDELETE | FOFX_EARLYFAILURE;
    // SAFETY: a call on the live object.
    check(unsafe { (ops.set_operation_flags)(op.0, flags) })?;
    let state = Box::into_raw(Box::new(Sink {
        vtbl: &SINK_VTBL,
        refs: AtomicU32::new(1),
        refused: AtomicBool::new(false),
        destroyed: AtomicBool::new(false),
        moved: AtomicBool::new(false),
        recycled: Mutex::new(None),
    })) as Raw;
    let sink_ref = Com(state);
    let mut cookie = 0u32;
    // SAFETY: calls on the live object; the sink and the item stay alive
    // (held here) until the operation is let go.
    let (performed, aborted) = unsafe {
        check((ops.advise)(op.0, sink_ref.0, &mut cookie))?;
        let mut hr = (ops.delete_item)(op.0, item.0, null_mut());
        if hr >= 0 {
            hr = (ops.perform_operations)(op.0);
        }
        let mut aborted = 0i32;
        let _ = (ops.get_any_operations_aborted)(op.0, &mut aborted);
        let _ = (ops.unadvise)(op.0, cookie);
        (hr, aborted != 0)
    };
    // SAFETY: the sink is alive: `sink_ref` holds it.
    let s = unsafe { sink(sink_ref.0) };
    if s.refused.load(Ordering::Acquire) {
        return Err(io::Error::other(
            "Windows would delete it for good, not move it to the Recycle Bin (there's no bin \
             for it, or it doesn't fit): it's left as it is",
        ));
    }
    if s.destroyed.load(Ordering::Acquire) {
        return Err(io::Error::other(
            "Windows deleted it for good instead of moving it to the Recycle Bin",
        ));
    }
    check(performed)?;
    if aborted || !s.moved.load(Ordering::Acquire) {
        return Err(io::Error::other("it wasn't moved to the Recycle Bin"));
    }
    Ok(s.recycled.lock().unwrap_or_else(|e| e.into_inner()).take())
}
