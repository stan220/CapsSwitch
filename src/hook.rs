use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HMODULE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, FindWindowW, PostMessageW, RegisterWindowMessageW, SetWindowsHookExW,
    HSHELL_LANGUAGE, HSHELL_WINDOWACTIVATED, WH_SHELL,
};

const HSHELL_RUDEAPPACTIVATED: u32 = 0x8004;

unsafe extern "system" fn shell_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let message = match code as u32 {
        HSHELL_LANGUAGE => RegisterWindowMessageW(w!("CapsSwitch.InputLanguageChanged")),
        HSHELL_WINDOWACTIVATED | HSHELL_RUDEAPPACTIVATED => {
            RegisterWindowMessageW(w!("CapsSwitch.WindowActivated"))
        }
        _ => 0,
    };

    if message != 0 {
        if let Ok(window) = FindWindowW(w!("CapsSwitchMessageTarget"), None) {
            let _ = PostMessageW(window, message, wparam, lparam);
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

#[no_mangle]
pub extern "system" fn install_caps_switch_hook() -> isize {
    unsafe {
        let mut module = HMODULE::default();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(shell_hook as usize as *const u16),
            &mut module,
        )
        .is_err()
        {
            return 0;
        }

        SetWindowsHookExW(WH_SHELL, Some(shell_hook), module, 0)
            .map(|hook| hook.0 as isize)
            .unwrap_or(0)
    }
}
