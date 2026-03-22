use crate::model::*;
use crate::tui::app::{App, Dialog, Screen};
use crate::util::{format_age, username_from_uid};
use bytesize::ByteSize;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

const CYAN: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const MARK_COLOR: Color = Color::Magenta;

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Scanning => draw_scanning(frame, app),
        Screen::Results => draw_results(frame, app),
    }
    if app.show_help {
        draw_help_overlay(frame);
    }
    match &app.dialog {
        Dialog::ConfirmStage => draw_confirm_dialog(frame, app),
        Dialog::StageResult(msg) => draw_result_dialog(frame, msg),
        Dialog::None => {}
    }
}

// ── Scanning screen ─────────────────────────────────────────────────────────

fn draw_scanning(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled(" DISKCLEAN ", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled("— Full Semantic Disk Analyzer", Style::default().fg(Color::White)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(CYAN)));
    frame.render_widget(title, chunks[0]);

    let phase_text = match &app.progress.phase {
        ScanPhase::DetectingApps => "Detecting installed applications...".to_string(),
        ScanPhase::ScanningUser(u) => format!("Scanning /Users/{}...", u),
        ScanPhase::ScanningSystem => "Scanning system directories...".to_string(),
        ScanPhase::Complete => "Scan complete!".to_string(),
    };

    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spin_char = spinner[app.spinner_tick % spinner.len()];

    let bar_width = (chunks[1].width as usize).saturating_sub(4);
    let fill = (app.spinner_tick * 3) % bar_width.max(1);
    let bar: String = (0..bar_width)
        .map(|i| {
            let d = ((i as isize) - (fill as isize)).unsigned_abs();
            if d < 4 { '█' } else { '░' }
        })
        .collect();

    let progress = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!(" {} ", spin_char), Style::default().fg(CYAN)),
            Span::styled(phase_text, Style::default().fg(Color::White)),
        ]),
        Line::from(Span::styled(format!(" [{}]", bar), Style::default().fg(CYAN))),
    ]);
    frame.render_widget(progress, chunks[1]);

    let stats = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} files scanned", app.progress.files_scanned),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        Span::styled(
            format!("{} categories found", app.live_categories.len()),
            Style::default().fg(Color::White),
        ),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        Span::styled(
            format!("{:.1}s elapsed", app.progress.elapsed.as_secs_f64()),
            Style::default().fg(DIM),
        ),
    ]));
    frame.render_widget(stats, chunks[2]);

    let mut live_cats: Vec<(&Category, u64)> = app.live_categories.iter()
        .map(|(cat, findings)| (cat, findings.iter().map(|f| f.physical_size).sum()))
        .collect();
    live_cats.sort_by(|a, b| b.1.cmp(&a.1));

    let items: Vec<ListItem> = live_cats.iter().map(|(cat, size)| {
        ListItem::new(Line::from(vec![
            Span::styled(format!("  {:>10}", ByteSize(*size)), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(cat.label(), Style::default().fg(Color::White)),
        ]))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .title(" Categories found so far ")
            .title_style(Style::default().fg(CYAN))
            .borders(Borders::TOP)
            .border_style(Style::default().fg(DIM)));
    frame.render_widget(list, chunks[3]);

    let hint = Paragraph::new(Span::styled(" Press q to quit", Style::default().fg(DIM)));
    frame.render_widget(hint, chunks[4]);
}

// ── Results screen ──────────────────────────────────────────────────────────

fn draw_results(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, main_chunks[0], app);

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_chunks[1]);

    draw_category_list(frame, panels[0], app);
    draw_detail_panel(frame, panels[1], app);
    draw_summary_bar(frame, main_chunks[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let elapsed = app.scan_stats.as_ref().map(|s| s.elapsed.as_secs_f64()).unwrap_or(0.0);
    let files = app.scan_stats.as_ref().map(|s| s.files_scanned).unwrap_or(0);
    let dataless = app.scan_stats.as_ref().map(|s| s.dataless_skipped).unwrap_or(0);

    let mut spans = vec![
        Span::styled(" DISKCLEAN ", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled("│ ", Style::default().fg(DIM)),
        Span::styled(format!("{}", ByteSize(app.grand_total)), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" reclaimable", Style::default().fg(Color::White)),
        Span::styled(" │ ", Style::default().fg(DIM)),
        Span::styled(format!("{} files in {:.1}s", files, elapsed), Style::default().fg(DIM)),
        Span::styled(" │ ", Style::default().fg(DIM)),
        Span::styled(format!("{} iCloud-evicted", dataless), Style::default().fg(DIM)),
    ];

    if app.staged_count > 0 {
        spans.push(Span::styled(" │ ", Style::default().fg(DIM)));
        spans.push(Span::styled(
            format!("{} staged ({})", app.staged_count, ByteSize(app.staged_size)),
            Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(vec![Line::from(spans)])
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(CYAN)));
    frame.render_widget(header, area);
}

fn draw_category_list(frame: &mut Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = Vec::new();
    let mut current_row: usize = 0;

    for (i, cat_row) in app.categories.iter().enumerate() {
        if cat_row.total_size == 0 && cat_row.category != Category::TimeMachineLocal {
            continue;
        }

        let is_expanded = app.expanded.contains(&i);
        let is_selected = current_row == app.selected;
        let is_marked = app.is_marked(i, None);
        let arrow = if is_expanded { "▾" } else { "▸" };

        let risk = cat_row.category.risk_level();
        let risk_style = risk.style();

        let mark_indicator = if is_marked { "✗ " } else { "  " };

        let line = Line::from(vec![
            Span::styled(
                if is_selected { "│" } else { " " },
                Style::default().fg(if is_selected { CYAN } else { DIM }),
            ),
            Span::styled(mark_indicator, Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", arrow), Style::default().fg(Color::White)),
            Span::styled(
                format!("{:>10}", ByteSize(cat_row.total_size)),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(cat_row.category.label(), Style::default().fg(Color::White)),
            Span::raw(" "),
            Span::styled(format!("[{}]", risk.short()), risk_style),
        ]);

        let style = if is_selected {
            Style::default().bg(Color::Rgb(30, 40, 55))
        } else {
            Style::default()
        };

        items.push(ListItem::new(line).style(style));
        current_row += 1;

        if is_expanded {
            let show = cat_row.findings.len().min(20);
            for fi in 0..show {
                let f = &cat_row.findings[fi];
                let is_finding_selected = current_row == app.selected;
                let is_finding_marked = app.is_marked(i, Some(fi));
                let user = username_from_uid(f.owner_uid);
                let age = format_age(f.last_modified);

                let path_str = f.path.to_string_lossy();
                let max_path = (area.width as usize).saturating_sub(35);
                let display_path = if path_str.len() > max_path && max_path > 2 {
                    format!("…{}", &path_str[path_str.len() - max_path + 1..])
                } else {
                    path_str.to_string()
                };

                let mark_ind = if is_finding_marked { "✗ " } else { "  " };

                let mut spans = vec![
                    Span::styled(
                        if is_finding_selected { "│" } else { " " },
                        Style::default().fg(if is_finding_selected { CYAN } else { DIM }),
                    ),
                    Span::styled(mark_ind, Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD)),
                    Span::styled("  ", Style::default().fg(DIM)),
                    Span::styled(
                        format!("{:>10}", ByteSize(f.physical_size)),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" "),
                    Span::styled(format!("[{}]", user), Style::default().fg(Color::Blue)),
                    Span::raw(" "),
                    Span::styled(display_path, Style::default().fg(Color::White)),
                    Span::styled(format!(" ({})", age), Style::default().fg(DIM)),
                ];

                if f.cloud_backed {
                    spans.push(Span::styled(" ☁", Style::default().fg(Color::Blue)));
                }

                let style = if is_finding_selected {
                    Style::default().bg(Color::Rgb(30, 40, 55))
                } else {
                    Style::default()
                };

                items.push(ListItem::new(Line::from(spans)).style(style));
                current_row += 1;
            }
            if cat_row.findings.len() > show {
                let hidden_size: u64 = cat_row.findings[show..].iter().map(|f| f.physical_size).sum();
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("       ... {} more ({})", cat_row.findings.len() - show, ByteSize(hidden_size)),
                    Style::default().fg(DIM),
                ))));
                current_row += 1;
            }
        }
    }

    let block = Block::default()
        .title(" Categories ")
        .title_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::new(0, 0, 0, 0));

    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = if app.selected >= visible_height {
        app.selected - visible_height + 1
    } else {
        0
    };

    let total_items = items.len();
    let display_items: Vec<ListItem> = items.into_iter().skip(scroll_offset).collect();

    let list = List::new(display_items).block(block);
    frame.render_widget(list, area);

    if total_items > visible_height {
        let mut scrollbar_state = ScrollbarState::new(total_items).position(scroll_offset);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area,
            &mut scrollbar_state,
        );
    }
}

