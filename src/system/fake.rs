#![cfg_attr(not(test), allow(dead_code))]

use super::{
    AppxPackage, Inspect, PathInfo, ProcessInfo, Provisioned, RegRoot, RegValue, ServiceInfo,
    SysError, TaskInfo, UserProfile,
};
use crate::catalog::Startup;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// In-memory `Inspect` for tests. Everything is absent unless added with a builder.
#[derive(Debug, Default)]
pub struct Fake {
    packages: Vec<AppxPackage>,
    provisioned: Option<Provisioned>,
    services: HashMap<String, ServiceInfo>,
    registry: HashMap<(String, String, String), RegValue>,
    tasks: Vec<TaskInfo>,
    processes: Vec<ProcessInfo>,
    paths: HashMap<PathBuf, PathInfo>,
    profiles: Vec<UserProfile>,
    interactive: Option<String>,
}

impl Fake {
    pub fn with_package(mut self, name: &str, non_removable: bool) -> Self {
        self.packages.push(AppxPackage {
            name: name.to_string(),
            full_name: format!("{name}_1.0.0.0_x64__8wekyb3d8bbwe"),
            family: format!("{name}_8wekyb3d8bbwe"),
            non_removable,
        });
        self
    }

    pub fn provisioned(mut self, value: Provisioned) -> Self {
        self.provisioned = Some(value);
        self
    }

    pub fn with_service(mut self, name: &str, startup: Startup, running: bool) -> Self {
        self.services.insert(
            name.to_ascii_lowercase(),
            ServiceInfo {
                name: name.to_string(),
                startup,
                running,
            },
        );
        self
    }

    pub fn with_registry(
        mut self,
        root: &RegRoot,
        path: &str,
        name: &str,
        value: RegValue,
    ) -> Self {
        self.registry.insert(key(root, path, name), value);
        self
    }

    pub fn with_task(mut self, path: &str, enabled: bool) -> Self {
        self.tasks.push(TaskInfo {
            path: path.to_string(),
            enabled,
        });
        self
    }

    pub fn with_process(mut self, name: &str) -> Self {
        self.processes.push(ProcessInfo {
            name: name.to_string(),
            path: None,
        });
        self
    }

    pub fn with_process_at(mut self, name: &str, path: &str) -> Self {
        self.processes.push(ProcessInfo {
            name: name.to_string(),
            path: Some(PathBuf::from(path)),
        });
        self
    }

    pub fn with_path(mut self, path: &str, files: u64, bytes: u64) -> Self {
        self.paths
            .insert(PathBuf::from(path), PathInfo { files, bytes });
        self
    }

    pub fn with_profile(mut self, sid: &str, name: &str, path: &str, loaded: bool) -> Self {
        self.profiles.push(UserProfile {
            sid: sid.to_string(),
            name: name.to_string(),
            path: PathBuf::from(path),
            loaded,
        });
        self
    }

    pub fn interactive(mut self, sid: &str) -> Self {
        self.interactive = Some(sid.to_string());
        self
    }
}

fn key(root: &RegRoot, path: &str, name: &str) -> (String, String, String) {
    (
        root.to_string(),
        path.to_ascii_lowercase(),
        name.to_ascii_lowercase(),
    )
}

impl Inspect for Fake {
    fn installed_packages(&self) -> Result<Vec<AppxPackage>, SysError> {
        Ok(self.packages.clone())
    }

    fn provisioned_packages(&self) -> Result<Provisioned, SysError> {
        Ok(self
            .provisioned
            .clone()
            .unwrap_or(Provisioned::Known(Vec::new())))
    }

    fn service(&self, name: &str) -> Result<Option<ServiceInfo>, SysError> {
        Ok(self.services.get(&name.to_ascii_lowercase()).cloned())
    }

    fn registry_value(
        &self,
        root: &RegRoot,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError> {
        Ok(self.registry.get(&key(root, path, name)).cloned())
    }

    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError> {
        Ok(self.tasks.clone())
    }

    fn running_processes(&self) -> Result<Vec<ProcessInfo>, SysError> {
        Ok(self.processes.clone())
    }

    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError> {
        Ok(self.paths.get(path).copied())
    }

    fn user_profiles(&self) -> Result<Vec<UserProfile>, SysError> {
        Ok(self.profiles.clone())
    }

    fn interactive_user(&self) -> Option<UserProfile> {
        let sid = self.interactive.as_ref()?;
        self.profiles.iter().find(|p| &p.sid == sid).cloned()
    }

    fn default_profile_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("C:\\Users\\Default"))
    }
}
