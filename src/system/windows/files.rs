use crate::system::{Outcome, PathInfo, SysError};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows::core::{HSTRING, PCWSTR};

pub fn info(path: &Path) -> Result<Option<PathInfo>, SysError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SysError::Other(e.to_string())),
    };
    if !meta.is_dir() {
        return Ok(Some(PathInfo {
            files: 1,
            bytes: meta.len(),
        }));
    }
    let mut total = PathInfo { files: 0, bytes: 0 };
    walk(path, &mut total);
    Ok(Some(total))
}

fn walk(dir: &Path, total: &mut PathInfo) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            walk(&entry.path(), total);
        } else {
            total.files += 1;
            total.bytes += meta.len();
        }
    }
}

pub fn run(exe: &Path, args: &[String], skip_exit_codes: &[i32]) -> Outcome {
    match Command::new(exe).args(args).status() {
        Ok(status) if status.success() => Outcome::Done,
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            if skip_exit_codes.contains(&code) {
                Outcome::Skipped(format!("nothing to do (exit code {code})"))
            } else {
                Outcome::Failed(format!("exit code {code}"))
            }
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

pub fn delete(path: &Path) -> Outcome {
    let result = match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::Skipped("already gone".into());
        }
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    match result {
        Ok(()) => Outcome::Done,
        Err(e) => {
            // A running executable or a loaded DLL inside the tree fails the whole
            // delete with a bare access denied. Say who it was and let Windows finish
            // during the next restart, the way installers handle files in use.
            let holders = super::process::residents(path);
            if holders.is_empty() {
                return Outcome::Failed(e.to_string());
            }
            let held = format!("held by {}", holders.join(", "));
            match schedule_at_restart(path) {
                Ok(()) => Outcome::Deferred(held),
                Err(why) => Outcome::Failed(format!(
                    "{e}; {held}; could not schedule for the next restart: {why}"
                )),
            }
        }
    }
}

/// Everything still under `tree`, children before parents, so the entries can be
/// removed in that order. A folder that cannot be listed is an error: queued as it
/// is, it would survive the restart together with every ancestor.
fn leftovers(tree: &Path) -> Result<Vec<PathBuf>, String> {
    let describe = |e: std::io::Error| format!("{}: {e}", tree.display());
    let meta = fs::symlink_metadata(tree).map_err(describe)?;
    let mut out = Vec::new();
    if meta.is_dir() && !meta.is_symlink() {
        for entry in fs::read_dir(tree).map_err(describe)? {
            let entry = entry.map_err(describe)?;
            let is_dir = entry
                .file_type()
                .is_ok_and(|t| t.is_dir() && !t.is_symlink());
            if is_dir {
                out.extend(leftovers(&entry.path())?);
            } else {
                out.push(entry.path());
            }
        }
    }
    out.push(tree.to_path_buf());
    Ok(out)
}

/// Paths past MAX_PATH need the verbatim prefix for MoveFileExW.
fn verbatim(p: &Path) -> PathBuf {
    let text = p.to_string_lossy();
    if text.starts_with(r"\\?\") || !p.is_absolute() {
        p.to_path_buf()
    } else {
        PathBuf::from(format!(r"\\?\{text}"))
    }
}

/// Queues the whole tree; stops at the first refusal and says how much was already
/// queued, since Windows will still delete that part at the restart.
fn schedule_at_restart(tree: &Path) -> Result<(), String> {
    let entries = leftovers(tree)?;
    for (queued, p) in entries.iter().enumerate() {
        unsafe {
            MoveFileExW(
                &HSTRING::from(verbatim(p).as_os_str()),
                PCWSTR::null(),
                MOVEFILE_DELAY_UNTIL_REBOOT,
            )
            .map_err(|e| {
                format!(
                    "{}: {} ({queued} of {} entries were queued before it)",
                    p.display(),
                    e.message().trim(),
                    entries.len()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::process;
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    /// A copy of ping.exe under `dir`, running for half a minute under a name of its
    /// own so that killing it cannot touch anything else on the machine.
    fn resident(dir: &Path, name: &str) -> Child {
        let _ = fs::remove_dir_all(dir);
        fs::create_dir_all(dir).unwrap();
        let system32 = Path::new(&std::env::var("SystemRoot").unwrap()).join("System32");
        let exe = dir.join(name);
        fs::copy(system32.join("ping.exe"), &exe).unwrap();
        let child = Command::new(&exe)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        child
    }

    /// Elevated (CI runners are) the delete gets scheduled for the next restart,
    /// unelevated it fails; both say who held the tree.
    fn held_by(outcome: Outcome) -> String {
        match outcome {
            Outcome::Failed(why) | Outcome::Deferred(why) => why,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn delete_names_the_process_holding_the_tree() {
        let dir = std::env::temp_dir().join("winprune-test-holder");
        let mut child = resident(&dir, "winprune-holder.exe");
        let outcome = delete(&dir);
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
        let why = held_by(outcome);
        assert!(why.contains("held by winprune-holder.exe (pid "), "{why}");
    }

    #[test]
    fn delete_names_the_process_that_loaded_a_dll_from_the_tree() {
        use windows::Win32::Foundation::FreeLibrary;
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        use windows::core::HSTRING;
        let dir = std::env::temp_dir().join("winprune-test-module");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let system32 = Path::new(&std::env::var("SystemRoot").unwrap()).join("System32");
        let dll = dir.join("winprune-module.dll");
        fs::copy(system32.join("version.dll"), &dll).unwrap();
        let module = unsafe { LoadLibraryW(&HSTRING::from(dll.as_os_str())) }.unwrap();
        let outcome = delete(&dir);
        unsafe {
            let _ = FreeLibrary(module);
        }
        let _ = fs::remove_dir_all(&dir);
        let own = std::env::current_exe().unwrap();
        let own = own.file_name().unwrap().to_string_lossy().to_string();
        let expected = format!(
            "held by {own} (pid {}) via winprune-module.dll",
            std::process::id()
        );
        let why = held_by(outcome);
        assert!(why.contains(&expected), "{why}");
    }

    #[test]
    fn leftovers_are_listed_deepest_first() {
        let dir = std::env::temp_dir().join("winprune-test-leftovers");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a\\b")).unwrap();
        fs::write(dir.join("a\\b\\deep.txt"), "x").unwrap();
        fs::write(dir.join("top.txt"), "x").unwrap();
        let listed = leftovers(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        let position = |name: &str| {
            listed
                .iter()
                .position(|p| p.file_name().unwrap() == name)
                .unwrap_or_else(|| panic!("{name} missing from {listed:?}"))
        };
        assert!(position("deep.txt") < position("b"));
        assert!(position("b") < position("a"));
        assert!(position("a") < position("winprune-test-leftovers"));
        assert!(position("top.txt") < position("winprune-test-leftovers"));
    }

    /// Queued as is, a folder we cannot list would survive the restart together with
    /// every ancestor while the report promised the opposite.
    #[test]
    fn leftovers_refuse_a_subfolder_that_cannot_be_listed() {
        let dir = std::env::temp_dir().join("winprune-test-denied");
        let locked = dir.join("locked");
        let user = std::env::var("USERNAME").unwrap();
        let icacls = |args: &[&str]| {
            Command::new("icacls")
                .arg(&locked)
                .args(args)
                .stdout(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let _ = icacls(&["/remove:d", &user]);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("secret.txt"), "x").unwrap();
        assert!(icacls(&["/deny", &format!("{user}:(RX)")]));
        let result = leftovers(&dir);
        assert!(icacls(&["/remove:d", &user]));
        let _ = fs::remove_dir_all(&dir);
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn kill_waits_so_the_tree_can_be_deleted_right_after() {
        let dir = std::env::temp_dir().join("winprune-test-killed");
        let mut child = resident(&dir, "winprune-killed.exe");
        assert_eq!(process::kill("winprune-killed"), Outcome::Done);
        let outcome = delete(&dir);
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(outcome, Outcome::Done);
    }
}
