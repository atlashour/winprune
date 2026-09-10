//! The catalog is the only place where Windows package, service, registry and task
//! names live. Everything else in the crate treats them as opaque strings.

mod load;
mod model;
mod validate;

pub use load::Catalog;
pub use model::{Category, Hive, Item, Level, RegType, Risk, Scope, Startup, Step, TaskAction};
