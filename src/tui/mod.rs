use crate::catalog::Catalog;
use crate::cli::Filter;
use crate::os::OsInfo;

pub fn run(_catalog: Catalog, _filter: Filter, _info: OsInfo) -> i32 {
    eprintln!("the interactive mode is not available yet; use `winprune plan` or `winprune apply`");
    crate::cli::EXIT_USAGE
}
