//! The real thing. Every submodule wraps one Windows subsystem and speaks only in the
//! types from `system`. Errors are mapped to `Outcome::Skipped` when Windows refuses
//! by design (protected service, non-removable package) and to `Outcome::Failed`
//! otherwise.

mod appx;
mod files;
mod process;
mod registry;
mod services;
mod tasks;

use super::{
    Apply, AppxPackage, Inspect, Outcome, PathInfo, Provisioned, RegValue, ServiceInfo, SysError,
    TaskInfo,
};
use crate::catalog::{Hive, Startup};
use std::path::Path;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

pub struct WindowsSystem;

impl WindowsSystem {
    /// COM must be initialised on the calling thread for the Task Scheduler. WinRT
    /// activation is fine with the multithreaded apartment as well.
    pub fn new() -> WindowsSystem {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        WindowsSystem
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl Inspect for WindowsSystem {
    fn installed_packages(&self) -> Result<Vec<AppxPackage>, SysError> {
        appx::installed()
    }

    fn provisioned_packages(&self) -> Result<Provisioned, SysError> {
        appx::provisioned()
    }

    fn service(&self, name: &str) -> Result<Option<ServiceInfo>, SysError> {
        services::query(name)
    }

    fn registry_value(
        &self,
        hive: Hive,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError> {
        registry::read(hive, path, name)
    }

    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError> {
        tasks::list()
    }

    fn running_processes(&self) -> Result<Vec<String>, SysError> {
        process::running()
    }

    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError> {
        files::info(path)
    }
}

impl Apply for WindowsSystem {
    fn remove_package(&mut self, full_name: &str) -> Outcome {
        appx::remove(full_name)
    }

    fn deprovision_package(&mut self, family: &str) -> Outcome {
        appx::deprovision(family)
    }

    fn stop_service(&mut self, name: &str) -> Outcome {
        services::stop(name)
    }

    fn set_service_startup(&mut self, name: &str, startup: Startup) -> Outcome {
        services::set_startup(name, startup)
    }

    fn registry_set(&mut self, hive: Hive, path: &str, name: &str, value: &RegValue) -> Outcome {
        registry::write(hive, path, name, value)
    }

    fn registry_delete(&mut self, hive: Hive, path: &str, name: &str) -> Outcome {
        registry::delete(hive, path, name)
    }

    fn task_disable(&mut self, path: &str) -> Outcome {
        tasks::disable(path)
    }

    fn task_delete(&mut self, path: &str) -> Outcome {
        tasks::delete(path)
    }

    fn kill_process(&mut self, name: &str) -> Outcome {
        process::kill(name)
    }

    fn run(&mut self, exe: &Path, args: &[String]) -> Outcome {
        files::run(exe, args)
    }

    fn delete_path(&mut self, path: &Path) -> Outcome {
        files::delete(path)
    }
}

fn is_access_denied(e: &windows::core::Error) -> bool {
    e.code() == windows::Win32::Foundation::E_ACCESSDENIED
}
