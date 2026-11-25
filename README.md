# CapsSwitch

<img src="assets/icon/CapsSwitch.svg" alt="CapsSwitch" width="128">

CapsLock switches the Windows input language. A small popup shows the new language for half a second. Caps Lock itself stays off.

The popup also appears when the layout is changed with Alt+Shift, Win+Space, or the language bar. Switching between windows does not trigger it.

## Download

[Releases](https://github.com/stan220/CapsSwitch/releases) contain ZIP packages for Windows 10/11 on x86-64, 32-bit x86, and ARM64. Each package includes the executable, the matching DLL, and the default configuration.

## Build

Requires Rust and Windows 10/11 x86-64.

```powershell
cargo build --release
```

Copy these files from `target/release` into one folder:

```text
caps-switch.exe
caps_switch_hook.dll
config.toml
```

Run `caps-switch.exe`. There is no console window. On administrator accounts, approve the UAC prompt so the app can also work in elevated windows such as Task Manager. To close it, end `caps-switch.exe` in Task Manager.

## Settings

`config.toml` next to the executable:

```toml
method = "winapi"
overlay = true
```

`winapi` switches the layout directly. `alt_shift` sends Left Alt+Shift instead.

Set `overlay = false` if you only need CapsLock switching without the popup.

Restart the app after editing the file. Without the file, the defaults are `winapi` and `overlay = true`.

## Notes

The DLL is required for layout-change notifications. It uses `WH_SHELL`; the popup itself is a layered Win32 window with a rounded region and alpha fading.

## License

MIT
