use super::plan::{OpKind, OpState, Plan};
use super::report::{OpResult, Report, Tally};
use crate::system::{Apply, Outcome};

/// Walks the selected items and hands every `WillApply` op to `sys`. Nothing here
/// aborts: a failed op is recorded and the next one runs. `on_event` fires per op so a
/// front end can show progress.
pub fn apply_plan(
    plan: &Plan,
    sys: &mut dyn Apply,
    dry_run: bool,
    on_event: &mut dyn FnMut(&OpResult),
) -> Report {
    let mut report = Report::new(plan, dry_run);
    for item in plan.selected() {
        for op in &item.ops {
            let outcome = match op.state {
                OpState::WillApply => run_op(&op.kind, sys),
                OpState::Absent | OpState::AlreadyDone => {
                    report.tally.absent += 1;
                    continue;
                }
                OpState::NonRemovable => Outcome::Skipped("not removable".into()),
                OpState::NeedsElevation => Outcome::Skipped("needs elevation".into()),
            };
            let result = OpResult {
                item: item.id.clone(),
                op: op.kind.to_string(),
                outcome,
            };
            report.tally.count(&result.outcome);
            on_event(&result);
            report.results.push(result);
        }
    }
    report
}

fn run_op(kind: &OpKind, sys: &mut dyn Apply) -> Outcome {
    match kind {
        OpKind::RemovePackage { full_name, .. } => sys.remove_package(full_name),
        OpKind::Deprovision { family } => sys.deprovision_package(family),
        OpKind::StopService { name } => sys.stop_service(name),
        OpKind::ServiceStartup { name, startup } => sys.set_service_startup(name, *startup),
        OpKind::RegistrySet {
            hive,
            path,
            name,
            value,
        } => sys.registry_set(*hive, path, name, value),
        OpKind::RegistryDelete { hive, path, name } => sys.registry_delete(*hive, path, name),
        OpKind::TaskDisable { path } => sys.task_disable(path),
        OpKind::TaskDelete { path } => sys.task_delete(path),
        OpKind::Kill { name } => sys.kill_process(name),
        OpKind::Run { exe, args } => sys.run(exe, args),
        OpKind::Delete { path } => sys.delete_path(path),
        OpKind::PackagePattern { .. } | OpKind::TaskPattern { .. } => {
            Outcome::Skipped("nothing to apply".into())
        }
    }
}

impl Tally {
    fn count(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Done => self.done += 1,
            Outcome::Skipped(_) => self.skipped += 1,
            Outcome::Failed(_) => self.failed += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, Level};
    use crate::engine::{Selection, build_plan};
    use crate::system::fake::Fake;
    use crate::system::recorder::Recorder;

    const CATALOG: &str = r#"
[[item]]
id = "appx.demo"
name = "Demo"
category = "appx"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "appx"
patterns = ["*Demo*", "*Missing*"]

[[item]]
id = "services.demo"
name = "Demo service"
category = "services"
level = "max"
risk = "low"
summary = "x"
warning = "w"
[[item.step]]
kind = "service"
names = ["DemoSvc"]
startup = "disabled"
"#;

    fn plan(level: Level) -> Plan {
        let catalog = Catalog::parse("t", CATALOG).unwrap();
        let sys = Fake::default()
            .with_package("Microsoft.DemoApp", false)
            .with_package("Microsoft.DemoCore", true)
            .with_service("DemoSvc", Startup::Automatic, true);
        build_plan(&catalog, &Selection::level(level), 22631, true, &sys)
    }

    use crate::catalog::Startup;

    #[test]
    fn apply_only_touches_selected_will_apply_ops() {
        let mut rec = Recorder::default();
        let report = apply_plan(&plan(Level::Medium), &mut rec, true, &mut |_| {});
        assert_eq!(
            rec.calls,
            vec!["remove package Microsoft.DemoApp_1.0.0.0_x64__8wekyb3d8bbwe"]
        );
        assert_eq!(report.tally.done, 1);
        assert_eq!(report.tally.skipped, 1);
        assert_eq!(report.tally.absent, 1);
        assert_eq!(report.tally.failed, 0);
    }

    #[test]
    fn service_is_stopped_before_startup_change() {
        let mut rec = Recorder::default();
        apply_plan(&plan(Level::Max), &mut rec, true, &mut |_| {});
        assert_eq!(
            rec.calls[1..],
            ["stop service DemoSvc", "service DemoSvc -> disabled"]
        );
    }

    #[test]
    fn events_fire_per_op_and_report_is_marked_dry_run() {
        let mut rec = Recorder::default();
        let mut seen = Vec::new();
        let report = apply_plan(&plan(Level::Medium), &mut rec, true, &mut |r| {
            seen.push(r.op.clone())
        });
        assert_eq!(seen.len(), 2);
        assert!(report.dry_run);
        assert!(report.to_json().contains("\"schema\": 1"));
    }
}
