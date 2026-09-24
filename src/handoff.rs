// Elevated runs get their configuration from a folder under ProgramData: both the
// interactive user and whichever admin answers the UAC prompt can read it, and it
// avoids quoting the whole selection on the command line.

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
    /// Open the TUI instead of running the CLI apply.
    #[serde(default)]
    pub tui: bool,
    /// Start the TUI at the confirmation screen: the selection was already made.
    #[serde(default)]
    pub at_confirm: bool,
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
    let count = |key: &str| t.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut s = format!(
        "{} done, {} skipped, {} blocked by Windows, {} absent or already done, {} failed",
        t.get("done")?.as_u64()?,
        t.get("skipped")?.as_u64()?,
        count("blocked"),
        t.get("absent")?.as_u64()?,
        t.get("failed")?.as_u64()?
    );
    if count("deferred") > 0 {
        s.push_str(&format!(
            ", {} left for the next restart",
            count("deferred")
        ));
    }
    Some(s)
}

fn prepare(request: &Request) -> Result<(PathBuf, Vec<String>), String> {
    let dir = new_run_dir()?;
    write_request(&dir, request)?;
    let args = vec!["--run-dir".to_string(), dir.display().to_string()];
    Ok((dir, args))
}

/// Starts the elevated child and returns its run folder without waiting: the
/// caller's window is about to close and the child has its own.
pub fn start_elevated(request: &Request) -> Result<PathBuf, String> {
    let (dir, args) = prepare(request)?;
    match os::start_elevated(&args) {
        Ok(()) => Ok(dir),
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Writes the request, runs the elevated child, prints what it reported and returns
/// its exit code. Callers restore the terminal before this.
pub fn run_elevated(request: &Request) -> i32 {
    let (dir, args) = match prepare(request) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return crate::cli::EXIT_ELEVATION;
        }
    };
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
            at_confirm: true,
        };
        let text = serde_json::to_string(&request).unwrap();
        let back: Request = serde_json::from_str(&text).unwrap();
        assert_eq!(back, request);
    }
}
