use super::expand_env;
use crate::catalog::{
    Catalog, Category, Hive, Item, Level, RegType, Risk, Startup, Step, TaskAction,
};
use crate::system::{AppxPackage, Inspect, Provisioned, RegValue, TaskInfo};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Selection {
    pub level: Level,
    pub only: Option<HashSet<String>>,
    pub skip: HashSet<String>,
    pub extra: HashSet<String>,
}

impl Selection {
    pub fn level(level: Level) -> Self {
        Selection {
            level,
            only: None,
            skip: HashSet::new(),
            extra: HashSet::new(),
        }
    }

    fn wants(&self, item: &Item) -> bool {
        if self.skip.contains(&item.id) {
            return false;
        }
        if self.extra.contains(&item.id) {
            return true;
        }
        match &self.only {
            Some(only) => only.contains(&item.id),
            None => self.level >= item.level,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    WillApply,
    Absent,
    AlreadyDone,
    NonRemovable,
    NeedsElevation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OpKind {
    RemovePackage {
        full_name: String,
        name: String,
    },
    Deprovision {
        family: String,
    },
    PackagePattern {
        pattern: String,
    },
    StopService {
        name: String,
    },
    ServiceStartup {
        name: String,
        startup: Startup,
    },
    RegistrySet {
        hive: Hive,
        path: String,
        name: String,
        value: RegValue,
    },
    RegistryDelete {
        hive: Hive,
        path: String,
        name: String,
    },
    TaskDisable {
        path: String,
    },
    TaskDelete {
        path: String,
    },
    TaskPattern {
        pattern: String,
    },
    Kill {
        name: String,
    },
    Run {
        exe: PathBuf,
        args: Vec<String>,
    },
    Delete {
        path: PathBuf,
    },
}

impl fmt::Display for OpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpKind::RemovePackage { name, .. } => write!(f, "remove package {name}"),
            OpKind::Deprovision { family } => write!(f, "deprovision {family}"),
            OpKind::PackagePattern { pattern } => write!(f, "package {pattern}"),
            OpKind::StopService { name } => write!(f, "stop service {name}"),
            OpKind::ServiceStartup { name, startup } => write!(f, "service {name} -> {startup}"),
            OpKind::RegistrySet {
                hive,
                path,
                name,
                value,
            } => {
                write!(f, "{hive}\\{path}\\{name} = {}", show_value(value))
            }
            OpKind::RegistryDelete { hive, path, name } => {
                write!(f, "delete {hive}\\{path}\\{name}")
            }
            OpKind::TaskDisable { path } => write!(f, "disable task {path}"),
            OpKind::TaskDelete { path } => write!(f, "delete task {path}"),
            OpKind::TaskPattern { pattern } => write!(f, "task {pattern}"),
            OpKind::Kill { name } => write!(f, "kill {name}"),
            OpKind::Run { exe, args } => write!(f, "run {} {}", exe.display(), args.join(" ")),
            OpKind::Delete { path } => write!(f, "delete {}", path.display()),
        }
    }
}

