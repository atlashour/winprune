use super::expand_env;
use crate::catalog::{
    Catalog, Category, Hive, Item, Level, RegType, Risk, Scope, Startup, Step, TaskAction,
};
use crate::system::{
    AppxPackage, Inspect, ProcessInfo, Provisioned, RegRoot, RegValue, TaskInfo, UserProfile,
    lives_under,
};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

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

    fn wants(&self, id: &str, level: Level) -> bool {
        if self.skip.contains(id) {
            return false;
        }
        if self.extra.contains(id) {
            return true;
        }
        match &self.only {
            Some(only) => only.contains(id),
            None => self.level >= level,
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
        root: RegRoot,
        path: String,
        name: String,
        value: RegValue,
    },
    RegistryDelete {
        root: RegRoot,
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
        skip_exit_codes: Vec<i32>,
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
                root,
                path,
                name,
                value,
            } => {
                write!(f, "{root}\\{path}\\{name} = {}", show_value(value))
            }
            OpKind::RegistryDelete { root, path, name } => {
                write!(f, "delete {root}\\{path}\\{name}")
            }
            OpKind::TaskDisable { path } => write!(f, "disable task {path}"),
            OpKind::TaskDelete { path } => write!(f, "delete task {path}"),
            OpKind::TaskPattern { pattern } => write!(f, "task {pattern}"),
            OpKind::Kill { name } => write!(f, "kill {name}"),
            OpKind::Run { exe, args, .. } => write!(f, "run {} {}", exe.display(), args.join(" ")),
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

impl Op {
    fn new(kind: OpKind, state: OpState) -> Op {
        Op {
            kind,
            state,
            detail: String::new(),
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Op {
        self.detail = detail.into();
        self
    }
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
    /// Account the per-user steps are written for, when it could be resolved.
    pub user: Option<String>,
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
        .filter(|(id, _, level)| selection.wants(id, *level))
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
        user: snapshot.interactive.as_ref().map(|p| p.name.clone()),
        items,
    }
}

/// Lists that are expensive to fetch are read once per plan.
struct Snapshot {
    packages: Vec<AppxPackage>,
    provisioned: Provisioned,
    tasks: Vec<TaskInfo>,
    processes: Vec<ProcessInfo>,
    profiles: Vec<UserProfile>,
    interactive: Option<UserProfile>,
    default_profile: Option<PathBuf>,
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
            profiles: sys.user_profiles().unwrap_or_default(),
            interactive: sys.interactive_user(),
            default_profile: sys.default_profile_path(),
        }
    }

    /// Registry roots a per-user step must reach for the given scope.
    fn user_roots(&self, scope: Scope) -> Vec<RegRoot> {
        let interactive = match &self.interactive {
            Some(p) => RegRoot::User {
                sid: p.sid.clone(),
                name: p.name.clone(),
            },
            None => RegRoot::CurrentUser,
        };
        match scope {
            Scope::User => vec![interactive],
            Scope::DefaultProfile => vec![RegRoot::DefaultProfile],
            Scope::AllUsers => {
                let mut roots = vec![interactive];
                let own = self.interactive.as_ref().map(|p| p.sid.as_str());
                for p in self
                    .profiles
                    .iter()
                    .filter(|p| p.loaded && Some(p.sid.as_str()) != own)
                {
                    roots.push(RegRoot::User {
                        sid: p.sid.clone(),
                        name: p.name.clone(),
                    });
                }
                roots.push(RegRoot::DefaultProfile);
                roots
            }
        }
    }

    /// Profile folders a per-user delete step must reach. `None` as the folder means
    /// "the current process environment", used when the interactive user is unknown.
    fn user_folders(&self, scope: Scope) -> Vec<(String, Option<PathBuf>)> {
        let interactive = match &self.interactive {
            Some(p) => (p.name.clone(), Some(p.path.clone())),
            None => ("current user".to_string(), None),
        };
        match scope {
            Scope::User => vec![interactive],
            Scope::DefaultProfile => vec![("Default".to_string(), self.default_profile.clone())],
            Scope::AllUsers => {
                let mut out = vec![interactive];
                let own = self.interactive.as_ref().map(|p| p.sid.as_str());
                for p in self.profiles.iter().filter(|p| Some(p.sid.as_str()) != own) {
                    out.push((p.name.clone(), Some(p.path.clone())));
                }
                out.push(("Default".to_string(), self.default_profile.clone()));
                out
            }
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
                scope,
            } => {
                let roots = match hive {
                    Hive::Hklm => vec![RegRoot::Machine],
                    Hive::Hkcr => vec![RegRoot::Classes],
                    Hive::Hkcu => snap.user_roots(*scope),
                };
                for root in roots {
                    plan_registry(
                        root,
                        path,
                        name,
                        *kind,
                        value.as_ref(),
                        *delete,
                        sys,
                        &mut ops,
                    );
                }
            }
            Step::Task { patterns, action } => plan_tasks(patterns, *action, snap, &mut ops),
            Step::Kill { processes } => plan_kill(processes, snap, &mut ops),
            Step::Run {
                candidates,
                args,
                skip_exit_codes,
            } => plan_run(candidates, args, skip_exit_codes, sys, &mut ops),
            Step::Delete { paths, scope } => {
                for (who, folder) in snap.user_folders(*scope) {
                    plan_delete(
                        paths,
                        folder.as_deref(),
                        &who,
                        snap,
                        sys,
                        &mut ops,
                        &mut live_notes,
                    );
                }
            }
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

/// Deprovisioning comes first: removing a registered package while it is still
/// provisioned lets Windows register it again for the next user.
fn plan_appx(patterns: &[String], snap: &Snapshot, ops: &mut Vec<Op>) {
    for pattern in patterns {
        let mut matched = false;
        if let Provisioned::Known(families) = &snap.provisioned {
            for family in families.iter().filter(|f| glob(pattern, f)) {
                matched = true;
                ops.push(Op::new(
                    OpKind::Deprovision {
                        family: family.clone(),
                    },
                    OpState::WillApply,
                ));
            }
        }
        for pkg in snap.packages.iter().filter(|p| glob(pattern, &p.name)) {
            matched = true;
            ops.push(Op::new(
                OpKind::RemovePackage {
                    full_name: pkg.full_name.clone(),
                    name: pkg.name.clone(),
                },
                if pkg.non_removable {
                    OpState::NonRemovable
                } else {
                    OpState::WillApply
                },
            ));
        }
        if !matched {
            ops.push(Op::new(
                OpKind::PackagePattern {
                    pattern: pattern.clone(),
                },
                OpState::Absent,
            ));
        }
    }
    // one line per step, not per pattern
    if matches!(snap.provisioned, Provisioned::NeedsElevation) {
        ops.push(
            Op::new(
                OpKind::PackagePattern {
                    pattern: "provisioned packages".into(),
                },
                OpState::NeedsElevation,
            )
            .with_detail("not checked without elevation"),
        );
    }
}

fn plan_services(names: &[String], startup: Startup, sys: &dyn Inspect, ops: &mut Vec<Op>) {
    for name in names {
        let kind = OpKind::ServiceStartup {
            name: name.clone(),
            startup,
        };
        match sys.service(name) {
            Ok(Some(info)) => {
                if startup == Startup::Disabled && info.running {
                    ops.push(Op::new(
                        OpKind::StopService { name: name.clone() },
                        OpState::WillApply,
                    ));
                }
                let state = if info.startup == startup {
                    OpState::AlreadyDone
                } else {
                    OpState::WillApply
                };
                ops.push(Op::new(kind, state).with_detail(format!("currently {}", info.startup)));
            }
            Ok(None) => ops.push(Op::new(kind, OpState::Absent)),
            Err(e) => ops.push(
                Op::new(kind, OpState::WillApply).with_detail(format!("could not inspect: {e}")),
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_registry(
    root: RegRoot,
    path: &str,
    name: &str,
    kind: Option<RegType>,
    value: Option<&toml::Value>,
    delete: bool,
    sys: &dyn Inspect,
    ops: &mut Vec<Op>,
) {
    // unreadable is not the same as absent; let apply try
    let (current, unreadable) = match sys.registry_value(&root, path, name) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("could not read: {e}"))),
    };
    if delete {
        let state = if current.is_some() || unreadable.is_some() {
            OpState::WillApply
        } else {
            OpState::Absent
        };
        ops.push(
            Op::new(
                OpKind::RegistryDelete {
                    root,
                    path: path.to_string(),
                    name: name.to_string(),
                },
                state,
            )
            .with_detail(unreadable.unwrap_or_default()),
        );
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
    let detail = match (current, unreadable) {
        (Some(v), _) => format!("currently {}", show_value(&v)),
        (None, Some(why)) => why,
        (None, None) => String::new(),
    };
    ops.push(
        Op::new(
            OpKind::RegistrySet {
                root,
                path: path.to_string(),
                name: name.to_string(),
                value: wanted,
            },
            state,
        )
        .with_detail(detail),
    );
}

fn plan_tasks(patterns: &[String], action: TaskAction, snap: &Snapshot, ops: &mut Vec<Op>) {
    for pattern in patterns {
        let matches: Vec<&TaskInfo> = snap
            .tasks
            .iter()
            .filter(|t| glob(pattern, &t.path))
            .collect();
        if matches.is_empty() {
            ops.push(Op::new(
                OpKind::TaskPattern {
                    pattern: pattern.clone(),
                },
                OpState::Absent,
            ));
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
            ops.push(Op::new(kind, state));
        }
    }
}

fn same_process(a: &str, b: &str) -> bool {
    a.trim_end_matches(".exe")
        .eq_ignore_ascii_case(b.trim_end_matches(".exe"))
}

fn plan_kill(processes: &[String], snap: &Snapshot, ops: &mut Vec<Op>) {
    for name in processes {
        let running = snap.processes.iter().any(|p| same_process(&p.name, name));
        ops.push(Op::new(
            OpKind::Kill { name: name.clone() },
            if running {
                OpState::WillApply
            } else {
                OpState::Absent
            },
        ));
    }
}

fn plan_run(
    candidates: &[String],
    args: &[String],
    skip_exit_codes: &[i32],
    sys: &dyn Inspect,
    ops: &mut Vec<Op>,
) {
    for candidate in candidates {
        let exe = PathBuf::from(expand_env(candidate));
        if matches!(sys.path_info(&exe), Ok(Some(_))) {
            ops.push(Op::new(
                OpKind::Run {
                    exe,
                    args: args.to_vec(),
                    skip_exit_codes: skip_exit_codes.to_vec(),
                },
                OpState::WillApply,
            ));
            return;
        }
    }
    ops.push(
        Op::new(
            OpKind::Run {
                exe: PathBuf::from(expand_env(&candidates[0])),
                args: args.to_vec(),
                skip_exit_codes: skip_exit_codes.to_vec(),
            },
            OpState::Absent,
        )
        .with_detail("no candidate executable found"),
    );
}

/// Expands the profile variables against a specific profile folder so a step can reach
/// accounts other than the one running winprune.
pub fn expand_for_profile(raw: &str, folder: Option<&std::path::Path>) -> PathBuf {
    let Some(folder) = folder else {
        return PathBuf::from(expand_env(raw));
    };
    let base = folder.to_string_lossy();
    let upper = raw.to_ascii_uppercase();
    let replaced = if let Some(rest) = upper.strip_prefix("%USERPROFILE%") {
        format!("{base}{}", &raw[raw.len() - rest.len()..])
    } else if let Some(rest) = upper.strip_prefix("%LOCALAPPDATA%") {
        format!("{base}\\AppData\\Local{}", &raw[raw.len() - rest.len()..])
    } else if let Some(rest) = upper.strip_prefix("%APPDATA%") {
        format!("{base}\\AppData\\Roaming{}", &raw[raw.len() - rest.len()..])
    } else {
        raw.to_string()
    };
    PathBuf::from(expand_env(&replaced))
}

/// A running executable inside a tree makes the whole delete fail with access denied,
/// so the plan stops it first. Kills are by name, the way a `kill` step works, which
/// is why a name matching this very executable is never planned: it would end the run.
fn plan_kill_residents(tree: &Path, snap: &Snapshot, ops: &mut Vec<Op>) {
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    for p in &snap.processes {
        let Some(path) = &p.path else {
            continue;
        };
        let planned = ops
            .iter()
            .any(|o| matches!(&o.kind, OpKind::Kill { name } if same_process(name, &p.name)));
        if !lives_under(path, tree)
            || planned
            || own.as_deref().is_some_and(|o| same_process(o, &p.name))
        {
            continue;
        }
        ops.push(
            Op::new(
                OpKind::Kill {
                    name: p.name.clone(),
                },
                OpState::WillApply,
            )
            .with_detail(format!("runs from {}", tree.display())),
        );
    }
}

fn plan_delete(
    paths: &[String],
    folder: Option<&std::path::Path>,
    who: &str,
    snap: &Snapshot,
    sys: &dyn Inspect,
    ops: &mut Vec<Op>,
    notes: &mut Vec<String>,
) {
    for raw in paths {
        let path = expand_for_profile(raw, folder);
        match sys.path_info(&path) {
            Ok(Some(info)) => {
                plan_kill_residents(&path, snap, ops);
                if info.files > 0 {
                    notes.push(format!(
                        "{} holds {} files ({}).",
                        path.display(),
                        info.files,
                        human_bytes(info.bytes)
                    ));
                }
                ops.push(
                    Op::new(OpKind::Delete { path }, OpState::WillApply).with_detail(format!(
                        "{who}: {} files, {}",
                        info.files,
                        human_bytes(info.bytes)
                    )),
                );
            }
            Ok(None) => ops.push(Op::new(OpKind::Delete { path }, OpState::Absent)),
            Err(e) => ops.push(
                Op::new(OpKind::Delete { path }, OpState::WillApply)
                    .with_detail(format!("{who}: could not inspect: {e}")),
            ),
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
    fn appx_deprovisions_first() {
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
        assert!(matches!(item.ops[0].kind, OpKind::Deprovision { .. }));
        let states: Vec<OpState> = item.ops.iter().map(|o| o.state).collect();
        assert_eq!(
            states,
            vec![
                OpState::WillApply,
                OpState::WillApply,
                OpState::NonRemovable
            ]
        );
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
    fn running_service_gets_a_stop_op() {
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
        let sys = Fake::default().with_registry(
            &RegRoot::Machine,
            "SOFTWARE\\Demo",
            "Flag",
            RegValue::Dword(1),
        );
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

    const DELETE_CATALOG: &str = r#"
[[item]]
id = "onedrive.temp"
name = "Temp"
category = "onedrive"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "delete"
paths = ["C:\\OneDriveTemp"]
"#;

    #[test]
    fn delete_step_stops_processes_running_from_the_tree_first() {
        let catalog = Catalog::parse("t", DELETE_CATALOG).unwrap();
        let sys = Fake::default()
            .with_path("C:\\OneDriveTemp", 3, 300)
            .with_process_at("Resident.exe", "c:\\onedrivetemp\\bin\\Resident.exe")
            .with_process_at("Neighbour.exe", "C:\\OneDriveTempX\\Neighbour.exe")
            .with_process("Elsewhere.exe");
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let ops: Vec<String> = plan.items[0]
            .ops
            .iter()
            .map(|o| o.kind.to_string())
            .collect();
        assert_eq!(ops, vec!["kill Resident.exe", "delete C:\\OneDriveTemp"]);
    }

    #[test]
    fn delete_step_never_plans_a_kill_of_winprune_itself() {
        // Kills are by name, so a copy of our own executable inside the tree would
        // take this very process down with it.
        let own = std::env::current_exe().unwrap();
        let own = own.file_name().unwrap().to_string_lossy().to_string();
        let catalog = Catalog::parse("t", DELETE_CATALOG).unwrap();
        let sys = Fake::default()
            .with_path("C:\\OneDriveTemp", 3, 300)
            .with_process_at(&own, &format!("C:\\OneDriveTemp\\Desktop\\{own}"));
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let ops: Vec<String> = plan.items[0]
            .ops
            .iter()
            .map(|o| o.kind.to_string())
            .collect();
        assert_eq!(ops, vec!["delete C:\\OneDriveTemp"]);
    }

    const KILL_THEN_DELETE_CATALOG: &str = r#"
[[item]]
id = "onedrive.temp"
name = "Temp"
category = "onedrive"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "kill"
processes = ["Resident"]
[[item.step]]
kind = "delete"
paths = ["C:\\OneDriveTemp"]
"#;

    #[test]
    fn resident_kill_is_not_repeated_after_an_explicit_kill_step() {
        let catalog = Catalog::parse("t", KILL_THEN_DELETE_CATALOG).unwrap();
        let sys = Fake::default()
            .with_path("C:\\OneDriveTemp", 3, 300)
            .with_process_at("Resident.exe", "C:\\OneDriveTemp\\Resident.exe");
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let ops: Vec<String> = plan.items[0]
            .ops
            .iter()
            .map(|o| o.kind.to_string())
            .collect();
        assert_eq!(ops, vec!["kill Resident", "delete C:\\OneDriveTemp"]);
    }

    const SCOPE_CATALOG: &str = r#"
[[item]]
id = "privacy.users"
name = "Users"
category = "privacy"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "registry"
hive = "hkcu"
scope = "all-users"
path = "Software\\Demo"
name = "Flag"
type = "dword"
value = 0
[[item.step]]
kind = "delete"
scope = "all-users"
paths = ["%LOCALAPPDATA%\\Demo"]
"#;

    #[test]
    fn all_users_scope() {
        let catalog = Catalog::parse("t", SCOPE_CATALOG).unwrap();
        let sys = Fake::default()
            .with_profile("S-1-5-21-1", "alice", "C:\\Users\\alice", true)
            .with_profile("S-1-5-21-2", "bob", "C:\\Users\\bob", true)
            .with_profile("S-1-5-21-3", "carol", "C:\\Users\\carol", false)
            .interactive("S-1-5-21-1")
            .with_path("C:\\Users\\bob\\AppData\\Local\\Demo", 3, 300);
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let ops = &plan.items[0].ops;
        let roots: Vec<String> = ops
            .iter()
            .filter_map(|o| match &o.kind {
                OpKind::RegistrySet { root, .. } => Some(root.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(roots, vec!["HKU:alice", "HKU:bob", "HKU:Default"]);
        let deletes: Vec<(String, OpState)> = ops
            .iter()
            .filter_map(|o| match &o.kind {
                OpKind::Delete { path } => Some((path.display().to_string(), o.state)),
                _ => None,
            })
            .collect();
        assert_eq!(deletes.len(), 4, "{deletes:?}");
        assert_eq!(
            deletes[1],
            (
                "C:\\Users\\bob\\AppData\\Local\\Demo".to_string(),
                OpState::WillApply
            )
        );
        assert_eq!(deletes[3].0, "C:\\Users\\Default\\AppData\\Local\\Demo");
        assert_eq!(plan.user.as_deref(), Some("alice"));
    }

    #[test]
    fn user_scope_falls_back_to_hkcu() {
        let catalog =
            Catalog::parse("t", SCOPE_CATALOG.replace("all-users", "user").as_str()).unwrap();
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            false,
            &Fake::default(),
        );
        assert!(matches!(
            &plan.items[0].ops[0].kind,
            OpKind::RegistrySet {
                root: RegRoot::CurrentUser,
                ..
            }
        ));
    }
}
