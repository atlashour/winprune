mod appx;
mod files;
mod process;
pub mod profiles;
mod registry;
mod services;
mod tasks;

use super::{
    Apply, AppxPackage, Inspect, Outcome, PathInfo, ProcessInfo, Provisioned, RegRoot, RegValue,
    ServiceInfo, SysError, TaskInfo, UserProfile,
};
use crate::catalog::Startup;
use profiles::DefaultMount;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

pub struct WindowsSystem {
    elevated: bool,
    interactive_sid: Option<String>,
    default_mount: RefCell<Option<Result<Arc<DefaultMount>, String>>>,
}

impl WindowsSystem {
    /// `interactive_sid` comes from the unelevated launcher when there is one;
    /// otherwise the console session decides.
    pub fn new(elevated: bool, interactive_sid: Option<String>) -> WindowsSystem {
        // MTA for the Task Scheduler COM calls; WinRT is fine with it too.
        unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED) == RPC_E_CHANGED_MODE {
                eprintln!("note: thread already initialised COM as STA");
            }
        }
        let interactive_sid = interactive_sid.or_else(|| {
            if elevated {
                profiles::console_user_sid().or_else(profiles::current_sid)
            } else {
                profiles::current_sid()
            }
        });
        WindowsSystem {
            elevated,
            interactive_sid,
            default_mount: RefCell::new(None),
        }
    }

    /// Loads the Default profile hive on first use and keeps it until drop.
    fn default_mount(&self) -> Result<(), String> {
        let mut slot = self.default_mount.borrow_mut();
        if slot.is_none() {
            let result = if !self.elevated {
                Err("needs elevation".to_string())
            } else {
                match profiles::default_path() {
                    Some(folder) => DefaultMount::shared(&folder),
                    None => Err("Default profile path unknown".to_string()),
                }
            };
            *slot = Some(result);
        }
        match slot.as_ref() {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e.clone()),
            None => unreachable!(),
        }
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
        root: &RegRoot,
        path: &str,
        name: &str,
    ) -> Result<Option<RegValue>, SysError> {
        registry::read(self, root, path, name)
    }

    fn tasks(&self) -> Result<Vec<TaskInfo>, SysError> {
        tasks::list()
    }

    fn running_processes(&self) -> Result<Vec<ProcessInfo>, SysError> {
        process::running()
    }

    fn path_info(&self, path: &Path) -> Result<Option<PathInfo>, SysError> {
        files::info(path)
    }

    fn user_profiles(&self) -> Result<Vec<UserProfile>, SysError> {
        profiles::list()
    }

    fn interactive_user(&self) -> Option<UserProfile> {
        let sid = self.interactive_sid.as_ref()?;
        profiles::list().ok()?.into_iter().find(|p| &p.sid == sid)
    }

    fn default_profile_path(&self) -> Option<PathBuf> {
        profiles::default_path()
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

    fn registry_set(
        &mut self,
        root: &RegRoot,
        path: &str,
        name: &str,
        value: &RegValue,
    ) -> Outcome {
        registry::write(self, root, path, name, value)
    }

    fn registry_delete(&mut self, root: &RegRoot, path: &str, name: &str) -> Outcome {
        registry::delete(self, root, path, name)
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

    fn run(&mut self, exe: &Path, args: &[String], skip_exit_codes: &[i32]) -> Outcome {
        files::run(exe, args, skip_exit_codes)
    }

    fn delete_path(&mut self, path: &Path) -> Outcome {
        files::delete(path)
    }
}

fn is_access_denied(e: &windows::core::Error) -> bool {
    e.code() == windows::Win32::Foundation::E_ACCESSDENIED
}
