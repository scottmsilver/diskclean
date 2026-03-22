pub mod app;
pub mod ui;

use app::{App, Dialog};
use crate::model::ScanEvent;
use crate::scanner;
use crossbeam_channel::{tick, select};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;

pub fn run_tui() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    let (_handle, scan_rx) = scanner::spawn_scan();
    let tick_rx = tick(Duration::from_millis(80));

    let mut app = App::new();

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        select! {
            recv(tick_rx) -> _ => {
                app.tick();
                while event::poll(Duration::ZERO)? {
                    if let Event::Key(key) = event::read()? {
                        handle_key(&mut app, key);
                    }
                }
            }
            recv(scan_rx) -> msg => {
                match msg {
                    Ok(ScanEvent::Progress(p)) => app.on_progress(p),
                    Ok(ScanEvent::Found(cat, finding)) => app.on_found(cat, finding),
                    Ok(ScanEvent::Complete(result)) => app.on_complete(result),
                    Err(_) => {
                        if matches!(app.screen, app::Screen::Scanning) {
                            app.on_complete(crate::model::ScanResult {
                                categories: Vec::new(),
                                grand_total: 0,
                                safe_total: 0,
                                cloud_total: 0,
                                files_scanned: app.progress.files_scanned,
                                perm_errors: app.progress.perm_errors,
                                dataless_skipped: app.progress.dataless_skipped,
                                elapsed: app.progress.elapsed,
                            });
                        }
                    }
                }
            }
        }

        if app.should_quit { break; }
    }

    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Print summary after exit
    if app.staged_count > 0 {
        println!(
            "\nMoved {} items ({}) to {:?}",
            app.staged_count,
            bytesize::ByteSize(app.staged_size),
            app.staging.path,
        );
        println!("Review the folder, then delete it when ready:");
        println!("  rm -rf {:?}", app.staging.path);
    }

    Ok(())
}

fn handle_key(app: &mut App, key: event::KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    // Dialog handling
    match &app.dialog {
        Dialog::ConfirmStage => {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.execute_stage();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    app.dialog = Dialog::None;
                }
                _ => {}
            }
            return;
        }
        Dialog::StageResult(_) => {
            // Any key dismisses the result dialog
            app.dialog = Dialog::None;
            return;
        }
        Dialog::None => {}
    }

    // Help overlay
    if app.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => app.show_help = false,
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Enter | KeyCode::Char(' ') => app.toggle_expand(),
        KeyCode::Char('g') | KeyCode::Home => app.home(),
        KeyCode::Char('G') | KeyCode::End => app.end(),
        // Mark for deletion
        KeyCode::Char('d') => app.toggle_mark(),
        // Execute move to staging
        KeyCode::Char('D') | KeyCode::Char('x') => app.request_stage(),
        _ => {}
    }
}
