#![windows_subsystem = "windows"]

use std::cell::{Cell, RefCell};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::{s, w, Error, Result, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, FreeLibrary, GetLastError, BOOL, COLORREF, ERROR_ALREADY_EXISTS, HMODULE, HWND,
    LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Globalization::{
    GetLocaleInfoW, LOCALE_SABBREVLANGNAME, LOCALE_SNATIVELANGNAME,
};
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, GetSysColor, InvalidateRect, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER, FF_SWISS, FW_SEMIBOLD, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{CreateMutexW, Sleep, CREATE_NO_WINDOW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, SendInput, HKL, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CAPITAL, VK_LMENU, VK_LSHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetClientRect,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, KillTimer,
    PostMessageW, PostThreadMessageW, RegisterClassW, RegisterWindowMessageW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, SetWindowTextW, SetWindowsHookExW,
    ShowWindow, TranslateMessage, UnhookWindowsHookEx, CS_HREDRAW, CS_VREDRAW, HHOOK, HWND_MESSAGE,
    HWND_TOPMOST, KBDLLHOOKSTRUCT, LWA_ALPHA, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, WH_KEYBOARD_LL, WM_ERASEBKGND, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN,
    WM_KEYUP, WM_PAINT, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const OVERLAY_TIMER_ID: usize = 1;
const PENDING_LANGUAGE_TIMER_ID: usize = 2;
const PENDING_LANGUAGE_DELAY: u32 = 50;
const ACTIVATION_GRACE: Duration = Duration::from_millis(200);
const OVERLAY_DURATION: Duration = Duration::from_millis(500);
const OVERLAY_FRAME_MS: u32 = 16;
const OVERLAY_MAX_ALPHA: f32 = 240.0;

static CAPS_LOCK_PRESSED: AtomicBool = AtomicBool::new(false);
static CAPS_MESSAGE: AtomicU32 = AtomicU32::new(0);
static SHELL_MESSAGE: AtomicU32 = AtomicU32::new(0);
static ACTIVATION_MESSAGE: AtomicU32 = AtomicU32::new(0);
static MESSAGE_TARGET: AtomicIsize = AtomicIsize::new(0);
static OVERLAY_ENABLED: OnceLock<bool> = OnceLock::new();
static SWITCH_METHOD: OnceLock<SwitchMethod> = OnceLock::new();

thread_local! {
    static OVERLAY: RefCell<Overlay> = RefCell::new(Overlay::default());
    static ACTIVATION: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST_LAYOUT: Cell<Option<HKL>> = const { Cell::new(None) };
}

enum SwitchMethod {
    WinApi,
    AltShift,
}

struct Overlay {
    window: HWND,
    language: String,
    started_at: Instant,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            window: HWND::default(),
            language: String::new(),
            started_at: Instant::now(),
        }
    }
}

fn read_config_value(name: &str) -> Option<String> {
    let config = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("config.toml")))
        .and_then(|path| fs::read_to_string(path).ok())?;

    config.lines().find_map(|line| {
        let line = line.split('#').next()?.trim();
        let (key, value) = line.split_once('=')?;

        (key.trim() == name).then(|| {
            value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
    })
}

fn read_switch_method() -> SwitchMethod {
    match read_config_value("method").as_deref() {
        Some("alt_shift") => SwitchMethod::AltShift,
        _ => SwitchMethod::WinApi,
    }
}

fn read_overlay_enabled() -> bool {
    !matches!(read_config_value("overlay").as_deref(), Some("false"))
}

fn read_autostart_enabled() -> bool {
    !matches!(read_config_value("autostart").as_deref(), Some("false"))
}

fn configure_autostart(enabled: bool) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };

    let mut command = Command::new("schtasks.exe");
    if enabled {
        command.args([
            "/Create",
            "/TN",
            "CapsSwitch",
            "/SC",
            "ONLOGON",
            "/RL",
            "HIGHEST",
            "/IT",
            "/TR",
        ]);
        command
            .arg(format!("\"{}\"", executable.display()))
            .arg("/F");
    } else {
        command.args(["/Delete", "/TN", "CapsSwitch", "/F"]);
    }

    let _ = command.creation_flags(CREATE_NO_WINDOW.0).status();
}

fn next_keyboard_layout() -> Option<(HWND, HKL)> {
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return None;
        }

        let thread_id = GetWindowThreadProcessId(window, None);
        let current = GetKeyboardLayout(thread_id);
        let count = GetKeyboardLayoutList(None);
        if count <= 1 {
            return None;
        }

        let mut layouts = vec![HKL::default(); count as usize];
        let count = GetKeyboardLayoutList(Some(&mut layouts)).max(0) as usize;
        layouts.truncate(count);
        if layouts.len() <= 1 {
            return None;
        }

        let next = layouts
            .iter()
            .position(|layout| layout.0 == current.0)
            .map(|index| layouts[(index + 1) % layouts.len()])
            .unwrap_or(layouts[0]);

        Some((window, next))
    }
}

