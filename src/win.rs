//! Small Windows-specific touches.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};

/// Dark title bar whose color matches the canvas, so the window reads as one surface.
pub fn style_titlebar(window: &impl HasWindowHandle, rgb: [u8; 3]) {
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let hwnd = h.hwnd.get() as *mut core::ffi::c_void;
    let dark: i32 = 1;
    let colorref: u32 = rgb[0] as u32 | (rgb[1] as u32) << 8 | (rgb[2] as u32) << 16;
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const _ as _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, &colorref as *const _ as _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &colorref as *const _ as _, 4);
    }
}

/// Open Explorer with the file selected.
pub fn reveal_in_explorer(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    // Explorer wants the quotes around the path only, not around the whole switch.
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// Start a process fully detached: no console, and crucially no inherited
/// handles. `std::process::Command` lets the child inherit the caller's
/// stdout pipe, which makes shells (and agents) wait for the viewer to exit.
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW,
    };
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
    let mut line = quote(&exe.display().to_string());
    for a in args {
        line.push(' ');
        line.push_str(&quote(a));
    }
    let mut cmd: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    // Run from the exe's folder so the viewer never holds the caller's directory open.
    let dir: Vec<u16> = exe.parent().map(|d| d.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect()).unwrap_or_default();
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT,
            std::ptr::null(),
            if dir.is_empty() { std::ptr::null() } else { dir.as_ptr() },
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
    Ok(())
}

/// Error dialog for failures before (or outside) the window.
pub fn message_box(msg: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), w(msg).as_ptr(), w("Flick").as_ptr(), MB_OK | MB_ICONERROR) };
}

/// Stop the display from sleeping or dimming while a video plays.
pub fn keep_awake(on: bool) {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    let flags = if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    unsafe { SetThreadExecutionState(flags) };
}
