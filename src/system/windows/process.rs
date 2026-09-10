use crate::system::{Outcome, SysError};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

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

pub fn running() -> Result<Vec<String>, SysError> {
    let mut names: Vec<String> = snapshot()?.into_iter().map(|(_, n)| n).collect();
    names.sort_unstable_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(names)
}

pub fn kill(name: &str) -> Outcome {
    let wanted = name.trim_end_matches(".exe");
    let procs = match snapshot() {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let mut killed = 0;
    let mut last_error = None;
    for (pid, exe) in procs {
        if !exe.trim_end_matches(".exe").eq_ignore_ascii_case(wanted) {
            continue;
        }
        unsafe {
            match OpenProcess(PROCESS_TERMINATE, false, pid) {
                Ok(h) => {
                    match TerminateProcess(h, 1) {
                        Ok(()) => killed += 1,
                        Err(e) => last_error = Some(e.message()),
                    }
                    let _ = CloseHandle(h);
                }
                Err(e) => last_error = Some(e.message()),
            }
        }
    }
    match (killed, last_error) {
        (0, Some(e)) => Outcome::Failed(e),
        (0, None) => Outcome::Skipped("not running".into()),
        _ => Outcome::Done,
    }
}
