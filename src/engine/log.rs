use super::report::{Report, file_stamp, timestamp};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Append-only log plus one JSON report per run. Elevated runs write under
/// ProgramData; anything else stays in the user's local AppData. If the preferred
/// folder cannot be written the next candidate is used, so a run always leaves a trace.
pub struct Log {
    dir: PathBuf,
    file: Option<File>,
}

impl Log {
    pub fn open(elevated: bool) -> Log {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if elevated && let Some(p) = std::env::var_os("ProgramData") {
            candidates.push(PathBuf::from(p));
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(std::env::temp_dir());

        for base in candidates {
            let dir = base.join("winprune");
            if fs::create_dir_all(&dir).is_err() {
                continue;
            }
            let opened = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("winprune.log"));
            if let Ok(file) = opened {
                return Log {
                    dir,
                    file: Some(file),
                };
            }
        }
        eprintln!("winprune could not open a log file anywhere; continuing without one");
        Log {
            dir: std::env::temp_dir().join("winprune"),
            file: None,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join("winprune.log")
    }

    pub fn line(&mut self, text: &str) {
        if let Some(f) = &mut self.file
            && writeln!(f, "[{}] {text}", timestamp()).is_err()
        {
            eprintln!("log write failed: {text}");
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
