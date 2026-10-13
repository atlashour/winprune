//! The catalog is the only place where Windows package, service, registry and task
//! names live. Everything else in the crate treats them as opaque strings.

mod load;
mod model;
mod validate;

pub use load::Catalog;
pub use model::{
    Applicability, Category, Hive, Item, Level, RegProbe, RegType, Risk, Scope, Startup, Step,
    TaskAction,
};

/// Variables a catalog path can start with to point inside a user profile, and where
/// each one sits under the profile folder. Steps that reach every account expand them
/// once per profile.
pub const PROFILE_VARS: [(&str, &str); 3] = [
    ("%USERPROFILE%", ""),
    ("%LOCALAPPDATA%", "\\AppData\\Local"),
    ("%APPDATA%", "\\AppData\\Roaming"),
];

/// `raw` starts with one of `PROFILE_VARS`, compared without case.
pub fn profile_var(raw: &str) -> Option<(&'static str, &'static str)> {
    let upper = raw.to_ascii_uppercase();
    PROFILE_VARS
        .iter()
        .find(|(var, _)| upper.starts_with(var))
        .copied()
}
