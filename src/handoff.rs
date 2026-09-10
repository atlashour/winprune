//! Handing a run over to the elevated copy of winprune. The unelevated launcher writes
//! everything the child needs into a folder under ProgramData (readable by both the
//! interactive user and whichever administrator answers the UAC prompt), starts the
//! child with `--run-dir`, waits, and reads the report the child leaves behind.
//! Nothing travels on the command line except that one path.

use crate::catalog::Level;
use crate::engine::{Report, file_stamp};
use crate::os;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub level: Level,
    #[serde(default)]
    pub only: Vec<String>,
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub catalog: Option<PathBuf>,
    #[serde(default)]
    pub dry_run: bool,
    /// Account the per-user steps are for: the user who started winprune.
    #[serde(default)]
    pub interactive_sid: Option<String>,
    /// Open the TUI at the confirmation screen instead of running the CLI apply.
    #[serde(default)]
    pub tui: bool,
}

pub const REQUEST_FILE: &str = "request.json";
pub const REPORT_FILE: &str = "report.json";

pub fn new_run_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .ok_or("ProgramData is not set")?;
    let dir =
        base.join("winprune")
            .join("runs")
            .join(format!("{}-{}", file_stamp(), std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn write_request(dir: &Path, request: &Request) -> Result<(), String> {
    let text = serde_json::to_string_pretty(request).map_err(|e| e.to_string())?;
    fs::write(dir.join(REQUEST_FILE), text).map_err(|e| e.to_string())
}

pub fn read_request(dir: &Path) -> Result<Request, String> {
    let path = dir.join(REQUEST_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write_report(dir: &Path, report: &Report) {
    let _ = fs::write(dir.join(REPORT_FILE), report.to_json());
}

pub fn read_report_summary(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(REPORT_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let t = value.get("tally")?;
    Some(format!(
        "{} done, {} skipped, {} absent or already done, {} failed",
        t.get("done")?.as_u64()?,
        t.get("skipped")?.as_u64()?,
        t.get("absent")?.as_u64()?,
        t.get("failed")?.as_u64()?
    ))
}

/// Writes the request, runs the elevated child and returns its exit code after
/// printing whatever it reported. `restore_terminal` runs before the UAC prompt so a
/// TUI caller hands the console back first.
pub fn run_elevated(request: &Request) -> i32 {
    let dir = match new_run_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return crate::cli::EXIT_ELEVATION;
        }
    };
    if let Err(e) = write_request(&dir, request) {
        eprintln!("{e}");
        return crate::cli::EXIT_ELEVATION;
    }
    let args = vec!["--run-dir".to_string(), dir.display().to_string()];
    match os::relaunch_elevated(&args) {
        Ok(code) => {
            match read_report_summary(&dir) {
                Some(summary) => println!("{summary}"),
                None if code == crate::cli::EXIT_ABORTED => {
                    println!("cancelled in the elevated window")
                }
                None => println!("the elevated run left no report (exit code {code})"),
            }
            println!("run folder: {}", dir.display());
            code
        }
        Err(e) => {
            eprintln!("{e}");
            let _ = fs::remove_dir_all(&dir);
            crate::cli::EXIT_ELEVATION
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let request = Request {
            level: Level::High,
            only: vec![],
            skip: vec!["appx.demo".into()],
            add: vec!["shell.widgets".into()],
            catalog: Some(PathBuf::from("C:\\x\\overlay.toml")),
            dry_run: true,
            interactive_sid: Some("S-1-5-21-1".into()),
            tui: true,
        };
        let text = serde_json::to_string(&request).unwrap();
        let back: Request = serde_json::from_str(&text).unwrap();
        assert_eq!(back, request);
    }
}
