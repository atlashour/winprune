use crate::system::{Outcome, SysError, TaskInfo};
use windows::Win32::Foundation::VARIANT_FALSE;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{
    IRegisteredTask, ITaskFolder, ITaskService, TASK_ENUM_HIDDEN, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::BSTR;

fn service() -> windows::core::Result<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let none = VARIANT::default();
        service.Connect(&none, &none, &none, &none)?;
        Ok(service)
    }
}

fn walk(folder: &ITaskFolder, out: &mut Vec<TaskInfo>) -> windows::core::Result<()> {
    unsafe {
        let tasks = folder.GetTasks(TASK_ENUM_HIDDEN.0)?;
        for i in 1..=tasks.Count()? {
            let task = tasks.get_Item(&VARIANT::from(i))?;
            out.push(TaskInfo {
                path: task.Path()?.to_string(),
                enabled: task.Enabled()?.as_bool(),
            });
        }
        let folders = folder.GetFolders(0)?;
        for i in 1..=folders.Count()? {
            walk(&folders.get_Item(&VARIANT::from(i))?, out)?;
        }
    }
    Ok(())
}

pub fn list() -> Result<Vec<TaskInfo>, SysError> {
    let mut out = Vec::new();
    unsafe {
        let root = service()
            .and_then(|s| s.GetFolder(&BSTR::from("\\")))
            .map_err(|e| SysError::Other(e.message()))?;
        walk(&root, &mut out).map_err(|e| SysError::Other(e.message()))?;
    }
    Ok(out)
}

fn split(path: &str) -> (String, String) {
    match path.rfind('\\') {
        Some(0) | None => ("\\".to_string(), path.trim_start_matches('\\').to_string()),
        Some(pos) => (path[..pos].to_string(), path[pos + 1..].to_string()),
    }
}

fn find(path: &str) -> windows::core::Result<(ITaskFolder, IRegisteredTask)> {
    let (dir, name) = split(path);
    unsafe {
        let folder = service()?.GetFolder(&BSTR::from(dir))?;
        let task = folder.GetTask(&BSTR::from(name))?;
        Ok((folder, task))
    }
}

pub fn disable(path: &str) -> Outcome {
    match find(path) {
        Ok((_, task)) => unsafe {
            match task.SetEnabled(VARIANT_FALSE) {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Failed(e.message()),
            }
        },
        Err(e) => Outcome::Skipped(format!("task not found: {}", e.message().trim())),
    }
}

pub fn delete(path: &str) -> Outcome {
    let (_, name) = split(path);
    match find(path) {
        Ok((folder, _)) => unsafe {
            match folder.DeleteTask(&BSTR::from(name), 0) {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Failed(e.message()),
            }
        },
        Err(e) => Outcome::Skipped(format!("task not found: {}", e.message().trim())),
    }
}
