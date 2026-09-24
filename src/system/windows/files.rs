use crate::system::{Outcome, PathInfo, SysError};
use std::fs;
use std::path::Path;
use std::process::Command;

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
            // A running executable inside the tree fails the whole delete with a bare
            // access denied; say who it was.
            let holders = super::process::residents(path);
            if holders.is_empty() {
                Outcome::Failed(e.to_string())
            } else {
                Outcome::Failed(format!("{e}; held by {}", holders.join(", ")))
            }
        }
    }
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

    #[test]
    fn delete_names_the_process_holding_the_tree() {
        let dir = std::env::temp_dir().join("winprune-test-holder");
        let mut child = resident(&dir, "winprune-holder.exe");
        let outcome = delete(&dir);
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
        match outcome {
            Outcome::Failed(why) => {
                assert!(why.contains("held by winprune-holder.exe (pid "), "{why}")
            }
            other => panic!("{other:?}"),
        }
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
