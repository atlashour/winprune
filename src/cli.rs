use crate::catalog::{Catalog, Level};
use crate::engine::{self, Log, OpState, Plan, Selection};
use crate::os;
use crate::system::Outcome;
use crate::system::recorder::Recorder;
use clap::{Args, Parser, Subcommand};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_FAILURES: i32 = 2;
pub const EXIT_ELEVATION: i32 = 3;

#[derive(Parser, Debug)]
#[command(name = "winprune", version, about = "Windows 10/11 debloater", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub filter: Filter,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show what would change without touching anything
    Plan {
        /// Print the plan as JSON
        #[arg(long)]
        json: bool,
        /// Show inspection details for unselected items too
        #[arg(long)]
        all: bool,
    },
    /// Apply the selected items (asks for elevation)
    Apply {
        /// Walk the whole apply path but change nothing
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation
        #[arg(long, short = 'y')]
        yes: bool,
        /// Set by the relaunched elevated process
        #[arg(long, hide = true)]
        elevated: bool,
    },
    /// Inspect the item catalog
    Catalog {
        #[command(subcommand)]
        what: CatalogCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum CatalogCommand {
    /// List every item with its level and risk
    List,
    /// Check the embedded catalog plus any overlay
    Validate,
}

#[derive(Args, Debug, Clone)]
pub struct Filter {
    /// Preset level: medium, high or max
    #[arg(long, short = 'l', default_value = "medium", global = true)]
    pub level: Level,
    /// TOML overlay that adds or overrides catalog items
    #[arg(long, value_name = "FILE", global = true)]
    pub catalog: Option<PathBuf>,
    /// Only these item ids (comma separated), ignoring the level
    #[arg(long, value_delimiter = ',', global = true)]
    pub only: Vec<String>,
    /// Leave these item ids out (comma separated)
    #[arg(long, value_delimiter = ',', global = true)]
    pub skip: Vec<String>,
    /// Add these item ids on top of the level (comma separated)
    #[arg(long, value_delimiter = ',', global = true)]
    pub add: Vec<String>,
    /// Set when the TUI relaunched itself elevated; opens straight at the confirmation
    #[arg(long, hide = true, global = true)]
    pub relaunched: bool,
}

impl Filter {
    pub fn selection(&self) -> Selection {
        Selection {
            level: self.level,
            only: if self.only.is_empty() {
                None
            } else {
                Some(self.only.iter().cloned().collect())
            },
            skip: self.skip.iter().cloned().collect::<HashSet<_>>(),
            extra: self.add.iter().cloned().collect::<HashSet<_>>(),
        }
    }
}

pub fn load_catalog(overlay: Option<&PathBuf>) -> Result<Catalog, String> {
    let catalog = Catalog::embedded().map_err(|e| e.to_string())?;
    match overlay {
        None => Ok(catalog),
        Some(path) => {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            catalog
                .with_overlay(&path.display().to_string(), &text)
                .map_err(|e| e.to_string())
        }
    }
}

pub fn run(cli: Cli) -> i32 {
    let catalog = match load_catalog(cli.filter.catalog.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return EXIT_USAGE;
        }
    };
    let info = os::detect();

    match cli.command {
        None => crate::tui::run(catalog, cli.filter, info),
        Some(Command::Catalog { what }) => catalog_command(&catalog, what),
        Some(Command::Plan { json, all }) => {
            let plan = build(&catalog, &cli.filter, info);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).expect("plan serialises")
                );
            } else {
                print!("{}", render_plan(&plan, all));
            }
            EXIT_OK
        }
        Some(Command::Apply {
            dry_run,
            yes,
            elevated,
        }) => apply_command(&catalog, &cli.filter, info, dry_run, yes, elevated),
    }
}

fn build(catalog: &Catalog, filter: &Filter, info: os::OsInfo) -> Plan {
    let sys = crate::system::windows::WindowsSystem::new();
    engine::build_plan(
        catalog,
        &filter.selection(),
        info.build,
        info.elevated,
        &sys,
    )
}

fn catalog_command(catalog: &Catalog, what: CatalogCommand) -> i32 {
    match what {
        CatalogCommand::Validate => {
            println!("{} items, catalog is valid", catalog.items.len());
            EXIT_OK
        }
        CatalogCommand::List => {
            println!("{:<34} {:<7} {:<7} name", "id", "level", "risk");
            for item in &catalog.items {
                println!(
                    "{:<34} {:<7} {:<7} {}",
                    item.id, item.level, item.risk, item.name
                );
            }
            EXIT_OK
        }
    }
}

