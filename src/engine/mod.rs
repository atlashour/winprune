//! Turns the catalog plus the current system state into a `Plan`, then applies it
//! through the `Apply` trait. Both halves are pure with respect to Windows.

mod apply;
mod expand;
mod log;
mod plan;
mod report;

pub use apply::apply_plan;
pub use expand::expand_env;
pub use log::Log;
pub use plan::{Op, OpKind, OpState, Plan, PlannedItem, Selection, build_plan};
pub use report::{OpResult, Report, Tally};
