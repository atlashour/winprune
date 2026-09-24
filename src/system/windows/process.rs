use crate::system::{Outcome, ProcessInfo, SysError, lives_under};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W,
    Process32FirstW, Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};
use windows::core::PWSTR;

/// How long a killed process gets to disappear before we move on. A delete that
/// follows needs the executable image unmapped, which lags TerminateProcess.
const EXIT_WAIT_MS: u32 = 5000;

fn snapshot() -> Result<Vec<(u32, String)>, SysError> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| SysError::Other(e.message()))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                out.push((
                    entry.th32ProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..len]),
                ));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    Ok(out)
}

/// The executable image of a process, `None` for the ones we may not open (system
/// and protected processes).
fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

pub fn running() -> Result<Vec<ProcessInfo>, SysError> {
    let mut out: Vec<ProcessInfo> = snapshot()?
        .into_iter()
        .map(|(pid, name)| ProcessInfo {
            name,
            path: image_path(pid),
        })
        .collect();
    out.sort_by_cached_key(|p| (p.name.to_ascii_lowercase(), p.path.clone()));
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name) && a.path == b.path);
    Ok(out)
}

/// Full paths of the modules a process has loaded, empty when it cannot be inspected.
fn modules(pid: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
        else {
            return out;
        };
        let mut entry = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        if Module32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExePath
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExePath.len());
                out.push(PathBuf::from(String::from_utf16_lossy(
                    &entry.szExePath[..len],
                )));
                if Module32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Processes keeping something inside `tree` mapped: their own executable, as
/// "name (pid N)", or a DLL they loaded, as "name (pid N) via file.dll". A shell
/// extension inside Explorer is the usual second case.
pub fn residents(tree: &Path) -> Vec<String> {
    let Ok(procs) = snapshot() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (pid, name) in procs {
        // 0 and 4 are the idle and system pseudo-processes; 0 would also mean "us".
        if pid == 0 || pid == 4 {
            continue;
        }
        if image_path(pid).is_some_and(|p| lives_under(&p, tree)) {
            out.push(format!("{name} (pid {pid})"));
        } else if let Some(dll) = modules(pid).into_iter().find(|m| lives_under(m, tree)) {
            let dll = dll
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(format!("{name} (pid {pid}) via {dll}"));
        }
    }
    out
}

pub fn kill(name: &str) -> Outcome {
    let wanted = name.trim_end_matches(".exe");
    let procs = match snapshot() {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let mut killed = Vec::new();
    let mut last_error = None;
    for (pid, exe) in procs {
        if !exe.trim_end_matches(".exe").eq_ignore_ascii_case(wanted) {
            continue;
        }
        unsafe {
            match OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
                Ok(h) => match TerminateProcess(h, 1) {
                    Ok(()) => killed.push(h),
                    Err(e) => {
                        last_error = Some(e.message());
                        let _ = CloseHandle(h);
                    }
                },
                Err(e) => last_error = Some(e.message()),
            }
        }
    }
    let outcome = match (killed.is_empty(), last_error) {
        (true, Some(e)) => Outcome::Failed(e),
        (true, None) => Outcome::Skipped("not running".into()),
        _ => Outcome::Done,
    };
    for h in killed {
        unsafe {
            let _ = WaitForSingleObject(h, EXIT_WAIT_MS);
            let _ = CloseHandle(h);
        }
    }
    outcome
}
