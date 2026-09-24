//! Boundary between the engine and the operating system. `Inspect` reads, `Apply`
//! writes. The engine never touches Windows APIs directly, which is what makes dry-run
//! and unit tests share the real code path.

pub mod fake;
pub mod recorder;
#[cfg(windows)]
pub mod windows;

use crate::catalog::Startup;
use serde::Serialize;
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppxPackage {
    pub name: String,
    pub full_name: String,
    pub family: String,
    /// Signed as part of Windows or a framework: removal is refused or pointless.
    pub non_removable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub startup: Startup,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInfo {
    pub path: String,
    pub enabled: bool,
}

/// A running process. `path` is the executable image, `None` when the process could
/// not be opened (protected or system processes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// Whether `path` is inside `tree`, by whole components. Windows paths compare
/// case-insensitively; `Path::starts_with` does not.
pub fn lives_under(path: &Path, tree: &Path) -> bool {
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_ascii_lowercase());
    lower(path).starts_with(lower(tree))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathInfo {
    pub files: u64,
    pub bytes: u64,
}

/// Provisioned packages need an elevated token to enumerate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provisioned {
    Known(Vec<String>),
    NeedsElevation,
}

/// A user account with a profile folder on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub sid: String,
    pub name: String,
    pub path: PathBuf,
    /// Its registry hive is mounted under HKEY_USERS right now.
    pub loaded: bool,
}

/// Where a registry step lands. Per-user roots are addressed by SID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "root", rename_all = "snake_case")]
pub enum RegRoot {
    Machine,
    Classes,
    CurrentUser,
    User { sid: String, name: String },
    DefaultProfile,
}

impl fmt::Display for RegRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegRoot::Machine => f.write_str("HKLM"),
            RegRoot::Classes => f.write_str("HKCR"),
            RegRoot::CurrentUser => f.write_str("HKCU"),
            RegRoot::User { name, .. } => write!(f, "HKU:{name}"),
            RegRoot::DefaultProfile => f.write_str("HKU:Default"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum RegValue {
    Dword(u32),
    String(String),
    /// Present, but of a type winprune never writes (binary, qword, multi-string).
    Other(String),
}

#[derive(Debug, Error)]
pub enum SysError {
    #[error("access denied")]
    AccessDenied,
    #[error("profile hive is not loaded")]
    NotLoaded,
    #[error("{0}")]
    Other(String),
}

pub trait Inspect {
    fn installed_packages(&self) -> Result<Vec<AppxPackage>, SysError>;
    fn provisioned_packages(&self) -> Result<Provisioned, SysError>;
    fn service(&self, name: &str) -> Result<Option<ServiceInfo>, SysError>;
    fn registry_value(
        &self,
        root: &RegRoot,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError>;
    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError>;
    fn running_processes(&self) -> Result<Vec<ProcessInfo>, SysError>;
    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError>;
    /// Every account with a profile folder, loaded or not.
    fn user_profiles(&self) -> Result<Vec<UserProfile>, SysError>;
    /// The account winprune is acting for: the one that started it, even after the
    /// UAC relaunch ran it as a different administrator.
    fn interactive_user(&self) -> Option<UserProfile>;
    /// Folder of the Default profile (normally C:\Users\Default).
    fn default_profile_path(&self) -> Option<PathBuf>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Skipped(String),
    /// Windows refused the change by design (a kernel driver guarding the value, a
    /// protected service). Not a bug in the tool, not something a retry fixes.
    Blocked(String),
    Failed(String),
}

pub trait Apply {
    fn remove_package(&mut self, full_name: &str) -> Outcome;
    fn deprovision_package(&mut self, family: &str) -> Outcome;
    fn stop_service(&mut self, name: &str) -> Outcome;
    fn set_service_startup(&mut self, name: &str, startup: Startup) -> Outcome;
    fn registry_set(&mut self, root: &RegRoot, path: &str, name: &str, value: &RegValue)
    -> Outcome;
    fn registry_delete(&mut self, root: &RegRoot, path: &str, name: &str) -> Outcome;
    fn task_disable(&mut self, path: &str) -> Outcome;
    fn task_delete(&mut self, path: &str) -> Outcome;
    fn kill_process(&mut self, name: &str) -> Outcome;
    /// Exit codes listed in `skip_exit_codes` count as "nothing to do", not failure.
    fn run(&mut self, exe: &Path, args: &[String], skip_exit_codes: &[i32]) -> Outcome;
    fn delete_path(&mut self, path: &Path) -> Outcome;
}
