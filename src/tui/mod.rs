//! Interactive front end. `app` owns the state and key handling, `ui` renders it; the
//! event loop here only wires them to the terminal and the engine.

mod app;
mod ui;

use crate::catalog::Catalog;
use crate::cli::{Context, EXIT_ABORTED, EXIT_FAILURES, EXIT_OK, EXIT_USAGE, Filter};
use crate::engine::{self, Log, OpResult, Report};
use crate::handoff;
use crate::system::recorder::Recorder;
use app::{Action, App, Screen};
use crossterm::event::{self, Event, KeyEventKind};
use std::sync::mpsc;
use std::time::Duration;

enum Progress {
    Op(OpResult),
    Done(Box<Report>),
}

pub fn run(catalog: Catalog, filter: Filter, ctx: Context, at_confirm: bool) -> i32 {
    let sys = ctx.system();
    let selection = filter.selection();
    let plan = engine::build_plan(
        &catalog,
        &selection,
        ctx.info.build,
        ctx.info.elevated,
        &sys,
    );
    let mut app = App::new(plan, selection, ctx.info, filter.catalog.clone());
    if at_confirm {
        app.screen = Screen::Confirm;
    }

    let mut terminal = ratatui::init();
    let code = event_loop(&mut terminal, &mut app, &ctx);
    ratatui::restore();
    if let Some(dir) = &ctx.run_dir {
        if let Some(report) = &app.report {
            handoff::write_report(dir, report);
        }
        // This is the elevated window; let the user read the result before it closes.
        if app.report.is_some() {
            println!(
                "{}",
                app.report.as_ref().map(|r| r.summary()).unwrap_or_default()
            );
        }
        println!("Press Enter to close.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    code
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App, ctx: &Context) -> i32 {
    let mut worker: Option<mpsc::Receiver<Progress>> = None;
    let mut log: Option<Log> = None;
    loop {
        if terminal.draw(|frame| ui::draw(frame, app)).is_err() {
            return EXIT_USAGE;
        }

        if let Some(rx) = &worker {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Progress::Op(r) => {
                        if let Some(l) = &mut log {
                            l.line(&describe(&r));
                        }
                        app.push_result(r);
                    }
                    Progress::Done(report) => {
                        if let Some(l) = &mut log {
                            l.line(&report.summary());
                            app.report_path =
                                l.write_report(&report).map(|p| p.display().to_string());
                            app.log_path = Some(l.path().display().to_string());
                        }
                        app.finish(*report);
                        worker = None;
                        break;
                    }
                }
            }
        }

        match event::poll(Duration::from_millis(100)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => return EXIT_USAGE,
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle(key) {
            Action::None => {}
            Action::Quit => {
                return match &app.report {
                    Some(r) if r.tally.failed > 0 => EXIT_FAILURES,
                    Some(_) => EXIT_OK,
                    None if ctx.run_dir.is_some() => EXIT_ABORTED,
                    None => EXIT_OK,
                };
            }
            Action::RelaunchElevated => {
                ratatui::restore();
                let mut request = app.request();
                request.interactive_sid = crate::system::windows::profiles::current_sid();
                return handoff::run_elevated(&request);
            }
            Action::StartApply => {
                let (tx, rx) = mpsc::channel();
                let plan = app.plan.clone();
                let dry_run = app.dry_run;
                let mut l = Log::open(app.info.elevated);
                l.line(&format!(
                    "apply level {} dry_run {} build {} (tui)",
                    plan.level, dry_run, plan.build
                ));
                log = Some(l);
                let worker_ctx = ctx.clone();
                std::thread::spawn(move || {
                    let mut on_event = |r: &OpResult| {
                        let _ = tx.send(Progress::Op(r.clone()));
                    };
                    let report = if dry_run {
                        let mut rec = Recorder::default();
                        engine::apply_plan(&plan, &mut rec, true, &mut on_event)
                    } else {
                        let mut sys = worker_ctx.system();
                        engine::apply_plan(&plan, &mut sys, false, &mut on_event)
                    };
                    let _ = tx.send(Progress::Done(Box::new(report)));
                });
                worker = Some(rx);
            }
        }
    }
}

fn describe(r: &OpResult) -> String {
    match &r.outcome {
        crate::system::Outcome::Done => format!("ok  {}  {}", r.item, r.op),
        crate::system::Outcome::Skipped(why) => format!("skip  {}  {} ({why})", r.item, r.op),
        crate::system::Outcome::Failed(why) => format!("FAIL  {}  {} ({why})", r.item, r.op),
    }
}

#[cfg(test)]
mod tests {
    use super::app::{App, Screen};
    use super::ui;
    use crate::catalog::{Catalog, Level};
    use crate::engine::{Selection, build_plan};
    use crate::os::OsInfo;
    use crate::system::fake::Fake;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const CATALOG: &str = r#"
[[item]]
id = "appx.demo"
name = "Demo app"
category = "appx"
level = "medium"
risk = "low"
summary = "Removes the demo app."
[[item.step]]
kind = "appx"
patterns = ["*Demo*"]

[[item]]
id = "onedrive.folder"
name = "Delete the folder"
category = "onedrive"
level = "max"
risk = "high"
summary = "Deletes it."
warning = "Everything goes."
[[item.step]]
kind = "delete"
paths = ["%USERPROFILE%\\OneDrive"]
"#;

    fn app(level: Level) -> App {
        let catalog = Catalog::parse("t", CATALOG).unwrap();
        let sys = Fake::default().with_package("Microsoft.DemoApp", false);
        let selection = Selection::level(level);
        let plan = build_plan(&catalog, &selection, 26100, false, &sys);
        App::new(
            plan,
            selection,
            OsInfo {
                build: 26100,
                elevated: false,
            },
            None,
        )
    }

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle(KeyEvent::from(code));
    }

    #[test]
    fn select_screen_lists_categories_and_items() {
        let screen = render(&app(Level::Medium));
        assert!(screen.contains("Preinstalled apps"), "{screen}");
        assert!(screen.contains("[x] Demo app"), "{screen}");
        assert!(screen.contains("[ ] Delete the folder"), "{screen}");
        assert!(
            screen.contains("remove package Microsoft.DemoApp"),
            "{screen}"
        );
    }

    #[test]
    fn space_toggles_item() {
        let mut a = app(Level::Medium);
        press(&mut a, KeyCode::Char(' '));
        assert!(render(&a).contains("[ ] Demo app"));
        press(&mut a, KeyCode::Char(' '));
        assert!(render(&a).contains("[x] Demo app"));
    }

    #[test]
    fn level_key_reselects_and_resets_manual_toggles() {
        let mut a = app(Level::Medium);
        press(&mut a, KeyCode::Char(' '));
        press(&mut a, KeyCode::Char('3'));
        let screen = render(&a);
        assert!(screen.contains("[x] Demo app"), "{screen}");
        assert!(screen.contains("[x] Delete the folder"), "{screen}");
    }

    #[test]
    fn filter_narrows_list() {
        let mut a = app(Level::Max);
        press(&mut a, KeyCode::Char('/'));
        for c in "folder".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        let screen = render(&a);
        assert!(!screen.contains("Demo app"), "{screen}");
        assert!(screen.contains("Delete the folder"), "{screen}");
    }

    #[test]
    fn confirm_requires_yes_for_high_risk() {
        let mut a = app(Level::Max);
        a.dry_run = true;
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.screen, Screen::Confirm);
        assert!(render(&a).contains("Type yes"));
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.screen, Screen::Confirm);
        for c in "yes".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.screen, Screen::Progress);
    }

    #[test]
    fn request_reproduces_selection() {
        let mut a = app(Level::High);
        press(&mut a, KeyCode::Char(' '));
        let request = a.request();
        assert_eq!(request.level, Level::High);
        assert_eq!(request.skip, vec!["appx.demo"]);
        assert!(request.add.is_empty());
        assert!(request.tui);
        assert!(!request.dry_run);
    }
}