fn draw_detail_panel(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Details ")
        .title_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::new(1, 1, 1, 0));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(cat_row) = app.selected_category() else {
        let empty = Paragraph::new(Span::styled("No category selected", Style::default().fg(DIM)));
        frame.render_widget(empty, inner);
        return;
    };

    let cat = &cat_row.category;
    let risk = cat.risk_level();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        cat.label(),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Size: ", Style::default().fg(DIM)),
        Span::styled(
            format!("{}", ByteSize(cat_row.total_size)),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({} items)", cat_row.findings.len()),
            Style::default().fg(DIM),
        ),
    ]));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Risk: ", Style::default().fg(DIM)),
        Span::styled(risk.label(), risk.style()),
    ]));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("Advice:", Style::default().fg(CYAN))));
    let advice = cat.advice();
    let wrap_width = inner.width.saturating_sub(2) as usize;
    for wrapped_line in textwrap(advice, wrap_width) {
        lines.push(Line::from(Span::styled(wrapped_line, Style::default().fg(Color::White))));
    }
    lines.push(Line::from(""));

    if let Some(cmd) = cat.quick_command() {
        lines.push(Line::from(Span::styled("Quick command:", Style::default().fg(CYAN))));
        lines.push(Line::from(Span::styled(
            format!("$ {}", cmd),
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(""));
    }

    // Selected finding detail
    if let Some((_cat_row, finding)) = app.selected_finding() {
        lines.push(Line::from(Span::styled("─── Selected Item ───", Style::default().fg(CYAN))));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Path: ", Style::default().fg(DIM)),
            Span::styled(finding.path.to_string_lossy().to_string(), Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Size: ", Style::default().fg(DIM)),
            Span::styled(format!("{} on disk", ByteSize(finding.physical_size)), Style::default().fg(Color::Yellow)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Age: ", Style::default().fg(DIM)),
            Span::styled(format_age(finding.last_modified), Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Owner: ", Style::default().fg(DIM)),
            Span::styled(username_from_uid(finding.owner_uid), Style::default().fg(Color::Blue)),
        ]));
        if finding.cloud_backed {
            lines.push(Line::from(Span::styled("☁ Backed up to cloud", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))));
        }
        if !finding.detail.is_empty() {
            lines.push(Line::from(Span::styled(&finding.detail, Style::default().fg(DIM))));
        }
    }

    // Staging info
    if !app.marked.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("─── Staging ───", Style::default().fg(MARK_COLOR))));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} items marked ({})", app.marked.len(), ByteSize(app.marked_size)),
                Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("Press D to move to {}", app.staging.path.display()),
            Style::default().fg(DIM),
        )));
    }

    let detail = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(detail, inner);
}