fn locale_info(locale: usize, kind: u32) -> Option<String> {
    let mut buffer = [0u16; 128];
    unsafe {
        let length = GetLocaleInfoW(locale as u32, kind, Some(&mut buffer));
        if length > 1 {
            Some(String::from_utf16_lossy(&buffer[..length as usize - 1]))
        } else {
            None
        }
    }
}

fn language_name(layout: HKL) -> String {
    let locale = layout.0 as usize & 0xffff;

    if locale & 0x3ff == 0x09 {
        return String::from("ENG");
    }

    if let Some(native_name) = locale_info(locale, LOCALE_SNATIVELANGNAME) {
        if !native_name.is_ascii() {
            let abbreviated: String = native_name.chars().take(3).collect();
            if abbreviated.chars().count() == 3 {
                return abbreviated.to_uppercase();
            }
        }
    }

    locale_info(locale, LOCALE_SABBREVLANGNAME)
        .map(|name| name.to_uppercase())
        .unwrap_or_else(|| String::from("???"))
}

unsafe fn install_shell_hook() -> Result<(HMODULE, HHOOK)> {
    let path = std::env::current_exe()?.with_file_name("caps_switch_hook.dll");
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let module = LoadLibraryW(PCWSTR(path.as_ptr()))?;

    let Some(install) = GetProcAddress(module, s!("install_caps_switch_hook")) else {
        let _ = FreeLibrary(module);
        return Err(Error::from_win32());
    };

    let hook = HHOOK(install() as *mut core::ffi::c_void);
    if hook.0.is_null() {
        let _ = FreeLibrary(module);
        return Err(Error::from_win32());
    }

    Ok((module, hook))
}

fn trace_overlay(message: &str) {
    if let Ok(executable) = std::env::current_exe() {
        let _ = fs::write(executable.with_file_name("overlay-trace.txt"), message);
    }
}

fn schedule_language_overlay() {
    trace_overlay("schedule requested");
    if !*OVERLAY_ENABLED.get_or_init(read_overlay_enabled) {
        trace_overlay("overlay disabled");
        return;
    }

    let window = HWND(MESSAGE_TARGET.load(Ordering::Relaxed) as *mut core::ffi::c_void);
    if window.0.is_null() {
        return;
    }

    unsafe {
        SetTimer(
            window,
            PENDING_LANGUAGE_TIMER_ID,
            PENDING_LANGUAGE_DELAY,
            None,
        );
    }
}

fn current_keyboard_layout() -> Option<HKL> {
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return None;
        }

        let thread_id = GetWindowThreadProcessId(window, None);
        Some(GetKeyboardLayout(thread_id))
    }
}

fn switch_input_language_winapi() {
    let Some((window, next)) = next_keyboard_layout() else {
        return;
    };

    unsafe {
        if PostMessageW(
            window,
            WM_INPUTLANGCHANGEREQUEST,
            WPARAM(0),
            LPARAM(next.0 as isize),
        )
        .is_ok()
        {
            schedule_language_overlay();
        }
    }
}

fn key_event(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn switch_input_language_alt_shift() {
    let inputs = [
        key_event(VK_LMENU, KEYBD_EVENT_FLAGS(0)),
        key_event(VK_LSHIFT, KEYBD_EVENT_FLAGS(0)),
        key_event(VK_LSHIFT, KEYEVENTF_KEYUP),
        key_event(VK_LMENU, KEYEVENTF_KEYUP),
    ];

    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn switch_input_language() {
    match SWITCH_METHOD.get_or_init(read_switch_method) {
        SwitchMethod::WinApi => switch_input_language_winapi(),
        SwitchMethod::AltShift => switch_input_language_alt_shift(),
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn close_previous_instance() -> bool {
    unsafe {
        let Ok(window) = FindWindowW(w!("CapsSwitchMessageTarget"), None) else {
            return true;
        };

        let thread_id = GetWindowThreadProcessId(window, None);
        if thread_id == 0 {
            return false;
        }
        let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));

        for _ in 0..100 {
            if FindWindowW(w!("CapsSwitchMessageTarget"), None).is_err() {
                return true;
            }
            Sleep(20);
        }

        false
    }
}

fn create_message_target() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(overlay_window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("CapsSwitchMessageTarget"),
            ..Default::default()
        };

        if RegisterClassW(&class) == 0 {
            return Err(Error::from_win32());
        }

        CreateWindowExW(
            Default::default(),
            w!("CapsSwitchMessageTarget"),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            instance,
            None,
        )
    }
}

