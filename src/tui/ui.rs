use super::app::{App, Pane, Row, Screen};
use crate::catalog::{Level, Risk};
use crate::engine::{OpState, PlannedItem};
use crate::system::Outcome;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap};

const ACCENT: Color = Color::Cyan;
const WARN: Color = Color::Yellow;
const DANGER: Color = Color::Red;
const DIM: Color = Color::DarkGray;

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_header(frame, header, app);
    match app.screen {
        Screen::Select => draw_select(frame, body, app),
        Screen::Confirm => {
            draw_select(frame, body, app);
            draw_confirm(frame, body, app);
        }
        Screen::Progress => draw_progress(frame, body, app),
        Screen::Result => draw_result(frame, body, app),
    }
    draw_footer(frame, footer, app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let elevated = if app.info.elevated {
        Span::styled("elevated", Style::new().fg(Color::Green))
    } else {
        Span::styled("not elevated", Style::new().fg(WARN))
    };
    let mode = if app.dry_run {
        Span::styled(" dry run ", Style::new().fg(Color::Black).bg(WARN))
    } else {
        Span::raw("")
    };
    let line = Line::from(vec![
        Span::styled(
            " winprune ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw(format!(" build {} ", app.info.build)),
        Span::styled("| ", Style::new().fg(DIM)),
        elevated,
        Span::styled(" | level ", Style::new().fg(DIM)),
        Span::styled(app.plan.level.to_string(), Style::new().fg(ACCENT).bold()),
        Span::styled(
            format!(
                " | {} items, {} operations ",
                app.plan.selected().count(),
                app.plan.will_apply_count()
            ),
            Style::new().fg(DIM),
        ),
        mode,
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let text = match app.screen {
        Screen::Select if app.editing_filter => format!(" filter: {}_   (Enter keep, Esc clear)", app.filter),
        Screen::Select => {
            " space toggle  a/n all/none  1/2/3 level  h/l fold  / filter  d dry run  tab details  enter continue  q quit"
                .to_string()
        }
        Screen::Confirm => " enter apply  esc back".to_string(),
        Screen::Progress => " applying, please wait".to_string(),
        Screen::Result => " q quit".to_string(),
    };
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(DIM)), area);
}

fn draw_select(frame: &mut Frame, area: Rect, app: &App) {
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    draw_list(frame, list_area, app);
    draw_details(frame, detail_area, app);
}

fn level_tag(level: Level) -> Span<'static> {
    let (text, color) = match level {
        Level::Medium => ("medium", Color::Green),
        Level::High => ("high", WARN),
        Level::Max => ("max", DANGER),
    };
    Span::styled(format!("{text:<6}"), Style::new().fg(color))
}