fn draw_summary_bar(frame: &mut Frame, area: Rect, app: &App) {
    let errors = app.scan_stats.as_ref().map(|s| s.perm_errors).unwrap_or(0);

    let mut spans = vec![
        Span::styled(" Safe: ", Style::default().fg(DIM)),
        Span::styled(format!("{}", ByteSize(app.safe_total)), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::styled(" │ Total: ", Style::default().fg(DIM)),
        Span::styled(format!("{}", ByteSize(app.grand_total)), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ];

    if !app.marked.is_empty() {
        spans.push(Span::styled(" │ Marked: ", Style::default().fg(DIM)));
        spans.push(Span::styled(
            format!("{} ({})", app.marked.len(), ByteSize(app.marked_size)),
            Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD),
        ));
    }

    if app.staged_count > 0 {
        spans.push(Span::styled(" │ Staged: ", Style::default().fg(DIM)));
        spans.push(Span::styled(
            format!("{}", ByteSize(app.staged_size)),
            Style::default().fg(MARK_COLOR),
        ));
    }

    if errors > 0 {
        spans.push(Span::styled(format!(" │ ⚠ {} errs", errors), Style::default().fg(Color::Yellow)));
    }

    let line1 = Line::from(spans);
    let line2 = Line::from(vec![
        Span::styled(" ↑↓", Style::default().fg(CYAN)),
        Span::styled(" nav  ", Style::default().fg(DIM)),
        Span::styled("⏎", Style::default().fg(CYAN)),
        Span::styled(" expand  ", Style::default().fg(DIM)),
        Span::styled("d", Style::default().fg(MARK_COLOR)),
        Span::styled(" mark  ", Style::default().fg(DIM)),
        Span::styled("D", Style::default().fg(MARK_COLOR)),
        Span::styled(" move to ~/To Delete  ", Style::default().fg(DIM)),
        Span::styled("?", Style::default().fg(CYAN)),
        Span::styled(" help  ", Style::default().fg(DIM)),
        Span::styled("q", Style::default().fg(CYAN)),
        Span::styled(" quit", Style::default().fg(DIM)),
    ]);

    let summary = Paragraph::new(vec![line1, line2])
        .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(CYAN)));
    frame.render_widget(summary, area);
}