fn create_overlay() -> Result<HWND> {
    let existing = OVERLAY.with(|overlay| overlay.borrow().window);
    if !existing.0.is_null() {
        return Ok(existing);
    }

    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("CapsSwitchOverlay"),
            ..Default::default()
        };

        if RegisterClassW(&class) == 0 {
            return Err(Error::from_win32());
        }

        let window = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            w!("CapsSwitchOverlay"),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )?;

        OVERLAY.with(|overlay| overlay.borrow_mut().window = window);
        Ok(window)
    }
}

fn ease(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

fn overlay_alpha(elapsed: Duration) -> u8 {
    let milliseconds = elapsed.as_secs_f32() * 1000.0;
    let opacity = if milliseconds < 100.0 {
        ease(milliseconds / 100.0)
    } else if milliseconds < 400.0 {
        1.0
    } else {
        1.0 - ease((milliseconds - 400.0) / 100.0)
    };

    (opacity.clamp(0.0, 1.0) * OVERLAY_MAX_ALPHA).round() as u8
}

fn show_language_overlay(language: String) {
    trace_overlay("show requested");
    let window = match create_overlay() {
        Ok(window) => window,
        Err(error) => {
            trace_overlay(&format!("create overlay failed: {error:?}"));
            return;
        }
    };

    OVERLAY.with(|overlay| {
        let mut state = overlay.borrow_mut();
        state.language = language;
        state.started_at = Instant::now();
        let title = wide(&state.language);
        drop(state);

        unsafe {
            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            let width = screen_width / 5;
            let height = screen_height / 5;

            let _ = SetWindowTextW(window, PCWSTR(title.as_ptr()));
            let _ = SetLayeredWindowAttributes(window, COLORREF(0), 0, LWA_ALPHA);
            let _ = InvalidateRect(window, None, BOOL(1));
            let _ = SetWindowPos(
                window,
                HWND_TOPMOST,
                (screen_width - width) / 2,
                (screen_height - height) / 2,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );

            let radius = height / 8;
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
            if SetWindowRgn(window, region, BOOL(1)) == 0 {
                let _ = DeleteObject(region);
            }

            SetTimer(window, OVERLAY_TIMER_ID, OVERLAY_FRAME_MS, None);
        }
    });
}

fn update_overlay() {
    let Some((window, elapsed)) = OVERLAY.with(|overlay| {
        let state = overlay.borrow();
        (!state.window.0.is_null()).then_some((state.window, state.started_at.elapsed()))
    }) else {
        return;
    };

    unsafe {
        if elapsed >= OVERLAY_DURATION {
            let _ = KillTimer(window, OVERLAY_TIMER_ID);
            let _ = ShowWindow(window, SW_HIDE);
        } else {
            let _ =
                SetLayeredWindowAttributes(window, COLORREF(0), overlay_alpha(elapsed), LWA_ALPHA);
        }
    }
}

fn windows_accent_color() -> COLORREF {
    unsafe {
        let mut argb = 0;
        let mut opaque = BOOL(0);
        if DwmGetColorizationColor(&mut argb, &mut opaque).is_ok() {
            COLORREF(((argb >> 16) & 0xff) | (argb & 0xff00) | ((argb & 0xff) << 16))
        } else {
            COLORREF(GetSysColor(COLOR_WINDOW))
        }
    }
}

fn contrasting_text_color(background: COLORREF) -> COLORREF {
    let red = (background.0 & 0xff) as f32;
    let green = ((background.0 >> 8) & 0xff) as f32;
    let blue = ((background.0 >> 16) & 0xff) as f32;
    let luminance = 0.299 * red + 0.587 * green + 0.114 * blue;

    if luminance > 160.0 {
        COLORREF(0x000000)
    } else {
        COLORREF(0xffffff)
    }
}

fn paint_overlay(window: HWND) {
    let language = OVERLAY.with(|overlay| overlay.borrow().language.clone());

    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let device_context = BeginPaint(window, &mut paint);
        let mut rect = RECT::default();
        let _ = GetClientRect(window, &mut rect);

        let background = windows_accent_color();
        let brush = CreateSolidBrush(background);
        FillRect(device_context, &rect, brush);
        let _ = DeleteObject(brush);

        let height = rect.bottom - rect.top;
        let font = CreateFontW(
            -height / 3,
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            w!("Segoe UI"),
        );
        let previous_font = SelectObject(device_context, font);
        SetBkMode(device_context, TRANSPARENT);
        SetTextColor(device_context, contrasting_text_color(background));

        let mut text: Vec<u16> = language.encode_utf16().collect();
        DrawTextW(
            device_context,
            &mut text,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        SelectObject(device_context, previous_font);
        let _ = DeleteObject(font);
        let _ = EndPaint(window, &paint);
    }
}

unsafe extern "system" fn overlay_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let caps_message = CAPS_MESSAGE.load(Ordering::Relaxed);
    if caps_message != 0 && message == caps_message {
        switch_input_language();
        return LRESULT(0);
    }

    let shell_message = SHELL_MESSAGE.load(Ordering::Relaxed);
    if shell_message != 0 && message == shell_message {
        let should_schedule = ACTIVATION.with(|activation| {
            activation
                .get()
                .is_some_and(|activated_at| activated_at.elapsed() > ACTIVATION_GRACE)
        });

        if should_schedule {
            schedule_language_overlay();
        }
        return LRESULT(0);
    }

    let activation_message = ACTIVATION_MESSAGE.load(Ordering::Relaxed);
    if activation_message != 0 && message == activation_message {
        let _ = KillTimer(window, PENDING_LANGUAGE_TIMER_ID);
        ACTIVATION.with(|activation| activation.set(Some(Instant::now())));
        if let Some(layout) = current_keyboard_layout() {
            LAST_LAYOUT.with(|last| last.set(Some(layout)));
        }
        return LRESULT(0);
    }

    match message {
        WM_PAINT => {
            paint_overlay(window);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER if wparam.0 == OVERLAY_TIMER_ID => {
            update_overlay();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == PENDING_LANGUAGE_TIMER_ID => {
            let _ = KillTimer(window, PENDING_LANGUAGE_TIMER_ID);
            if let Some(layout) = current_keyboard_layout() {
                let changed = LAST_LAYOUT.with(|last| {
                    let previous = last.get();
                    last.set(Some(layout));
                    previous.is_none_or(|previous| previous.0 != layout.0)
                });

                trace_overlay(&format!("language timer fired; changed: {changed}"));
                if changed {
                    show_language_overlay(language_name(layout));
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = *(lparam.0 as *const KBDLLHOOKSTRUCT);

        if event.vkCode == u32::from(VK_CAPITAL.0) {
            match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    if !CAPS_LOCK_PRESSED.swap(true, Ordering::Relaxed) {
                        let message = CAPS_MESSAGE.load(Ordering::Relaxed);
                        let target =
                            HWND(MESSAGE_TARGET.load(Ordering::Relaxed) as *mut core::ffi::c_void);
                        if message != 0 && !target.0.is_null() {
                            let _ = PostMessageW(target, message, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    CAPS_LOCK_PRESSED.store(false, Ordering::Relaxed);
                }
                _ => {}
            }

            return LRESULT(1);
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

fn main() -> Result<()> {
    configure_autostart(read_autostart_enabled());

    unsafe {
        let instance_mutex = CreateMutexW(None, false, w!("Local\\CapsSwitch"))?;
        let previous_running = GetLastError() == ERROR_ALREADY_EXISTS;
        if previous_running && FindWindowW(w!("CapsSwitchMessageTarget"), None).is_err() {
            let _ = CloseHandle(instance_mutex);
            return Ok(());
        }
        if !close_previous_instance() {
            let _ = CloseHandle(instance_mutex);
            return Ok(());
        }

        let message_target = create_message_target()?;
        MESSAGE_TARGET.store(message_target.0 as isize, Ordering::Relaxed);

        let caps_message = RegisterWindowMessageW(w!("CapsSwitch.CapsLockPressed"));
        let shell_message = RegisterWindowMessageW(w!("CapsSwitch.InputLanguageChanged"));
        let activation_message = RegisterWindowMessageW(w!("CapsSwitch.WindowActivated"));
        if caps_message == 0 || shell_message == 0 || activation_message == 0 {
            return Err(Error::from_win32());
        }
        CAPS_MESSAGE.store(caps_message, Ordering::Relaxed);
        SHELL_MESSAGE.store(shell_message, Ordering::Relaxed);
        ACTIVATION_MESSAGE.store(activation_message, Ordering::Relaxed);
        ACTIVATION.with(|activation| activation.set(Some(Instant::now())));
        LAST_LAYOUT.with(|last| last.set(current_keyboard_layout()));

        let (shell_library, shell_hook) = install_shell_hook()?;

        let keyboard_hook_handle = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            GetModuleHandleW(None)?,
            0,
        )?;

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = UnhookWindowsHookEx(keyboard_hook_handle);
        let _ = UnhookWindowsHookEx(shell_hook);
        let _ = FreeLibrary(shell_library);
        let _ = CloseHandle(instance_mutex);
    }

    Ok(())
}
