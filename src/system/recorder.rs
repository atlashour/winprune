use super::{Apply, Outcome, RegRoot, RegValue};
use crate::catalog::Startup;
use std::path::Path;

/// `Apply` implementation behind `--dry-run`: every call succeeds and is written down.
#[derive(Debug, Default)]
pub struct Recorder {
    pub calls: Vec<String>,
}

impl Recorder {
    fn note(&mut self, line: String) -> Outcome {
        self.calls.push(line);
        Outcome::Done
    }
}

impl Apply for Recorder {
    fn remove_package(&mut self, full_name: &str) -> Outcome {
        self.note(format!("remove package {full_name}"))
    }

    fn deprovision_package(&mut self, family: &str) -> Outcome {
        self.note(format!("deprovision {family}"))
    }

    fn stop_service(&mut self, name: &str) -> Outcome {
        self.note(format!("stop service {name}"))
    }

    fn set_service_startup(&mut self, name: &str, startup: Startup) -> Outcome {
        self.note(format!("service {name} -> {startup}"))
    }

    fn registry_set(
        &mut self,
        root: &RegRoot,
        path: &str,
        name: &str,
        value: &RegValue,
    ) -> Outcome {
        self.note(format!("registry set {root}\\{path}\\{name} = {value:?}"))
    }

    fn registry_delete(&mut self, root: &RegRoot, path: &str, name: &str) -> Outcome {
        self.note(format!("registry delete {root}\\{path}\\{name}"))
    }

    fn task_disable(&mut self, path: &str) -> Outcome {
        self.note(format!("disable task {path}"))
    }

    fn task_delete(&mut self, path: &str) -> Outcome {
        self.note(format!("delete task {path}"))
    }

    fn kill_process(&mut self, name: &str) -> Outcome {
        self.note(format!("kill {name}"))
    }

    fn run(&mut self, exe: &Path, args: &[String], _skip_exit_codes: &[i32]) -> Outcome {
        self.note(format!("run {} {}", exe.display(), args.join(" ")))
    }

    fn delete_path(&mut self, path: &Path) -> Outcome {
        self.note(format!("delete {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_records_every_call_in_order() {
        let mut r = Recorder::default();
        assert_eq!(r.kill_process("OneDrive"), Outcome::Done);
        assert_eq!(
            r.set_service_startup("BITS", Startup::Manual),
            Outcome::Done
        );
        assert_eq!(r.calls, vec!["kill OneDrive", "service BITS -> manual"]);
    }
}
