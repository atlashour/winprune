//! Boundary between the engine and the operating system. `Inspect` reads, `Apply`
//! writes. The engine never touches Windows APIs directly, which is what makes dry-run
//! and unit tests share the real code path.

pub mod fake;
pub mod recorder;
#[cfg(windows)]
pub mod windows;

use crate::catalog::{Hive, Startup};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppxPackage {
    pub name: String,
    pub full_name: String,
    pub family: String,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum RegValue {
    Dword(u32),
    String(String),
}

#[derive(Debug, Error)]
pub enum SysError {
    #[error("access denied")]
    AccessDenied,
    #[error("{0}")]
    Other(String),
}

pub trait Inspect {
    fn installed_packages(&self) -> Result<Vec<AppxPackage>, SysError>;
    fn provisioned_packages(&self) -> Result<Provisioned, SysError>;
    fn service(&self, name: &str) -> Result<Option<ServiceInfo>, SysError>;
    fn registry_value(
        &self,
        hive: Hive,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError>;
    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError>;
    fn running_processes(&self) -> Result<Vec<String>, SysError>;
    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Skipped(String),
    Failed(String),
}

pub trait Apply {
    fn remove_package(&mut self, full_name: &str) -> Outcome;
    fn deprovision_package(&mut self, family: &str) -> Outcome;
    fn stop_service(&mut self, name: &str) -> Outcome;
    fn set_service_startup(&mut self, name: &str, startup: Startup) -> Outcome;
    fn registry_set(&mut self, hive: Hive, path: &str, name: &str, value: &RegValue) -> Outcome;
    fn registry_delete(&mut self, hive: Hive, path: &str, name: &str) -> Outcome;
    fn task_disable(&mut self, path: &str) -> Outcome;
    fn task_delete(&mut self, path: &str) -> Outcome;
    fn kill_process(&mut self, name: &str) -> Outcome;
    fn run(&mut self, exe: &Path, args: &[String]) -> Outcome;
    fn delete_path(&mut self, path: &Path) -> Outcome;
}
