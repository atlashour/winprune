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

pub fn run(exe: &Path, args: &[String]) -> Outcome {
    match Command::new(exe).args(args).status() {
        Ok(status) if status.success() => Outcome::Done,
        Ok(status) => Outcome::Failed(format!("exit code {}", status.code().unwrap_or(-1))),
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
        Err(e) => Outcome::Failed(e.to_string()),
    }
}
