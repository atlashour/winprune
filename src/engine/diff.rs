//! What applying a selection would still change on this machine. Built from the same
//! plan apply walks, so it reads nothing the plan did not read and writes nothing.

use super::plan::{Op, OpState, Plan, PlannedItem, Selection};
use crate::catalog::{Applicability, Catalog, Category, Level, Risk};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Applying would change something.
    Pending,
    /// Nothing pending wherever the state could be read; the rest could not be read.
    Unconfirmed,
    /// Nothing known to be pending, and nothing could be confirmed either.
    Unverified,
    /// Every target is already in the wanted state.
    Applied,
    /// Nothing left to apply, but parts Windows keeps or winprune does not remove remain.
    Blocked,
    /// Nothing the item acts on exists here: removed earlier or never shipped.
    Absent,
    /// The item does not apply to this Windows build.
    NotApplicable,
}

impl ItemStatus {
    pub fn tag(self) -> &'static str {
        match self {
            ItemStatus::Pending => "pending",
            ItemStatus::Unconfirmed => "applied?",
            ItemStatus::Unverified => "unknown",
            ItemStatus::Applied => "applied",
            ItemStatus::Blocked => "blocked",
            ItemStatus::Absent => "absent",
            ItemStatus::NotApplicable => "n/a",
        }
    }
}