fn risk_tag(risk: Risk) -> Span<'static> {
    let (text, color) = match risk {
        Risk::Low => ("low", DIM),
        Risk::Medium => ("medium", WARN),
        Risk::High => ("HIGH", DANGER),
    };
    Span::styled(format!("risk {text:<6}"), Style::new().fg(color))
}

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|row| match row {
            Row::Category(c) => {
                let (selected, total) = app.category_counts(*c);
                let arrow = if app.collapsed.contains(c) { ">" } else { "v" };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {arrow} {}", c.title()),
                        Style::new().fg(ACCENT).bold(),
                    ),
                    Span::styled(format!("  {selected}/{total}"), Style::new().fg(DIM)),
                ]))
            }
            Row::Item(i) => item_line(&app.plan.items[*i], width),
        })
        .collect();
    let title = if app.filter.is_empty() {
        " Items ".to_string()
    } else {
        format!(" Items (filter: {}) ", app.filter)
    };
    let border = if app.pane == Pane::List { ACCENT } else { DIM };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::new().fg(border)),
        )
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn item_line(item: &PlannedItem, width: usize) -> ListItem<'static> {
    let mark = if item.selected { "[x]" } else { "[ ]" };
    let tags_width = 6 + 1 + 11;
    let name_width = width.saturating_sub(4 + 1 + tags_width + 3);
    let mut name = item.name.clone();
    if name.chars().count() > name_width {
        name = name
            .chars()
            .take(name_width.saturating_sub(1))
            .collect::<String>()
            + "~";
    }
    let name_style = if item.selected {
        Style::new()
    } else {
        Style::new().fg(DIM)
    };
    ListItem::new(Line::from(vec![
        Span::raw(format!("   {mark} ")),
        Span::styled(format!("{name:<name_width$} "), name_style),
        level_tag(item.level),
        Span::raw(" "),
        risk_tag(item.risk),
    ]))
}

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let border = if app.pane == Pane::Details {
        ACCENT
    } else {
        DIM
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Details ")
        .border_style(Style::new().fg(border));
    let Some(index) = app.current_item() else {
        let text = match app.rows().get(app.cursor) {
            Some(Row::Category(c)) => {
                let (selected, total) = app.category_counts(*c);
                format!(
                    "{}\n\n{selected} of {total} items selected.\n\nspace toggles the whole category.",
                    c.title()
                )
            }
            _ => "No items match the filter.".to_string(),
        };
        frame.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    };
    let item = &app.plan.items[index];
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(item.name.clone(), Style::new().bold())),
        Line::from(vec![
            Span::styled(item.id.clone(), Style::new().fg(DIM)),
            Span::raw("  "),
            level_tag(item.level),
            Span::raw(" "),
            risk_tag(item.risk),
        ]),
        Line::raw(""),
        Line::raw(item.summary.clone()),
    ];
    if let Some(w) = &item.warning {
        let color = if item.risk == Risk::High {
            DANGER
        } else {
            WARN
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("Warning: {w}"),
            Style::new().fg(color),
        )));
    }
    if !item.requires.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("Requires: {}", item.requires.join(", ")),
            Style::new().fg(DIM),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "On this machine",
        Style::new().fg(ACCENT),
    )));
    for op in &item.ops {
        let (tag, color) = match op.state {
            OpState::WillApply => ("apply ", Color::Green),
            OpState::Absent => ("absent", DIM),
            OpState::AlreadyDone => ("done  ", DIM),
            OpState::NonRemovable => ("keep  ", WARN),
            OpState::NeedsElevation => ("elev  ", WARN),
        };
        let mut spans = vec![
            Span::styled(format!(" {tag} "), Style::new().fg(color)),
            Span::raw(op.kind.to_string()),
        ];
        if !op.detail.is_empty() {
            spans.push(Span::styled(
                format!("  {}", op.detail),
                Style::new().fg(DIM),
            ));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered(area, 80, 80);
    frame.render_widget(Clear, popup);
    let selected: Vec<&PlannedItem> = app.plan.selected().collect();
    let high_risk = app.high_risk_selected();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "{} items, {} operations{}",
            selected.len(),
            app.plan.will_apply_count(),
            if app.dry_run { " (dry run)" } else { "" }
        ),
        Style::new().bold(),
    )));
    lines.push(Line::raw(""));
    for item in &selected {
        let mut spans = vec![Span::raw(format!("  {}  ", item.name)), risk_tag(item.risk)];
        if item.will_apply() == 0 {
            spans.push(Span::styled("  nothing to do", Style::new().fg(DIM)));
        }
        lines.push(Line::from(spans));
        if let Some(w) = &item.warning
            && item.risk != Risk::Low
        {
            let color = if item.risk == Risk::High {
                DANGER
            } else {
                WARN
            };
            lines.push(Line::from(Span::styled(
                format!("      {w}"),
                Style::new().fg(color),
            )));
        }
    }
    lines.push(Line::raw(""));
    if app.needs_elevation_to_apply() {
        lines.push(Line::from(Span::styled(
            "Applying needs administrator rights: winprune will ask through UAC and continue in a new window.",
            Style::new().fg(WARN),
        )));
        lines.push(Line::raw(""));
    }
    if high_risk {
        lines.push(Line::from(vec![
            Span::styled(
                "High-risk items selected. Type yes and press Enter: ",
                Style::new().fg(DANGER),
            ),
            Span::styled(format!("{}_", app.confirm_input), Style::new().bold()),
        ]));
    } else {
        lines.push(Line::from(Span::raw(
            "Press Enter to apply, Esc to go back.",
        )));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm ")
        .border_style(Style::new().fg(if high_risk { DANGER } else { ACCENT }));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_progress(frame: &mut Frame, area: Rect, app: &App) {
    let [gauge_area, log_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
    let done = app.progress.len();
    let total = app.progress_total.max(1);
    let ratio = (done as f64 / total as f64).min(1.0);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" Progress "))
            .gauge_style(Style::new().fg(ACCENT))
            .ratio(ratio)
            .label(format!("{done} / {}", app.progress_total)),
        gauge_area,
    );
    let visible = log_area.height.saturating_sub(2) as usize;
    let start = app.progress.len().saturating_sub(visible);
    let lines: Vec<Line> = app.progress[start..].iter().map(result_line).collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Log ")),
        log_area,
    );
}

fn result_line(r: &crate::engine::OpResult) -> Line<'static> {
    let (tag, color, note) = match &r.outcome {
        Outcome::Done => ("  ok ", Color::Green, String::new()),
        Outcome::Skipped(why) => (" skip", WARN, format!("  ({why})")),
        Outcome::Failed(why) => (" FAIL", DANGER, format!("  ({why})")),
    };
    Line::from(vec![
        Span::styled(tag.to_string(), Style::new().fg(color)),
        Span::styled(format!("  {}  ", r.item), Style::new().fg(DIM)),
        Span::raw(r.op.clone()),
        Span::styled(note, Style::new().fg(DIM)),
    ])
}

fn draw_result(frame: &mut Frame, area: Rect, app: &App) {
    let Some(report) = &app.report else {
        return;
    };
    let t = report.tally;
    let mut lines = vec![
        Line::from(Span::styled(
            if report.dry_run {
                "Dry run finished"
            } else {
                "Finished"
            },
            Style::new().bold(),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled(format!("{:>5}", t.done), Style::new().fg(Color::Green)),
            Span::raw("  done"),
        ]),
        Line::from(vec![
            Span::styled(format!("{:>5}", t.skipped), Style::new().fg(WARN)),
            Span::raw("  skipped (protected or not removable)"),
        ]),
        Line::from(vec![
            Span::styled(format!("{:>5}", t.absent), Style::new().fg(DIM)),
            Span::raw("  absent or already done"),
        ]),
        Line::from(vec![
            Span::styled(format!("{:>5}", t.failed), Style::new().fg(DANGER)),
            Span::raw("  failed"),
        ]),
        Line::raw(""),
    ];
    if let Some(p) = &app.report_path {
        lines.push(Line::from(vec![
            Span::styled("report  ", Style::new().fg(DIM)),
            Span::raw(p.clone()),
        ]));
    }
    if let Some(p) = &app.log_path {
        lines.push(Line::from(vec![
            Span::styled("log     ", Style::new().fg(DIM)),
            Span::raw(p.clone()),
        ]));
    }
    let failed: Vec<Line> = report
        .results
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Failed(_)))
        .map(result_line)
        .collect();
    if !failed.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Failures",
            Style::new().fg(DANGER).bold(),
        )));
        lines.extend(failed);
    }
    if !report.dry_run && t.done > 0 {
        lines.push(Line::raw(""));
        lines.push(Line::raw(
            "A restart is recommended before judging the result.",
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}