fn show_value(value: &RegValue) -> String {
    match value {
        RegValue::Dword(n) => n.to_string(),
        RegValue::String(s) => format!("\"{s}\""),
        RegValue::Other(kind) => format!("a {kind} value"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Op {
    #[serde(flatten)]
    pub kind: OpKind,
    pub state: OpState,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedItem {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub level: Level,
    pub risk: Risk,
    pub summary: String,
    pub warning: Option<String>,
    pub requires: Vec<String>,
    pub selected: bool,
    pub ops: Vec<Op>,
}

impl PlannedItem {
    pub fn will_apply(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| op.state == OpState::WillApply)
            .count()
    }

    pub fn needs_elevation(&self) -> bool {
        self.ops
            .iter()
            .any(|op| op.state == OpState::NeedsElevation)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub build: u32,
    pub elevated: bool,
    pub level: Level,
    pub items: Vec<PlannedItem>,
}

impl Plan {
    pub fn selected(&self) -> impl Iterator<Item = &PlannedItem> {
        self.items.iter().filter(|item| item.selected)
    }

    pub fn will_apply_count(&self) -> usize {
        self.selected().map(PlannedItem::will_apply).sum()
    }

    /// Re-evaluates `selected` after the user toggles items in the TUI. `requires`
    /// dependencies are dragged in; an item whose dependency is skipped stays off.
    pub fn reselect(&mut self, selection: &Selection) {
        let items: Vec<(String, Vec<String>, Level)> = self
            .items
            .iter()
            .map(|i| (i.id.clone(), i.requires.clone(), i.level))
            .collect();
        let wanted = resolve_selection(selection, &items);
        for item in &mut self.items {
            item.selected = wanted.contains(&item.id);
        }
    }
}

fn resolve_selection(
    selection: &Selection,
    items: &[(String, Vec<String>, Level)],
) -> HashSet<String> {
    let mut wanted: HashSet<String> = items
        .iter()
        .filter(|(id, _, level)| {
            let probe = Item {
                id: id.clone(),
                name: String::new(),
                category: Category::Appx,
                level: *level,
                risk: Risk::Low,
                summary: String::new(),
                warning: None,
                requires: Vec::new(),
                windows: Default::default(),
                enabled: true,
                step: Vec::new(),
            };
            selection.wants(&probe)
        })
        .map(|(id, _, _)| id.clone())
        .collect();

    loop {
        let mut added = false;
        for (id, requires, _) in items {
            if wanted.contains(id) {
                for dep in requires {
                    if !selection.skip.contains(dep) && wanted.insert(dep.clone()) {
                        added = true;
                    }
                }
            }
        }
        // An item whose dependency was explicitly skipped cannot run.
        let before = wanted.len();
        let keep: HashSet<String> = wanted
            .iter()
            .filter(|id| {
                items
                    .iter()
                    .find(|(i, _, _)| i == *id)
                    .is_none_or(|(_, requires, _)| requires.iter().all(|d| wanted.contains(d)))
            })
            .cloned()
            .collect();
        wanted = keep;
        if !added && wanted.len() == before {
            return wanted;
        }
    }
}

pub fn build_plan(
    catalog: &Catalog,
    selection: &Selection,
    build: u32,
    elevated: bool,
    sys: &dyn Inspect,
) -> Plan {
    let applicable: Vec<&Item> = catalog.applicable(build).collect();
    let items_meta: Vec<(String, Vec<String>, Level)> = applicable
        .iter()
        .map(|i| (i.id.clone(), i.requires.clone(), i.level))
        .collect();
    let wanted = resolve_selection(selection, &items_meta);

    let snapshot = Snapshot::take(sys);
    let items = applicable
        .iter()
        .map(|item| plan_item(item, wanted.contains(&item.id), &snapshot, sys))
        .collect();

    Plan {
        build,
        elevated,
        level: selection.level,
        items,
    }
}

/// Lists that are expensive to fetch are read once per plan.
struct Snapshot {
    packages: Vec<AppxPackage>,
    provisioned: Provisioned,
    tasks: Vec<TaskInfo>,
    processes: Vec<String>,
}

impl Snapshot {
    fn take(sys: &dyn Inspect) -> Self {
        Snapshot {
            packages: sys.installed_packages().unwrap_or_default(),
            provisioned: sys
                .provisioned_packages()
                .unwrap_or(Provisioned::NeedsElevation),
            tasks: sys.tasks().unwrap_or_default(),
            processes: sys.running_processes().unwrap_or_default(),
        }
    }
}

fn plan_item(item: &Item, selected: bool, snap: &Snapshot, sys: &dyn Inspect) -> PlannedItem {
    let mut ops = Vec::new();
    let mut live_notes = Vec::new();
    for step in &item.step {
        match step {
            Step::Appx { patterns } => plan_appx(patterns, snap, &mut ops),
            Step::Service { names, startup } => plan_services(names, *startup, sys, &mut ops),
            Step::Registry {
                hive,
                path,
                name,
                kind,
                value,
                delete,
            } => plan_registry(
                *hive,
                path,
                name,
                *kind,
                value.as_ref(),
                *delete,
                sys,
                &mut ops,
            ),
            Step::Task { patterns, action } => plan_tasks(patterns, *action, snap, &mut ops),
            Step::Kill { processes } => plan_kill(processes, snap, &mut ops),
            Step::Run { candidates, args } => plan_run(candidates, args, sys, &mut ops),
            Step::Delete { paths } => plan_delete(paths, sys, &mut ops, &mut live_notes),
        }
    }

    let warning = match (&item.warning, live_notes.is_empty()) {
        (Some(w), false) => Some(format!("{w} {}", live_notes.join(" "))),
        (Some(w), true) => Some(w.clone()),
        (None, false) => Some(live_notes.join(" ")),
        (None, true) => None,
    };

    PlannedItem {
        id: item.id.clone(),
        name: item.name.clone(),
        category: item.category,
        level: item.level,
        risk: item.risk,
        summary: item.summary.clone(),
        warning,
        requires: item.requires.clone(),
        selected,
        ops,
    }
}

/// Case-insensitive glob. Backslashes are the escape character for the matcher, so task
/// paths are compared with forward slashes on both sides.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase().replace('\\', "/");
    let text = text.to_ascii_lowercase().replace('\\', "/");
    glob_match::glob_match(&pattern, &text)
}

fn plan_appx(patterns: &[String], snap: &Snapshot, ops: &mut Vec<Op>) {
    for pattern in patterns {
        let mut matched = false;
        for pkg in snap.packages.iter().filter(|p| glob(pattern, &p.name)) {
            matched = true;
            ops.push(Op {
                kind: OpKind::RemovePackage {
                    full_name: pkg.full_name.clone(),
                    name: pkg.name.clone(),
                },
                state: if pkg.non_removable {
                    OpState::NonRemovable
                } else {
                    OpState::WillApply
                },
                detail: String::new(),
            });
        }
        if let Provisioned::Known(families) = &snap.provisioned {
            for family in families.iter().filter(|f| glob(pattern, f)) {
                matched = true;
                ops.push(Op {
                    kind: OpKind::Deprovision {
                        family: family.clone(),
                    },
                    state: OpState::WillApply,
                    detail: String::new(),
                });
            }
        }
        if !matched {
            ops.push(Op {
                kind: OpKind::PackagePattern {
                    pattern: pattern.clone(),
                },
                state: OpState::Absent,
                detail: String::new(),
            });
        }
    }
    // One line per step, not per pattern: the user only needs to know that the
    // provisioned list was out of reach.
    if matches!(snap.provisioned, Provisioned::NeedsElevation) {
        ops.push(Op {
            kind: OpKind::PackagePattern {
                pattern: "provisioned packages".into(),
            },
            state: OpState::NeedsElevation,
            detail: "not checked without elevation".into(),
        });
    }
}

fn plan_services(names: &[String], startup: Startup, sys: &dyn Inspect, ops: &mut Vec<Op>) {
    for name in names {
        match sys.service(name) {
            Ok(Some(info)) => {
                if startup == Startup::Disabled && info.running {
                    ops.push(Op {
                        kind: OpKind::StopService { name: name.clone() },
                        state: OpState::WillApply,
                        detail: String::new(),
                    });
                }
                ops.push(Op {
                    kind: OpKind::ServiceStartup {
                        name: name.clone(),
                        startup,
                    },
                    state: if info.startup == startup {
                        OpState::AlreadyDone
                    } else {
                        OpState::WillApply
                    },
                    detail: format!("currently {}", info.startup),
                });
            }
            Ok(None) => ops.push(Op {
                kind: OpKind::ServiceStartup {
                    name: name.clone(),
                    startup,
                },
                state: OpState::Absent,
                detail: String::new(),
            }),
            Err(e) => ops.push(Op {
                kind: OpKind::ServiceStartup {
                    name: name.clone(),
                    startup,
                },
                state: OpState::WillApply,
                detail: format!("could not inspect: {e}"),
            }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_registry(
    hive: Hive,
    path: &str,
    name: &str,
    kind: Option<RegType>,
    value: Option<&toml::Value>,
    delete: bool,
    sys: &dyn Inspect,
    ops: &mut Vec<Op>,
) {
    // A read that fails (access denied, odd key) must not look like "absent": the
    // apply step gets to try and report for itself.
    let (current, unreadable) = match sys.registry_value(hive, path, name) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("could not read: {e}"))),
    };
    if delete {
        ops.push(Op {
            kind: OpKind::RegistryDelete {
                hive,
                path: path.to_string(),
                name: name.to_string(),
            },
            state: if current.is_some() || unreadable.is_some() {
                OpState::WillApply
            } else {
                OpState::Absent
            },
            detail: unreadable.clone().unwrap_or_default(),
        });
        return;
    }
    let wanted = match (kind, value) {
        (Some(RegType::Dword), Some(v)) => {
            RegValue::Dword(v.as_integer().unwrap_or_default() as u32)
        }
        (Some(RegType::String), Some(v)) => {
            RegValue::String(v.as_str().unwrap_or_default().to_string())
        }
        _ => return,
    };
    let state = if current.as_ref() == Some(&wanted) {
        OpState::AlreadyDone
    } else {
        OpState::WillApply
    };
    ops.push(Op {
        kind: OpKind::RegistrySet {
            hive,
            path: path.to_string(),
            name: name.to_string(),
            value: wanted,
        },
        state,
        detail: match (current, unreadable) {
            (Some(v), _) => format!("currently {}", show_value(&v)),
            (None, Some(why)) => why,
            (None, None) => String::new(),
        },
    });
}

fn plan_tasks(patterns: &[String], action: TaskAction, snap: &Snapshot, ops: &mut Vec<Op>) {
    for pattern in patterns {
        let matches: Vec<&TaskInfo> = snap
            .tasks
            .iter()
            .filter(|t| glob(pattern, &t.path))
            .collect();
        if matches.is_empty() {
            ops.push(Op {
                kind: OpKind::TaskPattern {
                    pattern: pattern.clone(),
                },
                state: OpState::Absent,
                detail: String::new(),
            });
            continue;
        }
        for task in matches {
            let (kind, state) = match action {
                TaskAction::Disable => (
                    OpKind::TaskDisable {
                        path: task.path.clone(),
                    },
                    if task.enabled {
                        OpState::WillApply
                    } else {
                        OpState::AlreadyDone
                    },
                ),
                TaskAction::Delete => (
                    OpKind::TaskDelete {
                        path: task.path.clone(),
                    },
                    OpState::WillApply,
                ),
            };
            ops.push(Op {
                kind,
                state,
                detail: String::new(),
            });
        }
    }
}

fn plan_kill(processes: &[String], snap: &Snapshot, ops: &mut Vec<Op>) {
    for name in processes {
        let running = snap.processes.iter().any(|p| {
            p.trim_end_matches(".exe")
                .eq_ignore_ascii_case(name.trim_end_matches(".exe"))
        });
        ops.push(Op {
            kind: OpKind::Kill { name: name.clone() },
            state: if running {
                OpState::WillApply
            } else {
                OpState::Absent
            },
            detail: String::new(),
        });
    }
}

fn plan_run(candidates: &[String], args: &[String], sys: &dyn Inspect, ops: &mut Vec<Op>) {
    for candidate in candidates {
        let exe = PathBuf::from(expand_env(candidate));
        if matches!(sys.path_info(&exe), Ok(Some(_))) {
            ops.push(Op {
                kind: OpKind::Run {
                    exe,
                    args: args.to_vec(),
                },
                state: OpState::WillApply,
                detail: String::new(),
            });
            return;
        }
    }
    ops.push(Op {
        kind: OpKind::Run {
            exe: PathBuf::from(expand_env(&candidates[0])),
            args: args.to_vec(),
        },
        state: OpState::Absent,
        detail: "no candidate executable found".into(),
    });
}

fn plan_delete(paths: &[String], sys: &dyn Inspect, ops: &mut Vec<Op>, notes: &mut Vec<String>) {
    for raw in paths {
        let path = PathBuf::from(expand_env(raw));
        match sys.path_info(&path) {
            Ok(Some(info)) => {
                let detail = format!("{} files, {}", info.files, human_bytes(info.bytes));
                if info.files > 0 {
                    notes.push(format!(
                        "{} holds {} files ({}).",
                        path.display(),
                        info.files,
                        human_bytes(info.bytes)
                    ));
                }
                ops.push(Op {
                    kind: OpKind::Delete { path },
                    state: OpState::WillApply,
                    detail,
                });
            }
            _ => ops.push(Op {
                kind: OpKind::Delete { path },
                state: OpState::Absent,
                detail: String::new(),
            }),
        }
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::Fake;

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
patterns = ["*Demo*"]

[[item]]
id = "services.demo"
name = "Demo service"
category = "services"
level = "high"
risk = "low"
summary = "x"
warning = "w"
[[item.step]]
kind = "service"
names = ["DemoSvc"]
startup = "disabled"

[[item]]
id = "onedrive.folder"
name = "Folder"
category = "onedrive"
level = "max"
risk = "high"
summary = "x"
warning = "Gone."
requires = ["services.demo"]
[[item.step]]
kind = "delete"
paths = ["%USERPROFILE%\\OneDrive"]

[[item]]
id = "privacy.reg"
name = "Reg"
category = "privacy"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "registry"
hive = "hklm"
path = "SOFTWARE\\Demo"
name = "Flag"
type = "dword"
value = 1
"#;

    fn catalog() -> Catalog {
        Catalog::parse("test", CATALOG).unwrap()
    }

    fn ids(plan: &Plan) -> Vec<&str> {
        plan.selected().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn level_high_selects_medium_and_high_items() {
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::High),
            22631,
            false,
            &Fake::default(),
        );
        assert_eq!(
            ids(&plan),
            vec!["appx.demo", "services.demo", "privacy.reg"]
        );
    }

    #[test]
    fn skip_removes_item() {
        let mut sel = Selection::level(Level::High);
        sel.skip.insert("appx.demo".into());
        let plan = build_plan(&catalog(), &sel, 22631, false, &Fake::default());
        assert_eq!(ids(&plan), vec!["services.demo", "privacy.reg"]);
    }

    #[test]
    fn extra_drags_required_dependency() {
        let mut sel = Selection::level(Level::Medium);
        sel.extra.insert("onedrive.folder".into());
        let plan = build_plan(&catalog(), &sel, 22631, false, &Fake::default());
        assert!(ids(&plan).contains(&"services.demo"));
        assert!(ids(&plan).contains(&"onedrive.folder"));
    }

    #[test]
    fn skipped_dependency_unselects_dependant() {
        let mut sel = Selection::level(Level::Max);
        sel.skip.insert("services.demo".into());
        let plan = build_plan(&catalog(), &sel, 22631, false, &Fake::default());
        assert!(!ids(&plan).contains(&"onedrive.folder"));
    }

    #[test]
    fn appx_pattern_matches_installed_and_flags_non_removable() {
        let sys = Fake::default()
            .with_package("Microsoft.DemoApp", false)
            .with_package("Microsoft.DemoCore", true)
            .provisioned(Provisioned::Known(vec![
                "Microsoft.DemoApp_8wekyb3d8bbwe".into(),
            ]));
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let item = &plan.items[0];
        let states: Vec<OpState> = item.ops.iter().map(|o| o.state).collect();
        assert_eq!(
            states,
            vec![
                OpState::WillApply,
                OpState::NonRemovable,
                OpState::WillApply
            ]
        );
        assert!(matches!(item.ops[2].kind, OpKind::Deprovision { .. }));
    }

    #[test]
    fn appx_pattern_without_matches_is_absent() {
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            true,
            &Fake::default(),
        );
        assert_eq!(plan.items[0].ops[0].state, OpState::Absent);
    }

    #[test]
    fn provisioned_needs_elevation_when_unelevated() {
        let sys = Fake::default().provisioned(Provisioned::NeedsElevation);
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            false,
            &sys,
        );
        assert!(plan.items[0].needs_elevation());
    }

    #[test]
    fn service_with_same_startup_is_already_done_and_running_service_is_stopped() {
        let sys = Fake::default().with_service("DemoSvc", Startup::Disabled, true);
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::High),
            22631,
            true,
            &sys,
        );
        let ops = &plan.items[1].ops;
        assert!(matches!(ops[0].kind, OpKind::StopService { .. }));
        assert_eq!(ops[0].state, OpState::WillApply);
        assert_eq!(ops[1].state, OpState::AlreadyDone);
    }

    #[test]
    fn registry_value_already_set_is_already_done() {
        let sys =
            Fake::default().with_registry(Hive::Hklm, "SOFTWARE\\Demo", "Flag", RegValue::Dword(1));
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        assert_eq!(plan.items[3].ops[0].state, OpState::AlreadyDone);
    }

    #[test]
    fn delete_warning_carries_live_count() {
        let folder = expand_env("%USERPROFILE%\\OneDrive");
        let sys = Fake::default().with_path(&folder, 1342, 5_100_000_000);
        let plan = build_plan(&catalog(), &Selection::level(Level::Max), 22631, true, &sys);
        let warning = plan.items[2].warning.as_deref().unwrap();
        assert!(warning.starts_with("Gone. "), "{warning}");
        assert!(warning.contains("1342 files (4.7 GB)"), "{warning}");
    }

    #[test]
    fn reselect_follows_new_level() {
        let mut plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            false,
            &Fake::default(),
        );
        assert_eq!(ids(&plan), vec!["appx.demo", "privacy.reg"]);
        plan.reselect(&Selection::level(Level::Max));
        assert_eq!(ids(&plan).len(), 4);
    }

    const TASK_CATALOG: &str = r#"
[[item]]
id = "telemetry.tasks"
name = "Tasks"
category = "telemetry"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "task"
action = "disable"
patterns = ["\\Microsoft\\Windows\\Demo\\*", "\\Nope\\*"]
[[item.step]]
kind = "kill"
processes = ["DemoApp", "Ghost"]
"#;

    #[test]
    fn tasks_and_processes_are_matched_against_the_snapshot() {
        let catalog = Catalog::parse("t", TASK_CATALOG).unwrap();
        let sys = Fake::default()
            .with_task("\\Microsoft\\Windows\\Demo\\Collector", true)
            .with_task("\\Microsoft\\Windows\\Demo\\Uploader", false)
            .with_process("DemoApp.exe");
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let states: Vec<OpState> = plan.items[0].ops.iter().map(|o| o.state).collect();
        assert_eq!(
            states,
            vec![
                OpState::WillApply,
                OpState::AlreadyDone,
                OpState::Absent,
                OpState::WillApply,
                OpState::Absent
            ]
        );
    }
}