// ── Dialogs ─────────────────────────────────────────────────────────────────

fn draw_confirm_dialog(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let w = 60.min(area.width.saturating_sub(4));
    let h = 9.min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Move items to ~/To Delete ?",
            Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Items: ", Style::default().fg(DIM)),
            Span::styled(format!("{}", app.marked.len()), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("    Size: ", Style::default().fg(DIM)),
            Span::styled(format!("{}", ByteSize(app.marked_size)), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(" Destination: {}", app.staging.path.display()),
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled("[Y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(" Yes, move them    ", Style::default().fg(Color::White)),
            Span::styled("[N]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled(" Cancel", Style::default().fg(Color::White)),
        ]),
    ];

    let dialog = Paragraph::new(lines)
        .block(Block::default()
            .title(" Confirm ")
            .title_style(Style::default().fg(MARK_COLOR))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MARK_COLOR)));
    frame.render_widget(dialog, popup);
}

fn draw_result_dialog(frame: &mut Frame, msg: &str) {
    let area = frame.area();
    let w = 60.min(area.width.saturating_sub(4));
    let h = 7.min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let wrap_width = w.saturating_sub(4) as usize;
    let mut lines = vec![Line::from("")];
    for line in textwrap(msg, wrap_width) {
        lines.push(Line::from(Span::styled(format!(" {}", line), Style::default().fg(Color::White))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Press any key to continue", Style::default().fg(DIM))));

    let is_error = msg.contains("error");
    let border_color = if is_error { Color::Red } else { Color::Green };

    let dialog = Paragraph::new(lines)
        .block(Block::default()
            .title(if is_error { " Error " } else { " Done " })
            .title_style(Style::default().fg(border_color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)));
    frame.render_widget(dialog, popup);
}

// ── Help overlay ────────────────────────────────────────────────────────────

fn draw_help_overlay(frame: &mut Frame) {
    let area = frame.area();
    let w = 55.min(area.width.saturating_sub(4));
    let h = 20.min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled(" Keyboard Shortcuts", Style::default().fg(CYAN).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![Span::styled(" ↑/k    ", Style::default().fg(Color::White)), Span::styled("Move up", Style::default().fg(DIM))]),
        Line::from(vec![Span::styled(" ↓/j    ", Style::default().fg(Color::White)), Span::styled("Move down", Style::default().fg(DIM))]),
        Line::from(vec![Span::styled(" Enter  ", Style::default().fg(Color::White)), Span::styled("Expand / collapse category", Style::default().fg(DIM))]),
        Line::from(vec![Span::styled(" g / G  ", Style::default().fg(Color::White)), Span::styled("Jump to top / bottom", Style::default().fg(DIM))]),
        Line::from(""),
        Line::from(Span::styled(" Deletion", Style::default().fg(MARK_COLOR).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![Span::styled(" d      ", Style::default().fg(MARK_COLOR)), Span::styled("Mark / unmark item or category", Style::default().fg(DIM))]),
        Line::from(vec![Span::styled(" D / x  ", Style::default().fg(MARK_COLOR)), Span::styled("Move marked items to ~/To Delete", Style::default().fg(DIM))]),
        Line::from(""),
        Line::from(Span::styled(" Marked items are MOVED (not deleted).", Style::default().fg(DIM))),
        Line::from(Span::styled(" Review ~/To Delete then rm -rf it.", Style::default().fg(DIM))),
        Line::from(""),
        Line::from(vec![Span::styled(" ?      ", Style::default().fg(Color::White)), Span::styled("Toggle this help", Style::default().fg(DIM))]),
        Line::from(vec![Span::styled(" q/Esc  ", Style::default().fg(Color::White)), Span::styled("Quit", Style::default().fg(DIM))]),
    ];

    let help = Paragraph::new(help_text)
        .block(Block::default()
            .title(" Help ")
            .title_style(Style::default().fg(CYAN))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(CYAN)));
    frame.render_widget(help, popup);
}

fn textwrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![s.to_string()]; }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        if current.len() + word.len() + 1 > width && !current.is_empty() {
            lines.push(current.clone());
            current.clear();
        }
        if !current.is_empty() { current.push(' '); }
        current.push_str(word);
    }
    if !current.is_empty() { lines.push(current); }
    lines
}