pub fn render_plan(plan: &Plan, all: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Windows build {}, {}, level {} ({})",
        plan.build,
        if plan.elevated {
            "elevated"
        } else {
            "not elevated"
        },
        plan.level,
        plan.level.describe()
    );
    let mut category = None;
    for item in &plan.items {
        if category != Some(item.category) {
            category = Some(item.category);
            let _ = writeln!(out, "\n{}", item.category.title());
        }
        let mark = if item.selected { "[x]" } else { "[ ]" };
        let _ = writeln!(
            out,
            "  {mark} {:<32} {:<6} risk {}",
            item.id, item.level, item.risk
        );
        if !(item.selected || all) {
            continue;
        }
        for op in &item.ops {
            let tag = match op.state {
                OpState::WillApply => "apply",
                OpState::Absent => "absent",
                OpState::AlreadyDone => "done",
                OpState::NonRemovable => "keep",
                OpState::NeedsElevation => "elev",
            };
            let detail = if op.detail.is_empty() {
                String::new()
            } else {
                format!("  ({})", op.detail)
            };
            let _ = writeln!(out, "        {tag:<7} {}{detail}", op.kind);
        }
        if let Some(w) = &item.warning
            && item.selected
        {
            let _ = writeln!(out, "        warning: {w}");
        }
    }
    let selected = plan.selected().count();
    let elevation = plan.selected().filter(|i| i.needs_elevation()).count();
    let _ = writeln!(
        out,
        "\n{selected} items selected, {} operations to apply",
        plan.will_apply_count()
    );
    if elevation > 0 {
        let _ = writeln!(
            out,
            "{elevation} items could not be fully inspected without elevation"
        );
    }
    out
}

fn apply_command(
    catalog: &Catalog,
    filter: &Filter,
    info: os::OsInfo,
    dry_run: bool,
    yes: bool,
    elevated: bool,
) -> i32 {
    if !dry_run && !info.elevated {
        if !yes && !confirm("This will change the system. Type yes to continue: ") {
            return EXIT_USAGE;
        }
        let mut args: Vec<String> = std::env::args().skip(1).collect();
        if !args.iter().any(|a| a == "--yes" || a == "-y") {
            args.push("--yes".into());
        }
        args.push("--elevated".into());
        return match os::relaunch_elevated(&args) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                EXIT_ELEVATION
            }
        };
    }

    let plan = build(catalog, filter, info);
    print!("{}", render_plan(&plan, false));
    if !dry_run && !yes && !confirm("Type yes to apply: ") {
        return EXIT_USAGE;
    }

    let mut log = Log::open(info.elevated);
    log.line(&format!(
        "apply level {} dry_run {} build {}",
        plan.level, dry_run, plan.build
    ));
    println!();
    let mut on_event = |r: &engine::OpResult| {
        let line = match &r.outcome {
            Outcome::Done => format!("   ok  {}  {}", r.item, r.op),
            Outcome::Skipped(why) => format!(" skip  {}  {} ({why})", r.item, r.op),
            Outcome::Failed(why) => format!(" FAIL  {}  {} ({why})", r.item, r.op),
        };
        println!("{line}");
        log.line(line.trim_start());
    };
    let report = if dry_run {
        let mut rec = Recorder::default();
        engine::apply_plan(&plan, &mut rec, true, &mut on_event)
    } else {
        let mut sys = crate::system::windows::WindowsSystem::new();
        engine::apply_plan(&plan, &mut sys, false, &mut on_event)
    };
    log.line(&report.summary());
    println!("\n{}", report.summary());
    match log.write_report(&report) {
        Some(path) => println!("report: {}", path.display()),
        None => println!("report could not be written under {}", log.dir().display()),
    }
    println!("log: {}", log.path().display());

    if elevated {
        // The UAC relaunch opens its own console; keep it until the user has read this.
        confirm("Press Enter to close.");
    }
    if report.tally.failed > 0 {
        EXIT_FAILURES
    } else {
        EXIT_OK
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    answer.trim().eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_level_and_lists() {
        let cli = Cli::try_parse_from(["winprune", "plan", "--level", "max", "--skip", "a.b,c.d"])
            .unwrap();
        assert_eq!(cli.filter.level, Level::Max);
        assert_eq!(cli.filter.skip, vec!["a.b", "c.d"]);
        assert!(matches!(cli.command, Some(Command::Plan { .. })));
    }

    #[test]
    fn rejects_unknown_level() {
        assert!(Cli::try_parse_from(["winprune", "plan", "--level", "extreme"]).is_err());
    }

    #[test]
    fn no_subcommand_means_tui() {
        let cli = Cli::try_parse_from(["winprune", "--level", "high"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.filter.level, Level::High);
    }
}
