use crate::catalog::{Catalog, Level};
use crate::engine::{self, Log, OpState, Plan, Selection};
use crate::handoff::{self, Request};
use crate::os;
use crate::system::Outcome;
use crate::system::recorder::Recorder;
use clap::{Args, Parser, Subcommand};
use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_FAILURES: i32 = 2;
pub const EXIT_ELEVATION: i32 = 3;
pub const EXIT_ABORTED: i32 = 4;

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
    /// Folder written by the launcher for the elevated copy; see handoff.rs
    #[arg(long, value_name = "DIR", hide = true, global = true)]
    pub run_dir: Option<PathBuf>,
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

    pub fn request(&self, dry_run: bool, tui: bool) -> Request {
        Request {
            level: self.level,
            only: self.only.clone(),
            skip: self.skip.clone(),
            add: self.add.clone(),
            catalog: self
                .catalog
                .as_ref()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())),
            dry_run,
            interactive_sid: None,
            tui,
        }
    }

    fn from_request(request: &Request, run_dir: PathBuf) -> Filter {
        Filter {
            level: request.level,
            catalog: request.catalog.clone(),
            only: request.only.clone(),
            skip: request.skip.clone(),
            add: request.add.clone(),
            run_dir: Some(run_dir),
        }
    }
}

/// Everything a run needs to know about where it is running.
#[derive(Clone)]
pub struct Context {
    pub info: os::OsInfo,
    pub interactive_sid: Option<String>,
    pub run_dir: Option<PathBuf>,
}

impl Context {
    pub fn system(&self) -> crate::system::windows::WindowsSystem {
        crate::system::windows::WindowsSystem::new(self.info.elevated, self.interactive_sid.clone())
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
    let info = os::detect();

    // The elevated copy takes its whole configuration from the request file.
    if let Some(dir) = cli.filter.run_dir.clone() {
        return match handoff::read_request(&dir) {
            Ok(request) => run_request(request, dir, info),
            Err(e) => {
                eprintln!("{e}");
                EXIT_USAGE
            }
        };
    }

    let catalog = match load_catalog(cli.filter.catalog.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return EXIT_USAGE;
        }
    };
    let ctx = Context {
        info,
        interactive_sid: None,
        run_dir: None,
    };

    match cli.command {
        None => crate::tui::run(catalog, cli.filter, ctx, false),
        Some(Command::Catalog { what }) => catalog_command(&catalog, what),
        Some(Command::Plan { json, all }) => {
            let plan = build(&catalog, &cli.filter, &ctx);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).expect("plan serialises")
                );
            } else {
                print!("{}", render_plan(&plan, all));
            }
            let mut log = Log::open(ctx.info.elevated);
            let saved = log.write_plan(&plan);
            log.line(&format!(
                "plan level {} build {} elevated {}: {} items selected, {} operations{}",
                plan.level,
                plan.build,
                plan.elevated,
                plan.selected().count(),
                plan.will_apply_count(),
                saved
                    .as_ref()
                    .map(|p| format!(", saved to {}", p.display()))
                    .unwrap_or_default()
            ));
            if !json && let Some(p) = saved {
                println!("plan saved to {}", p.display());
            }
            EXIT_OK
        }
        Some(Command::Apply { dry_run, yes }) => {
            apply_command(&catalog, &cli.filter, &ctx, dry_run, yes)
        }
    }
}

fn run_request(request: Request, dir: PathBuf, info: os::OsInfo) -> i32 {
    let filter = Filter::from_request(&request, dir.clone());
    let catalog = match load_catalog(filter.catalog.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return EXIT_USAGE;
        }
    };
    let ctx = Context {
        info,
        interactive_sid: request.interactive_sid.clone(),
        run_dir: Some(dir),
    };
    if request.tui {
        crate::tui::run(catalog, filter, ctx, true)
    } else {
        apply_command(&catalog, &filter, &ctx, request.dry_run, true)
    }
}

fn build(catalog: &Catalog, filter: &Filter, ctx: &Context) -> Plan {
    let sys = ctx.system();
    engine::build_plan(
        catalog,
        &filter.selection(),
        ctx.info.build,
        ctx.info.elevated,
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
    if let Some(user) = &plan.user {
        let _ = writeln!(
            out,
            "per-user settings are written for {user} and the Default profile"
        );
    }
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
    ctx: &Context,
    dry_run: bool,
    yes: bool,
) -> i32 {
    if !dry_run && !ctx.info.elevated {
        if !io::stdin().is_terminal() {
            eprintln!(
                "apply needs administrator rights; the UAC prompt cannot be answered from a script. Run it from an elevated console."
            );
            return EXIT_ELEVATION;
        }
        if !yes && !confirm("This will change the system. Type yes to continue: ") {
            return EXIT_ABORTED;
        }
        let mut request = filter.request(false, false);
        request.interactive_sid = crate::system::windows::profiles::current_sid();
        return handoff::run_elevated(&request);
    }

    let plan = build(catalog, filter, ctx);
    print!("{}", render_plan(&plan, false));
    if !dry_run && !yes && !confirm("Type yes to apply: ") {
        return EXIT_ABORTED;
    }

    let mut log = Log::open(ctx.info.elevated);
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
        let mut sys = ctx.system();
        engine::apply_plan(&plan, &mut sys, false, &mut on_event)
    };
    log.line(&report.summary());
    println!("\n{}", report.summary());
    match log.write_report(&report) {
        Some(path) => println!("report: {}", path.display()),
        None => println!("report could not be written under {}", log.dir().display()),
    }
    println!("log: {}", log.path().display());
    if let Some(dir) = &ctx.run_dir {
        handoff::write_report(dir, &report);
        // The UAC relaunch opens its own console; keep it until the user has read this.
        pause("Press Enter to close.");
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

fn pause(prompt: &str) {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().read_line(&mut answer);
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

    #[test]
    fn request_carries_the_whole_filter() {
        let cli = Cli::try_parse_from([
            "winprune", "apply", "--level", "high", "--skip", "a.b", "--add", "c.d", "--only",
            "e.f",
        ])
        .unwrap();
        let request = cli.filter.request(true, false);
        assert_eq!(request.level, Level::High);
        assert_eq!(request.skip, vec!["a.b"]);
        assert_eq!(request.add, vec!["c.d"]);
        assert_eq!(request.only, vec!["e.f"]);
        assert!(request.dry_run);
        let back = Filter::from_request(&request, PathBuf::from("x"));
        assert_eq!(back.only, vec!["e.f"]);
    }
}
