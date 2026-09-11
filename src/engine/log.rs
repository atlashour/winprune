use super::report::{Report, file_stamp, timestamp};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Append-only log plus one JSON report per run. Elevated runs write under
/// ProgramData; anything else stays in the user's local AppData.
pub struct Log {
    dir: PathBuf,
    file: Option<File>,
}

impl Log {
    pub fn open(elevated: bool) -> Log {
        let base = if elevated {
            std::env::var_os("ProgramData")
        } else {
            std::env::var_os("LOCALAPPDATA")
        }
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
        let dir = base.join("winprune");
        let _ = fs::create_dir_all(&dir);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("winprune.log"))
            .ok();
        Log { dir, file }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join("winprune.log")
    }

    pub fn line(&mut self, text: &str) {
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "[{}] {text}", timestamp());
        }
    }

    pub fn write_report(&self, report: &Report) -> Option<PathBuf> {
        let path = self.dir.join(format!("report-{}.json", file_stamp()));
        fs::write(&path, report.to_json()).ok().map(|_| path)
    }

    pub fn write_plan(&self, plan: &crate::engine::Plan) -> Option<PathBuf> {
        let path = self.dir.join(format!("plan-{}.json", file_stamp()));
        let text = serde_json::to_string_pretty(plan).ok()?;
        fs::write(&path, text).ok().map(|_| path)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
