use crate::catalog::{Category, Level, Risk};
use crate::engine::{OpResult, Plan, Report, Selection};
use crate::os::OsInfo;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Select,
    Confirm,
    Progress,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Details,
}

/// What the event loop must do after a key was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    StartApply,
    RelaunchElevated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Category(Category),
    Item(usize),
}

pub struct App {
    pub plan: Plan,
    pub selection: Selection,
    pub info: OsInfo,
    pub screen: Screen,
    pub pane: Pane,
    pub cursor: usize,
    pub collapsed: HashSet<Category>,
    pub filter: String,
    pub editing_filter: bool,
    pub detail_scroll: u16,
    pub dry_run: bool,
    pub confirm_input: String,
    pub progress: Vec<OpResult>,
    pub progress_total: usize,
    pub report: Option<Report>,
    pub report_path: Option<String>,
    pub log_path: Option<String>,
    rows: Vec<Row>,
}

impl App {
    pub fn new(plan: Plan, selection: Selection, info: OsInfo) -> App {
        let mut app = App {
            plan,
            selection,
            info,
            screen: Screen::Select,
            pane: Pane::List,
            cursor: 0,
            collapsed: HashSet::new(),
            filter: String::new(),
            editing_filter: false,
            detail_scroll: 0,
            dry_run: false,
            confirm_input: String::new(),
            progress: Vec::new(),
            progress_total: 0,
            report: None,
            report_path: None,
            log_path: None,
            rows: Vec::new(),
        };
        app.rebuild_rows();
        app.move_to_first_item();
        app
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn current_item(&self) -> Option<usize> {
        match self.rows.get(self.cursor) {
            Some(Row::Item(i)) => Some(*i),
            _ => None,
        }
    }

    pub fn category_counts(&self, category: Category) -> (usize, usize) {
        let items = self.plan.items.iter().filter(|i| i.category == category);
        let total = items.clone().count();
        let selected = items.filter(|i| i.selected).count();
        (selected, total)
    }

    pub fn high_risk_selected(&self) -> bool {
        self.plan.selected().any(|i| i.risk == Risk::High)
    }

    pub fn needs_elevation_to_apply(&self) -> bool {
        !self.dry_run && !self.info.elevated
    }

    fn rebuild_rows(&mut self) {
        let filter = self.filter.to_ascii_lowercase();
        let mut rows = Vec::new();
        for category in Category::ALL {
            let members: Vec<usize> = self
                .plan
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.category == category)
                .filter(|(_, item)| {
                    filter.is_empty()
                        || item.id.to_ascii_lowercase().contains(&filter)
                        || item.name.to_ascii_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect();
            if members.is_empty() {
                continue;
            }
            rows.push(Row::Category(category));
            if !self.collapsed.contains(&category) {
                rows.extend(members.into_iter().map(Row::Item));
            }
        }
        self.rows = rows;
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
    }

    fn move_to_first_item(&mut self) {
        if let Some(pos) = self.rows.iter().position(|r| matches!(r, Row::Item(_))) {
            self.cursor = pos;
        }
    }

    fn reselect(&mut self) {
        self.plan.reselect(&self.selection);
    }

    fn toggle_item(&mut self, index: usize) {
        let id = self.plan.items[index].id.clone();
        if self.plan.items[index].selected {
            self.selection.extra.remove(&id);
            self.selection.skip.insert(id);
        } else {
            self.selection.skip.remove(&id);
            self.selection.extra.insert(id);
        }
        self.reselect();
    }

    fn set_category(&mut self, category: Category, on: bool) {
        let ids: Vec<String> = self
            .plan
            .items
            .iter()
            .filter(|i| i.category == category)
            .map(|i| i.id.clone())
            .collect();
        for id in ids {
            if on {
                self.selection.skip.remove(&id);
                self.selection.extra.insert(id);
            } else {
                self.selection.extra.remove(&id);
                self.selection.skip.insert(id);
            }
        }
        self.reselect();
    }

    pub fn set_level(&mut self, level: Level) {
        self.selection = Selection::level(level);
        self.plan.level = level;
        self.reselect();
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        match self.screen {
            Screen::Select => self.handle_select(key),
            Screen::Confirm => self.handle_confirm(key),
            Screen::Progress => Action::None,
            Screen::Result => match key.code {
                KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc => Action::Quit,
                _ => Action::None,
            },
        }
    }

    fn handle_select(&mut self, key: KeyEvent) -> Action {
        if self.editing_filter {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.editing_filter = false;
                }
                KeyCode::Enter => self.editing_filter = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.rebuild_rows();
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.rows.len().saturating_sub(1),
            KeyCode::Char(' ') => match self.rows.get(self.cursor).cloned() {
                Some(Row::Item(i)) => self.toggle_item(i),
                Some(Row::Category(c)) => {
                    let (selected, total) = self.category_counts(c);
                    self.set_category(c, selected < total);
                }
                None => {}
            },
            KeyCode::Left | KeyCode::Char('h') => self.fold(true),
            KeyCode::Right | KeyCode::Char('l') => self.fold(false),
            KeyCode::Char('a') => {
                if let Some(c) = self.current_category() {
                    self.set_category(c, true);
                }
            }
            KeyCode::Char('n') => {
                if let Some(c) = self.current_category() {
                    self.set_category(c, false);
                }
            }
            KeyCode::Char('1') => self.set_level(Level::Medium),
            KeyCode::Char('2') => self.set_level(Level::High),
            KeyCode::Char('3') => self.set_level(Level::Max),
            KeyCode::Char('d') => self.dry_run = !self.dry_run,
            KeyCode::Char('/') => self.editing_filter = true,
            KeyCode::Tab => {
                self.pane = match self.pane {
                    Pane::List => Pane::Details,
                    Pane::Details => Pane::List,
                }
            }
            KeyCode::Enter if self.plan.selected().next().is_some() => {
                self.confirm_input.clear();
                self.screen = Screen::Confirm;
            }
            _ => {}
        }
        Action::None
    }

    fn handle_confirm(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Select;
                Action::None
            }
            KeyCode::Backspace => {
                self.confirm_input.pop();
                Action::None
            }
            KeyCode::Char(c) => {
                self.confirm_input.push(c);
                Action::None
            }
            KeyCode::Enter => {
                if self.high_risk_selected()
                    && !self.confirm_input.trim().eq_ignore_ascii_case("yes")
                {
                    return Action::None;
                }
                if self.needs_elevation_to_apply() {
                    return Action::RelaunchElevated;
                }
                self.begin_progress();
                Action::StartApply
            }
            _ => Action::None,
        }
    }

    pub fn begin_progress(&mut self) {
        self.progress.clear();
        self.progress_total = self.plan.will_apply_count();
        self.screen = Screen::Progress;
    }

    pub fn push_result(&mut self, result: OpResult) {
        self.progress.push(result);
    }

    pub fn finish(&mut self, report: Report) {
        self.report = Some(report);
        self.screen = Screen::Result;
    }

    fn current_category(&self) -> Option<Category> {
        match self.rows.get(self.cursor)? {
            Row::Category(c) => Some(*c),
            Row::Item(i) => Some(self.plan.items[*i].category),
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.pane == Pane::Details {
            self.detail_scroll = (self.detail_scroll as i32 + delta).max(0) as u16;
            return;
        }
        let len = self.rows.len() as i32;
        if len == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, len - 1) as usize;
        self.detail_scroll = 0;
    }

    fn fold(&mut self, collapse: bool) {
        let Some(category) = self.current_category() else {
            return;
        };
        if collapse {
            self.collapsed.insert(category);
        } else {
            self.collapsed.remove(&category);
        }
        self.rebuild_rows();
        if let Some(pos) = self.rows.iter().position(|r| *r == Row::Category(category)) {
            self.cursor = pos;
        }
    }

    /// Arguments that reproduce the current selection in a relaunched process.
    pub fn relaunch_args(&self) -> Vec<String> {
        let mut args = vec!["--level".to_string(), self.selection.level.to_string()];
        let mut skip: Vec<&String> = self.selection.skip.iter().collect();
        let mut add: Vec<&String> = self.selection.extra.iter().collect();
        skip.sort();
        add.sort();
        if !skip.is_empty() {
            args.push("--skip".into());
            args.push(
                skip.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if !add.is_empty() {
            args.push("--add".into());
            args.push(add.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","));
        }
        args.push("--relaunched".into());
        args
    }
}
