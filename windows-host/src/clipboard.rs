//! Bounded Unicode text clipboard access for the host's sidecar connection.

use std::io;
use std::mem::size_of;
use std::ptr;
use std::slice;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};

const CF_UNICODETEXT: u32 = 13;
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

pub fn create_owner_window() -> io::Result<HWND> {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "PsychBeacon clipboard\0".encode_utf16().collect();
    unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .map_err(windows_error)
    }
}

pub fn destroy_owner_window(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn read_text(owner: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(owner)).ok()?;
        let _guard = ClipboardGuard;
        let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        if size < size_of::<u16>()
            || size > (MAX_CLIPBOARD_BYTES + size_of::<u16>()) * size_of::<u16>()
        {
            return None;
        }
        let ptr = GlobalLock(global).cast::<u16>();
        if ptr.is_null() {
            return None;
        }
        let count = size / size_of::<u16>();
        let units = slice::from_raw_parts(ptr, count);
        let length = units.iter().position(|unit| *unit == 0).unwrap_or(count);
        let text = String::from_utf16_lossy(&units[..length]);
        let _ = GlobalUnlock(global);
        (text.len() <= MAX_CLIPBOARD_BYTES).then_some(text)
    }
}

pub fn write_text(owner: HWND, text: &str) -> io::Result<()> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "clipboard text exceeds 1 MiB",
        ));
    }

    unsafe {
        OpenClipboard(Some(owner)).map_err(windows_error)?;
        let _guard = ClipboardGuard;
        EmptyClipboard().map_err(windows_error)?;

        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * size_of::<u16>();
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(windows_error)?;
        let destination = GlobalLock(global).cast::<u16>();
        if destination.is_null() {
            let _ = GlobalFree(Some(global));
            return Err(io::Error::last_os_error());
        }
        ptr::copy_nonoverlapping(wide.as_ptr(), destination, wide.len());
        let _ = GlobalUnlock(global);

        if let Err(error) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))) {
            let _ = GlobalFree(Some(global));
            return Err(windows_error(error));
        }
    }
    Ok(())
}

fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}
