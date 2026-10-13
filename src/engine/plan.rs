use super::expand_env;
use crate::catalog::{
    Catalog, Category, Hive, Item, Level, RegProbe, RegType, Risk, Scope, Startup, Step,
    TaskAction, profile_var,
};
use crate::system::{
    AppxPackage, Inspect, ProcessInfo, Provisioned, RegRoot, RegValue, SysError, TaskInfo,
    UserProfile, lives_under,
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

    pub(crate) fn wants(&self, id: &str, level: Level) -> bool {
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
    /// Stands for a list the plan could not read, so whatever it holds was not seen.
    NotListed {
        what: String,
    },
    Unsupported {
        reason: String,
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
            OpKind::NotListed { what } => write!(f, "{what}"),
            OpKind::Unsupported { reason } => write!(f, "{reason}"),
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
    /// The current state could not be read, so `state` is a guess that lets apply try.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unverified: bool,
}

impl Op {
    fn new(kind: OpKind, state: OpState) -> Op {
        Op {
            kind,
            state,
            detail: String::new(),
            unverified: false,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Op {
        self.detail = detail.into();
        self
    }

    fn unverified(mut self, unverified: bool) -> Op {
        self.unverified = unverified;
        self
    }

    /// Kills are a precondition of other ops, not a state the item leaves behind.
    pub fn is_state(&self) -> bool {
        !matches!(self.kind, OpKind::Kill { .. })
    }

    /// Applying would change something the plan could read.
    pub fn pending(&self) -> bool {
        self.state == OpState::WillApply && !self.unverified
    }

    /// The current state is not known: a read failed, or only an elevated process sees it.
    pub fn unread(&self) -> bool {
        self.unverified || self.state == OpState::NeedsElevation
    }

    /// Short label for listings. An unread op must not look like a known one.
    pub fn tag(&self) -> &'static str {
        match (self.state, self.unverified) {
            (OpState::WillApply, true) => "apply?",
            (OpState::WillApply, false) => "apply",
            (OpState::Absent | OpState::AlreadyDone, true) => "unread",
            (OpState::Absent, false) => "absent",
            (OpState::AlreadyDone, false) => "done",
            (OpState::NonRemovable, _) => "keep",
            (OpState::NeedsElevation, _) => "elev",
        }
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

    pub fn state_ops(&self) -> impl Iterator<Item = &Op> {
        self.ops.iter().filter(|op| op.is_state())
    }

    pub fn has_unread(&self) -> bool {
        self.state_ops().any(Op::unread)
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

fn split<T: Default>(result: Result<T, crate::system::SysError>) -> (T, Option<String>) {
    match result {
        Ok(v) => (v, None),
        Err(e) => (T::default(), Some(e.to_string())),
    }
}

/// Lists that are expensive to fetch are read once per plan. Each `*_error` says why
/// its list could not be read; the empty list it leaves then means nothing.
struct Snapshot {
    packages: Vec<AppxPackage>,
    packages_error: Option<String>,
    provisioned: Provisioned,
    provisioned_error: Option<String>,
    tasks: Vec<TaskInfo>,
    tasks_error: Option<String>,
    processes: Vec<ProcessInfo>,
    processes_error: Option<String>,
    profiles: Vec<UserProfile>,
    profiles_error: Option<String>,
    interactive: Option<UserProfile>,
    default_profile: Option<PathBuf>,
}

impl Snapshot {
    fn take(sys: &dyn Inspect) -> Self {
        let (packages, packages_error) = split(sys.installed_packages());
        let (tasks, tasks_error) = split(sys.tasks());
        let (processes, processes_error) = split(sys.running_processes());
        let (profiles, profiles_error) = split(sys.user_profiles());
        let (provisioned, provisioned_error) = match sys.provisioned_packages() {
            Ok(p) => (p, None),
            Err(e) => (Provisioned::Known(Vec::new()), Some(e.to_string())),
        };
        Snapshot {
            packages,
            packages_error,
            provisioned,
            provisioned_error,
            tasks,
            tasks_error,
            processes,
            processes_error,
            profiles,
            profiles_error,
            interactive: sys.interactive_user(),
            default_profile: sys.default_profile_path(),
        }
    }

    /// Marks a step scoped to every account when the other accounts could not be listed.
    fn other_profiles_op(&self, scope: Scope) -> Option<Op> {
        let e = self
            .profiles_error
            .as_ref()
            .filter(|_| scope == Scope::AllUsers)?;
        Some(not_listed(
            "other user profiles",
            &format!("could not list user profiles: {e}"),
        ))
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
                if *hive == Hive::Hkcu {
                    ops.extend(snap.other_profiles_op(*scope));
                }
                for root in registry_roots(*hive, *scope, snap) {
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
                when_registered,
            } => plan_run(
                candidates,
                args,
                skip_exit_codes,
                when_registered,
                snap,
                sys,
                &mut ops,
            ),
            Step::Unsupported {
                when_registered,
                reason,
            } => ops.push(plan_unsupported(when_registered, reason, snap, sys)),
            Step::Delete { paths, scope } => {
                ops.extend(snap.other_profiles_op(*scope));
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
        // without the installed list a miss means nothing; the line below stands for it
        if !matched && snap.packages_error.is_none() {
            ops.push(Op::new(
                OpKind::PackagePattern {
                    pattern: pattern.clone(),
                },
                OpState::Absent,
            ));
        }
    }
    // one line per step, not per pattern
    for (list, error) in [
        ("installed packages", &snap.packages_error),
        ("provisioned packages", &snap.provisioned_error),
    ] {
        if let Some(e) = error {
            ops.push(not_listed(list, &format!("could not list: {e}")));
        }
    }
    if matches!(snap.provisioned, Provisioned::NeedsElevation) {
        ops.push(
            Op::new(
                OpKind::NotListed {
                    what: "provisioned packages".into(),
                },
                OpState::NeedsElevation,
            )
            .with_detail("not checked without elevation"),
        );
    }
}

fn not_listed(what: &str, why: &str) -> Op {
    Op::new(
        OpKind::NotListed {
            what: what.to_string(),
        },
        OpState::Absent,
    )
    .with_detail(why)
    .unverified(true)
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
                Op::new(kind, OpState::WillApply)
                    .with_detail(format!("could not inspect: {e}"))
                    .unverified(true),
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
        // apply could not write it either, so it is not part of the plan at all
        Err(SysError::Unreachable) => return,
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
            .unverified(unreadable.is_some())
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
    let unverified = unreadable.is_some();
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
        .with_detail(detail)
        .unverified(unverified),
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
            let op = Op::new(
                OpKind::TaskPattern {
                    pattern: pattern.clone(),
                },
                OpState::Absent,
            );
            ops.push(match &snap.tasks_error {
                Some(e) => op
                    .with_detail(format!("could not list tasks: {e}"))
                    .unverified(true),
                None => op,
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
        let kind = OpKind::Kill { name: name.clone() };
        // without the process list, let apply try; killing what is not running is a no-op
        if let Some(e) = &snap.processes_error {
            ops.push(
                Op::new(kind, OpState::WillApply)
                    .with_detail(format!("could not list processes: {e}"))
                    .unverified(true),
            );
            continue;
        }
        let running = snap.processes.iter().any(|p| same_process(&p.name, name));
        ops.push(Op::new(
            kind,
            if running {
                OpState::WillApply
            } else {
                OpState::Absent
            },
        ));
    }
}

enum Presence {
    Present,
    Missing,
    Unknown(String),
}

fn registry_roots(hive: Hive, scope: Scope, snap: &Snapshot) -> Vec<RegRoot> {
    match hive {
        Hive::Hklm => vec![RegRoot::Machine],
        Hive::Hkcr => vec![RegRoot::Classes],
        Hive::Hkcu => snap.user_roots(scope),
    }
}

/// Who a probe asks about. A command acts for the account that runs it, which is not
/// the interactive user when another administrator answered the UAC prompt.
#[derive(Clone, Copy)]
enum ProbeFor {
    Interactive,
    RunningAccount,
}

/// One value found is enough; otherwise a value that could not be read, or other
/// profiles that could not be listed for an all-users probe, leave it unknown.
fn presence(probes: &[RegProbe], who: ProbeFor, snap: &Snapshot, sys: &dyn Inspect) -> Presence {
    let mut unknown = None;
    for probe in probes {
        let roots = match (who, probe.hive, probe.scope) {
            (ProbeFor::RunningAccount, Hive::Hkcu, Scope::User) => vec![RegRoot::CurrentUser],
            _ => registry_roots(probe.hive, probe.scope, snap),
        };
        for root in roots {
            match sys.registry_value(&root, &probe.path, &probe.name) {
                Ok(Some(_)) => return Presence::Present,
                Ok(None) | Err(SysError::Unreachable) => {}
                Err(e) => {
                    unknown.get_or_insert_with(|| {
                        format!("could not read {root}\\{}\\{}: {e}", probe.path, probe.name)
                    });
                }
            }
        }
    }
    if let Some(why) = unknown {
        return Presence::Unknown(why);
    }
    match &snap.profiles_error {
        Some(e) if probes.iter().any(|p| p.scope == Scope::AllUsers) => {
            Presence::Unknown(format!("could not list user profiles: {e}"))
        }
        _ => Presence::Missing,
    }
}

fn plan_unsupported(probes: &[RegProbe], reason: &str, snap: &Snapshot, sys: &dyn Inspect) -> Op {
    let kind = OpKind::Unsupported {
        reason: reason.to_string(),
    };
    match presence(probes, ProbeFor::Interactive, snap, sys) {
        Presence::Present => {
            Op::new(kind, OpState::NonRemovable).with_detail("winprune does not remove it")
        }
        Presence::Missing => Op::new(kind, OpState::Absent),
        Presence::Unknown(why) => Op::new(kind, OpState::Absent)
            .with_detail(why)
            .unverified(true),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_run(
    candidates: &[String],
    args: &[String],
    skip_exit_codes: &[i32],
    when_registered: &[RegProbe],
    snap: &Snapshot,
    sys: &dyn Inspect,
    ops: &mut Vec<Op>,
) {
    let mut unchecked = None;
    for candidate in candidates {
        let exe = PathBuf::from(expand_env(candidate));
        match sys.exists(&exe) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                unchecked.get_or_insert_with(|| format!("could not check {}: {e}", exe.display()));
                continue;
            }
        }
        let kind = OpKind::Run {
            exe,
            args: args.to_vec(),
            skip_exit_codes: skip_exit_codes.to_vec(),
        };
        let presence = if when_registered.is_empty() {
            Presence::Present
        } else {
            presence(when_registered, ProbeFor::RunningAccount, snap, sys)
        };
        ops.push(match presence {
            Presence::Present => Op::new(kind, OpState::WillApply),
            Presence::Missing => {
                Op::new(kind, OpState::AlreadyDone).with_detail("nothing it removes is present")
            }
            Presence::Unknown(why) => Op::new(kind, OpState::WillApply)
                .with_detail(why)
                .unverified(true),
        });
        return;
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
        .unverified(unchecked.is_some())
        .with_detail(unchecked.unwrap_or_else(|| "no candidate executable found".into())),
    );
}

/// Expands the profile variables against a specific profile folder so a step can reach
/// accounts other than the one running winprune.
pub fn expand_for_profile(raw: &str, folder: Option<&std::path::Path>) -> PathBuf {
    let Some(folder) = folder else {
        return PathBuf::from(expand_env(raw));
    };
    let replaced = match profile_var(raw) {
        Some((var, sub)) => format!("{}{sub}{}", folder.to_string_lossy(), &raw[var.len()..]),
        None => raw.to_string(),
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
                        "{} holds {} ({}).",
                        path.display(),
                        count(info.files, "file", "files"),
                        human_bytes(info.bytes)
                    ));
                }
                ops.push(
                    Op::new(OpKind::Delete { path }, OpState::WillApply).with_detail(format!(
                        "{who}: {}, {}",
                        count(info.files, "file", "files"),
                        human_bytes(info.bytes)
                    )),
                );
            }
            Ok(None) => ops.push(Op::new(OpKind::Delete { path }, OpState::Absent)),
            Err(e) => ops.push(
                Op::new(OpKind::Delete { path }, OpState::WillApply)
                    .with_detail(format!("{who}: could not inspect: {e}"))
                    .unverified(true),
            ),
        }
    }
}

pub fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
        assert!(plan.items[0].has_unread());
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
    fn unreadable_registry_value_is_planned_but_unverified() {
        let sys = Fake::default().unreadable(&RegRoot::Machine);
        let plan = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            false,
            &sys,
        );
        let op = &plan.items[3].ops[0];
        assert_eq!(op.state, OpState::WillApply);
        assert!(op.unverified);
        let readable = build_plan(
            &catalog(),
            &Selection::level(Level::Medium),
            22631,
            false,
            &Fake::default(),
        );
        assert!(!readable.items[3].ops[0].unverified);
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

    const RUN_CATALOG: &str = r#"
[[item]]
id = "onedrive.setup"
name = "Setup"
category = "onedrive"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "run"
candidates = ["C:\\Setup.exe"]
args = ["/uninstall"]
when_registered = [{ hive = "hkcu", path = "Software\\Demo", name = "Version" }, { hive = "hklm", path = "SOFTWARE\\Demo", name = "Version" }]
"#;

    fn alice() -> RegRoot {
        RegRoot::User {
            sid: "S-1-5-21-1".into(),
            name: "alice".into(),
        }
    }

    fn run_base() -> Fake {
        Fake::default()
            .with_profile("S-1-5-21-1", "alice", "C:\\Users\\alice", true)
            .interactive("S-1-5-21-1")
            .with_path("C:\\Setup.exe", 1, 1)
    }

    fn run_op_in(catalog: &str, sys: &Fake) -> Op {
        let catalog = Catalog::parse("t", catalog).unwrap();
        let plan = build_plan(&catalog, &Selection::level(Level::Medium), 22631, true, sys);
        plan.items[0].ops[0].clone()
    }

    fn run_op(sys: &Fake) -> Op {
        run_op_in(RUN_CATALOG, sys)
    }

    #[test]
    fn run_step_is_done_when_nothing_it_removes_is_registered() {
        assert_eq!(run_op(&run_base()).state, OpState::AlreadyDone);
        let user = run_base().with_registry(
            &RegRoot::CurrentUser,
            "Software\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        assert_eq!(run_op(&user).state, OpState::WillApply);
        let machine = run_base().with_registry(
            &RegRoot::Machine,
            "SOFTWARE\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        assert_eq!(run_op(&machine).state, OpState::WillApply);
    }

    #[test]
    fn run_step_asks_about_the_account_the_command_runs_as() {
        // another administrator answered UAC: the uninstaller acts for that account,
        // not for alice who started winprune
        let only_alice = run_base().with_registry(
            &alice(),
            "Software\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        assert_eq!(run_op(&only_alice).state, OpState::AlreadyDone);
        let running = run_base().with_registry(
            &RegRoot::CurrentUser,
            "Software\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        assert_eq!(run_op(&running).state, OpState::WillApply);
    }

    #[test]
    fn run_step_without_probe_always_runs() {
        let catalog = RUN_CATALOG
            .lines()
            .filter(|l| !l.starts_with("when_registered"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(run_op_in(&catalog, &run_base()).state, OpState::WillApply);
    }

    #[test]
    fn run_step_probe_that_cannot_be_read_is_unverified() {
        let op = run_op(&run_base().unreadable(&RegRoot::CurrentUser));
        assert_eq!(op.state, OpState::WillApply);
        assert!(op.unverified, "{op:?}");
        assert!(op.detail.starts_with("could not read"), "{}", op.detail);

        // one value found is enough, whatever else could not be read
        let found = run_base().unreadable(&RegRoot::CurrentUser).with_registry(
            &RegRoot::Machine,
            "SOFTWARE\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        let op = run_op(&found);
        assert_eq!(op.state, OpState::WillApply);
        assert!(!op.unverified);
    }

    #[test]
    fn all_users_probe_is_unverified_when_profiles_cannot_be_listed() {
        let all_users = RUN_CATALOG.replace(
            "{ hive = \"hkcu\", path = \"Software\\\\Demo\", name = \"Version\" }",
            "{ hive = \"hkcu\", scope = \"all-users\", path = \"Software\\\\Demo\", name = \"Version\" }",
        );
        assert_ne!(all_users, RUN_CATALOG);
        let sys = Fake::default()
            .with_path("C:\\Setup.exe", 1, 1)
            .failing_profiles();
        let op = run_op_in(&all_users, &sys);
        assert_eq!(op.state, OpState::WillApply);
        assert!(op.unverified);
        assert!(op.detail.contains("user profiles"), "{}", op.detail);

        // the account that runs winprune is still readable, so a user probe is known
        assert_eq!(run_op(&sys).state, OpState::AlreadyDone);
    }

    #[test]
    fn run_candidate_that_cannot_be_checked_is_unverified_not_absent() {
        let op = run_op(&Fake::default().failing_path("C:\\Setup.exe"));
        assert_eq!(op.state, OpState::Absent);
        assert!(op.unverified);
        assert!(op.detail.starts_with("could not check"), "{}", op.detail);
        let missing = run_op(&Fake::default());
        assert_eq!(missing.state, OpState::Absent);
        assert!(!missing.unverified);
    }

    const UNSUPPORTED_CATALOG: &str = r#"
[[item]]
id = "onedrive.machine"
name = "Machine"
category = "onedrive"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "unsupported"
reason = "installed for all users"
when_registered = [{ hive = "hklm", path = "SOFTWARE\\Demo", name = "Version" }]
"#;

    #[test]
    fn unsupported_step_reports_what_it_finds_and_apply_skips_it() {
        let present = Fake::default().with_registry(
            &RegRoot::Machine,
            "SOFTWARE\\Demo",
            "Version",
            RegValue::String("1.0".into()),
        );
        let op = run_op_in(UNSUPPORTED_CATALOG, &present);
        assert_eq!(op.state, OpState::NonRemovable);
        assert_eq!(op.kind.to_string(), "installed for all users");

        let catalog = Catalog::parse("t", UNSUPPORTED_CATALOG).unwrap();
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &present,
        );
        let report = crate::engine::apply_plan(
            &plan,
            &mut crate::system::recorder::Recorder::default(),
            true,
            &mut |_| {},
        );
        assert_eq!(
            report.results[0].outcome,
            crate::system::Outcome::Skipped("winprune does not remove it".into())
        );

        assert_eq!(
            run_op_in(UNSUPPORTED_CATALOG, &Fake::default()).state,
            OpState::Absent
        );
        let unread = run_op_in(
            UNSUPPORTED_CATALOG,
            &Fake::default().unreadable(&RegRoot::Machine),
        );
        assert_eq!(unread.state, OpState::Absent);
        assert!(unread.unverified);
    }

    #[test]
    fn default_profile_class_keys_are_left_out_not_reported_unread() {
        let catalog = Catalog::parse(
            "t",
            &SCOPE_CATALOG.replace("Software\\\\Demo", "Software\\\\Classes\\\\Demo"),
        )
        .unwrap();
        let sys = Fake::default()
            .with_profile("S-1-5-21-1", "alice", "C:\\Users\\alice", true)
            .interactive("S-1-5-21-1");
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let roots: Vec<String> = plan.items[0]
            .ops
            .iter()
            .filter_map(|o| match &o.kind {
                OpKind::RegistrySet { root, .. } => Some(root.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(roots, vec!["HKU:alice"]);
        assert!(!plan.items[0].has_unread());
    }

    const PATTERNS_CATALOG: &str = r#"
[[item]]
id = "appx.many"
name = "Many"
category = "appx"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "appx"
patterns = ["*One*", "*Two*", "*Three*"]
"#;

    fn appx_ops(sys: &Fake) -> Vec<Op> {
        let catalog = Catalog::parse("t", PATTERNS_CATALOG).unwrap();
        let plan = build_plan(&catalog, &Selection::level(Level::Medium), 22631, true, sys);
        plan.items[0].ops.clone()
    }

    #[test]
    fn unreadable_package_list_is_one_unread_line_per_step() {
        let ops = appx_ops(&Fake::default().failing_packages());
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert!(ops[0].unverified);
        assert_eq!(ops[0].kind.to_string(), "installed packages");
    }

    #[test]
    fn unreadable_provisioned_list_is_unread_not_an_elevation_matter() {
        let ops = appx_ops(&Fake::default().failing_provisioned());
        let line = ops
            .iter()
            .find(|o| o.kind.to_string() == "provisioned packages")
            .unwrap();
        assert!(line.unverified);
        assert!(line.detail.starts_with("could not list"), "{}", line.detail);
        assert!(ops.iter().all(|o| o.state != OpState::NeedsElevation));
    }

    #[test]
    fn unreadable_task_list_leaves_task_patterns_unverified() {
        let catalog = Catalog::parse("t", TASK_CATALOG).unwrap();
        let sys = Fake::default().failing_tasks().with_process("DemoApp.exe");
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &sys,
        );
        let tasks: Vec<&Op> = plan.items[0]
            .ops
            .iter()
            .filter(|o| matches!(o.kind, OpKind::TaskPattern { .. }))
            .collect();
        assert_eq!(tasks.len(), 2);
        assert!(
            tasks
                .iter()
                .all(|o| o.unverified && o.state == OpState::Absent)
        );
    }

    #[test]
    fn kills_are_attempted_when_processes_cannot_be_listed() {
        let catalog = Catalog::parse("t", TASK_CATALOG).unwrap();
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &Fake::default().failing_processes(),
        );
        let kills: Vec<&Op> = plan.items[0]
            .ops
            .iter()
            .filter(|o| matches!(o.kind, OpKind::Kill { .. }))
            .collect();
        assert_eq!(kills.len(), 2);
        assert!(
            kills
                .iter()
                .all(|o| o.unverified && o.state == OpState::WillApply)
        );
    }

    #[test]
    fn all_users_steps_say_when_other_profiles_could_not_be_listed() {
        let catalog = Catalog::parse("t", SCOPE_CATALOG).unwrap();
        let plan = build_plan(
            &catalog,
            &Selection::level(Level::Medium),
            22631,
            true,
            &Fake::default().failing_profiles(),
        );
        let markers = plan.items[0]
            .ops
            .iter()
            .filter(|o| o.kind.to_string() == "other user profiles")
            .count();
        assert_eq!(markers, 2, "one per all-users step");
        assert!(plan.items[0].has_unread());

        let user_scope =
            Catalog::parse("t", SCOPE_CATALOG.replace("all-users", "user").as_str()).unwrap();
        let plan = build_plan(
            &user_scope,
            &Selection::level(Level::Medium),
            22631,
            true,
            &Fake::default().failing_profiles(),
        );
        assert!(
            plan.items[0]
                .ops
                .iter()
                .all(|o| o.kind.to_string() != "other user profiles")
        );
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
