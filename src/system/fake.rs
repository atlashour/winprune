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
    unreadable: Vec<String>,
    packages_fail: bool,
    provisioned_fail: bool,
    tasks_fail: bool,
    processes_fail: bool,
    profiles_fail: bool,
    failing_paths: Vec<PathBuf>,
}

fn unavailable(what: &str) -> SysError {
    SysError::Other(format!("{what} unavailable"))
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

    pub fn failing_packages(mut self) -> Self {
        self.packages_fail = true;
        self
    }

    pub fn failing_provisioned(mut self) -> Self {
        self.provisioned_fail = true;
        self
    }

    pub fn failing_tasks(mut self) -> Self {
        self.tasks_fail = true;
        self
    }

    pub fn failing_processes(mut self) -> Self {
        self.processes_fail = true;
        self
    }

    pub fn failing_profiles(mut self) -> Self {
        self.profiles_fail = true;
        self
    }

    /// Checking `path` fails, as it does for a folder the account cannot traverse.
    pub fn failing_path(mut self, path: &str) -> Self {
        self.failing_paths.push(PathBuf::from(path));
        self
    }

    /// Every read under `root` fails, the way the Default profile hive does without
    /// elevation.
    pub fn unreadable(mut self, root: &RegRoot) -> Self {
        self.unreadable.push(root.to_string());
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
        if self.packages_fail {
            return Err(unavailable("package manager"));
        }
        Ok(self.packages.clone())
    }

    fn provisioned_packages(&self) -> Result<Provisioned, SysError> {
        if self.provisioned_fail {
            return Err(unavailable("provisioning store"));
        }
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
        // as on Windows: the Default profile's UsrClass.dat is never mounted
        if *root == RegRoot::DefaultProfile
            && path.to_ascii_lowercase().starts_with("software\\classes\\")
        {
            return Err(SysError::Unreachable);
        }
        if self.unreadable.contains(&root.to_string()) {
            return Err(SysError::NotLoaded);
        }
        Ok(self.registry.get(&key(root, path, name)).cloned())
    }

    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError> {
        if self.tasks_fail {
            return Err(unavailable("task scheduler"));
        }
        Ok(self.tasks.clone())
    }

    fn running_processes(&self) -> Result<Vec<ProcessInfo>, SysError> {
        if self.processes_fail {
            return Err(unavailable("process list"));
        }
        Ok(self.processes.clone())
    }

    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError> {
        if self.failing_paths.iter().any(|p| p == path) {
            return Err(SysError::AccessDenied);
        }
        Ok(self.paths.get(path).copied())
    }

    fn exists(&self, path: &Path) -> Result<bool, SysError> {
        if self.failing_paths.iter().any(|p| p == path) {
            return Err(SysError::AccessDenied);
        }
        Ok(self.paths.contains_key(path))
    }

    fn user_profiles(&self) -> Result<Vec<UserProfile>, SysError> {
        if self.profiles_fail {
            return Err(unavailable("profile list"));
        }
        Ok(self.profiles.clone())
    }

    fn interactive_user(&self) -> Option<UserProfile> {
        // the real lookup goes through the same profile list
        if self.profiles_fail {
            return None;
        }
        let sid = self.interactive.as_ref()?;
        self.profiles.iter().find(|p| &p.sid == sid).cloned()
    }

    fn default_profile_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("C:\\Users\\Default"))
    }
}
