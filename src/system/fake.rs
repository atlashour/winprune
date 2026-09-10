#![cfg_attr(not(test), allow(dead_code))]

use super::{
    AppxPackage, Inspect, PathInfo, Provisioned, RegValue, ServiceInfo, SysError, TaskInfo,
};
use crate::catalog::{Hive, Startup};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// In-memory `Inspect` for tests. Everything is absent unless added with a builder.
#[derive(Debug, Default)]
pub struct Fake {
    packages: Vec<AppxPackage>,
    provisioned: Option<Provisioned>,
    services: HashMap<String, ServiceInfo>,
    registry: HashMap<(Hive, String, String), RegValue>,
    tasks: Vec<TaskInfo>,
    processes: Vec<String>,
    paths: HashMap<PathBuf, PathInfo>,
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

    pub fn with_registry(mut self, hive: Hive, path: &str, name: &str, value: RegValue) -> Self {
        self.registry.insert(
            (hive, path.to_ascii_lowercase(), name.to_ascii_lowercase()),
            value,
        );
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
        self.processes.push(name.to_string());
        self
    }

    pub fn with_path(mut self, path: &str, files: u64, bytes: u64) -> Self {
        self.paths
            .insert(PathBuf::from(path), PathInfo { files, bytes });
        self
    }
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
        hive: Hive,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError> {
        Ok(self
            .registry
            .get(&(hive, path.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .cloned())
    }

    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError> {
        Ok(self.tasks.clone())
    }

    fn running_processes(&self) -> Result<Vec<String>, SysError> {
        Ok(self.processes.clone())
    }

    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError> {
        Ok(self.paths.get(path).copied())
    }
}