impl PlannedItem {
    pub fn status(&self) -> ItemStatus {
        let ops = || self.state_ops();
        if ops().any(Op::pending) {
            ItemStatus::Pending
        } else if ops().any(Op::unread) {
            if ops().any(|op| !op.unread() && op.state != OpState::WillApply) {
                ItemStatus::Unconfirmed
            } else {
                ItemStatus::Unverified
            }
        } else if ops().any(|op| op.state == OpState::NonRemovable) {
            ItemStatus::Blocked
        } else if ops().any(|op| op.state == OpState::AlreadyDone) {
            ItemStatus::Applied
        } else {
            ItemStatus::Absent
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffItem {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub level: Level,
    pub risk: Risk,
    pub status: ItemStatus,
    /// Operations apply would run, with what the plan saw.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    /// Operations whose current state could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DiffCounts {
    pub pending: usize,
    pub unconfirmed: usize,
    pub unverified: usize,
    pub applied: usize,
    pub blocked: usize,
    pub absent: usize,
    pub not_applicable: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub schema: u32,
    pub version: &'static str,
    pub build: u32,
    pub elevated: bool,
    pub level: Level,
    pub user: Option<String>,
    pub counts: DiffCounts,
    pub items: Vec<DiffItem>,
}

/// `plan` must have been built from `catalog` and `selection`; the catalog is only
/// consulted for the selected items the plan left out because of the build.
pub fn build_diff(catalog: &Catalog, selection: &Selection, plan: &Plan) -> Diff {
    let mut items: Vec<DiffItem> = plan.selected().map(diff_item).collect();
    items.extend(
        catalog
            .items
            .iter()
            .filter(|i| !i.windows.matches(plan.build) && selection.wants(&i.id, i.level))
            .map(|i| DiffItem {
                id: i.id.clone(),
                name: i.name.clone(),
                category: i.category,
                level: i.level,
                risk: i.risk,
                status: ItemStatus::NotApplicable,
                pending: Vec::new(),
                unverified: Vec::new(),
                reason: Some(describe_builds(&i.windows)),
            }),
    );
    let order: HashMap<&str, usize> = catalog
        .items
        .iter()
        .enumerate()
        .map(|(n, i)| (i.id.as_str(), n))
        .collect();
    items.sort_by_key(|i| (i.category, order.get(i.id.as_str()).copied()));

    let mut counts = DiffCounts::default();
    for item in &items {
        *match item.status {
            ItemStatus::Pending => &mut counts.pending,
            ItemStatus::Unconfirmed => &mut counts.unconfirmed,
            ItemStatus::Unverified => &mut counts.unverified,
            ItemStatus::Applied => &mut counts.applied,
            ItemStatus::Blocked => &mut counts.blocked,
            ItemStatus::Absent => &mut counts.absent,
            ItemStatus::NotApplicable => &mut counts.not_applicable,
        } += 1;
    }

    Diff {
        schema: 1,
        version: env!("CARGO_PKG_VERSION"),
        build: plan.build,
        elevated: plan.elevated,
        level: plan.level,
        user: plan.user.clone(),
        counts,
        items,
    }
}

fn diff_item(item: &PlannedItem) -> DiffItem {
    let line = |op: &Op| {
        if op.detail.is_empty() {
            op.kind.to_string()
        } else {
            format!("{} ({})", op.kind, op.detail)
        }
    };
    DiffItem {
        id: item.id.clone(),
        name: item.name.clone(),
        category: item.category,
        level: item.level,
        risk: item.risk,
        status: item.status(),
        pending: item
            .state_ops()
            .filter(|op| op.pending())
            .map(line)
            .collect(),
        unverified: item
            .state_ops()
            .filter(|op| op.unread())
            .map(line)
            .collect(),
        reason: None,
    }
}

fn describe_builds(w: &Applicability) -> String {
    match (w.min_build, w.max_build) {
        (Some(min), Some(max)) => format!("needs build {min} to {max}"),
        (Some(min), None) => format!("needs build {min} or later"),
        (None, Some(max)) => format!("needs build {max} or earlier"),
        (None, None) => "not for this build".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Startup;
    use crate::engine::build_plan;
    use crate::system::fake::Fake;
    use crate::system::{Provisioned, RegRoot, RegValue};

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
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "service"
names = ["DemoSvc"]
startup = "disabled"

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
[[item.step]]
kind = "kill"
processes = ["Demo"]

[[item]]
id = "privacy.new"
name = "New"
category = "privacy"
level = "medium"
risk = "low"
summary = "x"
windows = { min_build = 26200 }
[[item.step]]
kind = "registry"
hive = "hklm"
path = "SOFTWARE\\Demo"
name = "New"
type = "dword"
value = 1

[[item]]
id = "privacy.later"
name = "Later"
category = "privacy"
level = "max"
risk = "low"
summary = "x"
warning = "w"
windows = { min_build = 26200 }
[[item.step]]
kind = "registry"
hive = "hklm"
path = "SOFTWARE\\Demo"
name = "Later"
type = "dword"
value = 1
"#;

    fn diff(sys: &Fake, level: Level) -> Diff {
        let catalog = Catalog::parse("t", CATALOG).unwrap();
        let selection = Selection::level(level);
        let plan = build_plan(&catalog, &selection, 26100, true, sys);
        build_diff(&catalog, &selection, &plan)
    }

    fn status(d: &Diff, id: &str) -> ItemStatus {
        d.items.iter().find(|i| i.id == id).unwrap().status
    }

    #[test]
    fn untouched_machine_reports_everything_pending() {
        let sys = Fake::default()
            .with_package("Microsoft.DemoApp", false)
            .with_service("DemoSvc", Startup::Automatic, true);
        let d = diff(&sys, Level::Medium);
        assert_eq!(status(&d, "appx.demo"), ItemStatus::Pending);
        assert_eq!(status(&d, "services.demo"), ItemStatus::Pending);
        assert_eq!(status(&d, "privacy.reg"), ItemStatus::Pending);
        let svc = d.items.iter().find(|i| i.id == "services.demo").unwrap();
        assert_eq!(
            svc.pending,
            vec![
                "stop service DemoSvc",
                "service DemoSvc -> disabled (currently automatic)"
            ]
        );
    }

    #[test]
    fn applied_machine_reports_applied_and_absent() {
        let sys = Fake::default()
            .with_service("DemoSvc", Startup::Disabled, false)
            .with_registry(
                &RegRoot::Machine,
                "SOFTWARE\\Demo",
                "Flag",
                RegValue::Dword(1),
            );
        let d = diff(&sys, Level::Medium);
        assert_eq!(status(&d, "appx.demo"), ItemStatus::Absent);
        assert_eq!(status(&d, "services.demo"), ItemStatus::Applied);
        assert_eq!(status(&d, "privacy.reg"), ItemStatus::Applied);
        assert_eq!(d.counts.pending, 0);
        assert_eq!(d.counts.applied, 2);
    }

    #[test]
    fn running_process_alone_does_not_make_an_item_pending() {
        let sys = Fake::default()
            .with_registry(
                &RegRoot::Machine,
                "SOFTWARE\\Demo",
                "Flag",
                RegValue::Dword(1),
            )
            .with_process("Demo.exe");
        assert_eq!(
            status(&diff(&sys, Level::Medium), "privacy.reg"),
            ItemStatus::Applied
        );
    }

    #[test]
    fn readable_part_applied_and_the_rest_unreadable_is_unconfirmed() {
        let sys = Fake::default()
            .with_service("DemoSvc", Startup::Disabled, false)
            .provisioned(Provisioned::NeedsElevation);
        let d = diff(&sys, Level::Medium);
        assert_eq!(status(&d, "appx.demo"), ItemStatus::Unconfirmed);
        assert_eq!(status(&d, "services.demo"), ItemStatus::Applied);
        assert_eq!(d.counts.unconfirmed, 1);
    }

    #[test]
    fn unreadable_state_is_unverified_not_pending() {
        let sys = Fake::default().unreadable(&RegRoot::Machine);
        let d = diff(&sys, Level::Medium);
        assert_eq!(status(&d, "privacy.reg"), ItemStatus::Unverified);
        let item = d.items.iter().find(|i| i.id == "privacy.reg").unwrap();
        assert!(item.pending.is_empty());
        assert_eq!(item.unverified.len(), 1);
    }

    #[test]
    fn installed_package_is_pending_even_when_provisioned_are_unknown() {
        let installed = Fake::default()
            .provisioned(Provisioned::NeedsElevation)
            .with_package("Microsoft.DemoApp", false);
        assert_eq!(
            status(&diff(&installed, Level::Medium), "appx.demo"),
            ItemStatus::Pending
        );
    }

    #[test]
    fn an_unreadable_package_list_is_not_an_empty_one() {
        let sys = Fake::default().failing_packages();
        assert_eq!(
            status(&diff(&sys, Level::Medium), "appx.demo"),
            ItemStatus::Unverified
        );
    }

    #[test]
    fn non_removable_leftovers_are_blocked() {
        let sys = Fake::default().with_package("Microsoft.DemoCore", true);
        assert_eq!(
            status(&diff(&sys, Level::Medium), "appx.demo"),
            ItemStatus::Blocked
        );
    }

    const MIXED_CATALOG: &str = r#"
[[item]]
id = "appx.mixed"
name = "Mixed"
category = "appx"
level = "medium"
risk = "low"
summary = "x"
[[item.step]]
kind = "appx"
patterns = ["*DemoCore*"]
[[item.step]]
kind = "service"
names = ["DemoSvc"]
startup = "disabled"
"#;

    fn mixed_status(sys: &Fake) -> ItemStatus {
        let catalog = Catalog::parse("t", MIXED_CATALOG).unwrap();
        let selection = Selection::level(Level::Medium);
        let plan = build_plan(&catalog, &selection, 26100, true, sys);
        build_diff(&catalog, &selection, &plan).items[0].status
    }

    #[test]
    fn applied_parts_next_to_a_part_windows_keeps_are_blocked_not_applied() {
        let sys = Fake::default()
            .with_package("Microsoft.DemoCore", true)
            .with_service("DemoSvc", Startup::Disabled, false);
        assert_eq!(mixed_status(&sys), ItemStatus::Blocked);
    }

    #[test]
    fn a_readable_blocked_part_with_the_rest_unreadable_is_unconfirmed() {
        let sys = Fake::default()
            .with_package("Microsoft.DemoCore", true)
            .provisioned(Provisioned::NeedsElevation);
        assert_eq!(
            status(&diff(&sys, Level::Medium), "appx.demo"),
            ItemStatus::Unconfirmed
        );
    }

    const EARLY_ITEM: &str = r#"
[[item]]
id = "privacy.first"
name = "First"
category = "privacy"
level = "medium"
risk = "low"
summary = "x"
windows = { min_build = 26200 }
[[item.step]]
kind = "registry"
hive = "hklm"
path = "SOFTWARE\\Demo"
name = "First"
type = "dword"
value = 1
"#;

    #[test]
    fn items_for_another_build_keep_their_catalog_place() {
        let catalog = Catalog::parse("t", &format!("{EARLY_ITEM}{CATALOG}")).unwrap();
        let selection = Selection::level(Level::Medium);
        let plan = build_plan(&catalog, &selection, 26100, true, &Fake::default());
        let d = build_diff(&catalog, &selection, &plan);
        let privacy: Vec<&str> = d
            .items
            .iter()
            .filter(|i| i.id.starts_with("privacy."))
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(privacy, vec!["privacy.first", "privacy.reg", "privacy.new"]);
    }

    #[test]
    fn items_for_another_build_are_listed_only_when_the_level_wants_them() {
        let d = diff(&Fake::default(), Level::Medium);
        let new = d.items.iter().find(|i| i.id == "privacy.new").unwrap();
        assert_eq!(new.status, ItemStatus::NotApplicable);
        assert_eq!(new.reason.as_deref(), Some("needs build 26200 or later"));
        assert!(d.items.iter().all(|i| i.id != "privacy.later"));
        assert_eq!(d.counts.not_applicable, 1);
    }

    #[test]
    fn diff_follows_skip_and_level() {
        let catalog = Catalog::parse("t", CATALOG).unwrap();
        let mut selection = Selection::level(Level::Medium);
        selection.skip.insert("privacy.reg".into());
        let plan = build_plan(&catalog, &selection, 26100, true, &Fake::default());
        let d = build_diff(&catalog, &selection, &plan);
        assert!(d.items.iter().all(|i| i.id != "privacy.reg"));
        assert_eq!(d.level, Level::Medium);
    }

    #[test]
    fn json_carries_schema_and_statuses() {
        let json = serde_json::to_string(&diff(&Fake::default(), Level::Medium)).unwrap();
        assert!(json.contains("\"schema\":1"), "{json}");
        assert!(json.contains("\"status\":\"not_applicable\""), "{json}");
        assert!(json.contains("\"status\":\"pending\""), "{json}");
    }
}
