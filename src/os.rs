//! Facts about the running machine and the process: Windows build, elevation, and the
//! UAC relaunch. Kept apart from `system` because the engine needs the build number
//! before any inspection happens.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsInfo {
    pub build: u32,
    pub elevated: bool,
}

#[cfg(windows)]
pub fn detect() -> OsInfo {
    OsInfo {
        build: win::build().unwrap_or(0),
        elevated: win::is_elevated(),
    }
}

#[cfg(not(windows))]
pub fn detect() -> OsInfo {
    OsInfo {
        build: 0,
        elevated: false,
    }
}

/// Runs this executable again through the UAC prompt with `args`, waits for it and
/// returns its exit code. `Err` means the prompt was refused or the launch failed.
#[cfg(windows)]
pub fn relaunch_elevated(args: &[String]) -> Result<i32, String> {
    win::relaunch_elevated(args)
}

#[cfg(not(windows))]
pub fn relaunch_elevated(_args: &[String]) -> Result<i32, String> {
    Err("elevation is only available on Windows".into())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
    };
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    pub fn build() -> Option<u32> {
        let key = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
            .ok()?;
        let text: String = key.get_value("CurrentBuildNumber").ok()?;
        text.trim().parse().ok()
    }

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut info = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut info as *mut _ as *mut c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && info.TokenIsElevated != 0
        }
    }

    pub fn relaunch_elevated(args: &[String]) -> Result<i32, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let file = HSTRING::from(exe.as_os_str());
        let params = HSTRING::from(quote_args(args));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe {
            ShellExecuteExW(&mut info)
                .map_err(|e| format!("elevation refused: {}", e.message()))?;
            if info.hProcess.is_invalid() {
                return Err("elevated process did not start".into());
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            Ok(code as i32)
        }
    }

    fn quote_args(args: &[String]) -> String {
        args.iter()
            .map(|a| {
                if a.contains(' ') || a.contains('"') {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}
