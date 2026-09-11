use crate::engine::expand_env;
use crate::system::{SysError, UserProfile};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LUID, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupAccountNameW,
    LookupPrivilegeValueW, PSID, SE_BACKUP_NAME, SE_PRIVILEGE_ENABLED, SE_RESTORE_NAME,
    SID_NAME_USE, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Registry::{HKEY_USERS, RegLoadKeyW, RegUnLoadKeyW};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTSDomainName, WTSFreeMemory, WTSGetActiveConsoleSessionId,
    WTSQuerySessionInformationW, WTSUserName,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

const PROFILE_LIST: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList";
/// Mount point for the Default profile hive; unique so it never collides with the
/// `HKU\Default` other scripts use.
pub const DEFAULT_MOUNT: &str = "winprune-default";
// ProfileList maps SIDs to profile folders; the Default profile's NTUSER.DAT is what
// new accounts are copied from.

pub fn list() -> Result<Vec<UserProfile>, SysError> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(PROFILE_LIST, KEY_READ)
        .map_err(|e| SysError::Other(e.to_string()))?;
    let users = RegKey::predef(winreg::enums::HKEY_USERS);
    let mut out = Vec::new();
    for sid in root.enum_keys().flatten() {
        if !(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-")) || sid.ends_with(".bak")
        {
            continue;
        }
        let Ok(key) = root.open_subkey_with_flags(&sid, KEY_READ) else {
            continue;
        };
        let Ok(image): Result<String, _> = key.get_value("ProfileImagePath") else {
            continue;
        };
        let path = PathBuf::from(expand_env(&image));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| sid.clone());
        let loaded = users.open_subkey_with_flags(&sid, KEY_READ).is_ok();
        out.push(UserProfile {
            sid,
            name,
            path,
            loaded,
        });
    }
    Ok(out)
}

pub fn default_path() -> Option<PathBuf> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(PROFILE_LIST, KEY_READ)
        .ok()?;
    let value: String = root.get_value("Default").ok()?;
    Some(PathBuf::from(expand_env(&value)))
}

/// SID of the account this process runs as.
pub fn current_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buffer = vec![0u8; len as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid)
    }
}

/// SID of the user signed in at the physical console, which is who a UAC relaunch was
/// started for even when the prompt was answered with another administrator account.
pub fn console_user_sid() -> Option<String> {
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        if session == u32::MAX {
            return None;
        }
        let user = wts_string(session, WTSUserName)?;
        let domain = wts_string(session, WTSDomainName)?;
        if user.is_empty() {
            return None;
        }
        let account = HSTRING::from(format!("{domain}\\{user}"));
        let mut sid_len = 0u32;
        let mut domain_len = 0u32;
        let mut use_ = SID_NAME_USE::default();
        let _ = LookupAccountNameW(
            PCWSTR::null(),
            &account,
            None,
            &mut sid_len,
            None,
            &mut domain_len,
            &mut use_,
        );
        let mut sid = vec![0u8; sid_len as usize];
        let mut domain_buf = vec![0u16; domain_len as usize];
        LookupAccountNameW(
            PCWSTR::null(),
            &account,
            Some(PSID(sid.as_mut_ptr() as *mut c_void)),
            &mut sid_len,
            Some(PWSTR(domain_buf.as_mut_ptr())),
            &mut domain_len,
            &mut use_,
        )
        .ok()?;
        sid_to_string(PSID(sid.as_mut_ptr() as *mut c_void))
    }
}

unsafe fn wts_string(
    session: u32,
    class: windows::Win32::System::RemoteDesktop::WTS_INFO_CLASS,
) -> Option<String> {
    unsafe {
        let mut buffer = PWSTR::null();
        let mut bytes = 0u32;
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            session,
            class,
            &mut buffer,
            &mut bytes,
        )
        .ok()?;
        let text = buffer.to_string().ok();
        WTSFreeMemory(buffer.0 as *mut c_void);
        text
    }
}

unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    unsafe {
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let out = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
        out
    }
}

/// The Default profile hive, loaded under `HKEY_USERS\winprune-default` for as long
/// as this value lives. Needs an elevated token with backup and restore privileges.
pub struct DefaultMount;

impl DefaultMount {
    pub fn mount(folder: &Path) -> Result<DefaultMount, String> {
        let hive = folder.join("NTUSER.DAT");
        if !hive.exists() {
            return Err(format!("{} not found", hive.display()));
        }
        enable_privilege(SE_BACKUP_NAME)?;
        enable_privilege(SE_RESTORE_NAME)?;
        let name = HSTRING::from(DEFAULT_MOUNT);
        let file = HSTRING::from(hive.as_os_str());
        let mut last = ERROR_SUCCESS;
        // The profile service holds the file briefly while it creates a new account.
        for _ in 0..3 {
            last = unsafe { RegLoadKeyW(HKEY_USERS, &name, &file) };
            if last == ERROR_SUCCESS {
                return Ok(DefaultMount);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err(format!(
            "could not load the Default profile hive (error {})",
            last.0
        ))
    }
}

impl Drop for DefaultMount {
    fn drop(&mut self) {
        let name = HSTRING::from(DEFAULT_MOUNT);
        for _ in 0..5 {
            if unsafe { RegUnLoadKeyW(HKEY_USERS, &name) } == ERROR_SUCCESS {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

fn enable_privilege(name: PCWSTR) -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| e.message())?;
        let mut luid = LUID::default();
        let looked_up = LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid);
        let result = looked_up.and_then(|()| {
            let state = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token, false, Some(&state), 0, None, None)
        });
        let _ = CloseHandle(token);
        result.map_err(|e| {
            format!(
                "privilege {}: {}",
                name.to_string().unwrap_or_default(),
                e.message()
            )
        })
    }
}
