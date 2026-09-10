use super::is_access_denied;
use crate::catalog::Startup;
use crate::system::{Outcome, ServiceInfo, SysError};
use windows::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST;
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_TYPE, OpenSCManagerW,
    OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW, QueryServiceStatus, SC_HANDLE,
    SC_MANAGER_CONNECT, SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_START_TYPE, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
};
use windows::core::{HSTRING, PCWSTR};
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_SET_VALUE};

// Where the SCM keeps start types. Only used as a fallback for services that reject
// ChangeServiceConfig even when elevated (the Update Medic service). The write only
// holds until the next cumulative update restores it.
const SERVICES_KEY: &str = "SYSTEM\\CurrentControlSet\\Services";

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open(name: &str, access: u32) -> Result<Option<Handle>, windows::core::Error> {
    unsafe {
        let scm = Handle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT,
        )?);
        match OpenServiceW(scm.0, &HSTRING::from(name), access) {
            Ok(h) => Ok(Some(Handle(h))),
            Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn to_startup(start_type: SERVICE_START_TYPE) -> Startup {
    match start_type {
        SERVICE_DISABLED => Startup::Disabled,
        SERVICE_DEMAND_START => Startup::Manual,
        _ => Startup::Automatic,
    }
}

fn from_startup(startup: Startup) -> SERVICE_START_TYPE {
    match startup {
        Startup::Disabled => SERVICE_DISABLED,
        Startup::Manual => SERVICE_DEMAND_START,
        Startup::Automatic => SERVICE_AUTO_START,
    }
}

pub fn query(name: &str) -> Result<Option<ServiceInfo>, SysError> {
    let handle = open(name, SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS).map_err(map_err)?;
    let Some(handle) = handle else {
        return Ok(None);
    };
    unsafe {
        let mut needed = 0u32;
        let _ = QueryServiceConfigW(handle.0, None, 0, &mut needed);
        let mut buffer = vec![0u8; needed as usize];
        QueryServiceConfigW(
            handle.0,
            Some(buffer.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
            needed,
            &mut needed,
        )
        .map_err(map_err)?;
        let config = &*(buffer.as_ptr() as *const QUERY_SERVICE_CONFIGW);
        let mut status = SERVICE_STATUS::default();
        QueryServiceStatus(handle.0, &mut status).map_err(map_err)?;
        Ok(Some(ServiceInfo {
            name: name.to_string(),
            startup: to_startup(config.dwStartType),
            running: status.dwCurrentState != SERVICE_STOPPED,
        }))
    }
}

pub fn stop(name: &str) -> Outcome {
    match open(name, SERVICE_STOP) {
        Ok(Some(handle)) => unsafe {
            let mut status = SERVICE_STATUS::default();
            match ControlService(handle.0, SERVICE_CONTROL_STOP, &mut status) {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Skipped(format!("could not stop: {}", e.message().trim())),
            }
        },
        Ok(None) => Outcome::Skipped("service not found".into()),
        Err(e) => Outcome::Failed(e.message()),
    }
}

pub fn set_startup(name: &str, startup: Startup) -> Outcome {
    match open(name, SERVICE_CHANGE_CONFIG) {
        Ok(Some(handle)) => unsafe {
            let result = ChangeServiceConfigW(
                handle.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                from_startup(startup),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            );
            match result {
                Ok(()) => Outcome::Done,
                Err(e) if is_access_denied(&e) => set_startup_in_registry(name, startup),
                Err(e) => Outcome::Failed(e.message()),
            }
        },
        Ok(None) => Outcome::Skipped("service not found".into()),
        Err(e) if is_access_denied(&e) => set_startup_in_registry(name, startup),
        Err(e) => Outcome::Failed(e.message()),
    }
}

fn set_startup_in_registry(name: &str, startup: Startup) -> Outcome {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(format!("{SERVICES_KEY}\\{name}"), KEY_SET_VALUE);
    match key.and_then(|k| k.set_value("Start", &from_startup(startup).0)) {
        Ok(()) => Outcome::Done,
        Err(_) => Outcome::Skipped("protected by Windows".into()),
    }
}

fn map_err(e: windows::core::Error) -> SysError {
    if is_access_denied(&e) {
        SysError::AccessDenied
    } else {
        SysError::Other(e.message())
    }
}
