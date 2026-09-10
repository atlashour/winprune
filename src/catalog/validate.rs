use super::model::{Hive, Item, Level, Risk, Scope, Step};
use std::collections::HashSet;

/// Roots a `delete` step may point at. Anything outside is a catalog bug, not a policy
/// choice, so it fails at load time.
const DELETABLE_ROOTS: &[&str] = &[
    "%LOCALAPPDATA%\\",
    "%APPDATA%\\",
    "%PROGRAMDATA%\\",
    "%USERPROFILE%\\",
    "C:\\OneDriveTemp",
];

pub fn check(items: &[Item]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();

    let mut seen = HashSet::new();
    for item in items {
        if !seen.insert(item.id.as_str()) {
            errors.push(format!("{}: duplicate id", item.id));
        }
        if item.id.split('.').count() != 2
            || item
                .id
                .chars()
                .any(|c| c.is_whitespace() || c.is_uppercase())
        {
            errors.push(format!(
                "{}: id must look like 'category.name' in lowercase",
                item.id
            ));
        }
        if item.summary.trim().is_empty() {
            errors.push(format!("{}: summary is empty", item.id));
        }
        if item.step.is_empty() {
            errors.push(format!("{}: has no steps", item.id));
        }
        if item.level >= Level::High && item.warning.is_none() {
            errors.push(format!("{}: level {} needs a warning", item.id, item.level));
        }
        for dep in &item.requires {
            if !ids.contains(dep.as_str()) {
                errors.push(format!("{}: requires unknown item '{dep}'", item.id));
            }
        }
        if let (Some(min), Some(max)) = (item.windows.min_build, item.windows.max_build)
            && min > max
        {
            errors.push(format!("{}: min_build is above max_build", item.id));
        }
        for (n, step) in item.step.iter().enumerate() {
            for problem in check_step(step) {
                errors.push(format!("{} step {}: {problem}", item.id, n + 1));
            }
            if matches!(step, Step::Delete { .. })
                && item.risk != Risk::High
                && deletes_user_data(step)
            {
                errors.push(format!("{}: deletes user data, risk must be high", item.id));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_step(step: &Step) -> Vec<String> {
    let mut problems = Vec::new();
    match step {
        Step::Appx { patterns } => {
            if patterns.is_empty() {
                problems.push("appx step without patterns".into());
            }
        }
        Step::Service { names, .. } => {
            if names.is_empty() {
                problems.push("service step without names".into());
            }
        }
        Step::Registry {
            hive,
            path,
            name,
            kind,
            value,
            delete,
            scope,
        } => {
            if path.trim().is_empty() || name.trim().is_empty() {
                problems.push("registry step needs path and name".into());
            }
            if *scope != Scope::User && *hive != Hive::Hkcu {
                problems.push("scope is only meaningful with hive = \"hkcu\"".into());
            }
            match (delete, kind, value) {
                (true, None, None) => {}
                (true, _, _) => {
                    problems.push("registry delete must not carry type or value".into())
                }
                (false, Some(_), Some(_)) => {}
                (false, _, _) => {
                    problems.push("registry set needs type and value, or delete = true".into())
                }
            }
        }
        Step::Task { patterns, .. } => {
            if patterns.is_empty() {
                problems.push("task step without patterns".into());
            }
        }
        Step::Kill { processes } => {
            if processes.is_empty() {
                problems.push("kill step without processes".into());
            }
        }
        Step::Run { candidates, .. } => {
            if candidates.is_empty() {
                problems.push("run step without candidates".into());
            }
        }
        Step::Delete { paths, scope } => {
            if paths.is_empty() {
                problems.push("delete step without paths".into());
            }
            for path in paths {
                let upper = path.to_ascii_uppercase();
                if !DELETABLE_ROOTS
                    .iter()
                    .any(|root| upper.starts_with(&root.to_ascii_uppercase()))
                {
                    problems.push(format!("delete path '{path}' is outside the allowed roots"));
                }
                if *scope != Scope::User && !is_profile_path(&upper) {
                    problems.push(format!(
                        "delete path '{path}' is not inside a user profile, scope must be user"
                    ));
                }
            }
        }
    }
    problems
}

fn is_profile_path(upper: &str) -> bool {
    upper.starts_with("%LOCALAPPDATA%\\")
        || upper.starts_with("%APPDATA%\\")
        || upper.starts_with("%USERPROFILE%\\")
}

fn deletes_user_data(step: &Step) -> bool {
    match step {
        Step::Delete { paths, .. } => paths.iter().any(|p| {
            let upper = p.to_ascii_uppercase();
            upper.starts_with("%USERPROFILE%\\") && !upper.contains("\\APPDATA\\")
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::catalog::Catalog;

    fn item(extra: &str, steps: &str) -> String {
        format!(
            "[[item]]\nid = \"appx.demo\"\nname = \"Demo\"\ncategory = \"appx\"\nlevel = \"medium\"\nrisk = \"low\"\nsummary = \"x\"\n{extra}\n{steps}\n"
        )
    }

    #[test]
    fn rejects_duplicate_ids() {
        let one = item("", "[[item.step]]\nkind = \"appx\"\npatterns = [\"*A*\"]");
        let text = format!("{one}{one}");
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("duplicate id"), "{err}");
    }

    #[test]
    fn rejects_unknown_requires() {
        let text = item(
            "requires = [\"nope.nope\"]",
            "[[item.step]]\nkind = \"appx\"\npatterns = [\"*A*\"]",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("requires unknown item"), "{err}");
    }

    #[test]
    fn rejects_delete_outside_allowed_roots() {
        let text = item(
            "",
            "[[item.step]]\nkind = \"delete\"\npaths = [\"C:\\\\Windows\\\\System32\"]",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("outside the allowed roots"), "{err}");
    }

    #[test]
    fn user_data_delete_must_be_high_risk() {
        let text = item(
            "",
            "[[item.step]]\nkind = \"delete\"\npaths = [\"%USERPROFILE%\\\\OneDrive\"]",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("risk must be high"), "{err}");
    }

    #[test]
    fn high_level_needs_warning() {
        let text = item("", "[[item.step]]\nkind = \"appx\"\npatterns = [\"*A*\"]")
            .replace("\"medium\"", "\"high\"");
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("needs a warning"), "{err}");
    }

    #[test]
    fn scope_requires_hkcu() {
        let text = item(
            "",
            "[[item.step]]\nkind = \"registry\"\nhive = \"hklm\"\nscope = \"all-users\"\npath = \"SOFTWARE\\\\X\"\nname = \"Y\"\ntype = \"dword\"\nvalue = 1",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("only meaningful with hive"), "{err}");
    }

    #[test]
    fn scoped_delete_must_stay_inside_the_profile() {
        let text = item(
            "",
            "[[item.step]]\nkind = \"delete\"\nscope = \"all-users\"\npaths = [\"%PROGRAMDATA%\\\\X\"]",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("scope must be user"), "{err}");
    }

    #[test]
    fn registry_step_needs_value_or_delete() {
        let text = item(
            "",
            "[[item.step]]\nkind = \"registry\"\nhive = \"hklm\"\npath = \"SOFTWARE\\\\X\"\nname = \"Y\"",
        );
        let err = Catalog::parse("t", &text).unwrap_err().to_string();
        assert!(err.contains("needs type and value"), "{err}");
    }
}
