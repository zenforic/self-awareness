use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::io;
use std::time::Duration;

use crate::config::{self, Config, ImageFormat};
use crate::daemon;
use crate::startup;
use crate::cleanup;

/// Action returned by the TUI after the user exits.
#[derive(Debug, PartialEq)]
pub enum TuiAction {
    Quit,
    StartDaemon,
    StopDaemon,
    Save,
}

/// Run the TUI for configuring the self-awareness monitor.
pub fn run_tui(config: &mut Config) -> Result<TuiAction> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run TUI loop
    let result = run_ui(&mut terminal, config);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_ui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &mut Config,
) -> Result<TuiAction> {
    let mut focused_field: usize = 0;
    let mut editing = false;
    let mut edit_buffer = String::new();

    loop {
        terminal.draw(|frame| ui(frame, config, focused_field, editing, &edit_buffer))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            return Ok(TuiAction::Quit);
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            config.save()?;
                            return Ok(TuiAction::Save);
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            // Start daemon
                            config.save()?;
                            return Ok(TuiAction::StartDaemon);
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            // Stop daemon
                            config.save()?;
                            return Ok(TuiAction::StopDaemon);
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            // Toggle startup
                            config.start_on_boot = !config.start_on_boot;
                        }
                        KeyCode::Esc => {
                            if editing {
                                editing = false;
                            } else {
                                return Ok(TuiAction::Quit);
                            }
                        }
                        KeyCode::Tab => {
                            if editing {
                                editing = false;
                            } else {
                                focused_field = (focused_field + 1) % 5;
                            }
                        }
                        KeyCode::BackTab => {
                            if editing {
                                editing = false;
                            } else {
                                focused_field = if focused_field == 0 { 4 } else { focused_field - 1 };
                            }
                        }
                        KeyCode::Enter => {
                            if editing {
                                apply_edit(config, focused_field, &edit_buffer);
                                editing = false;
                            } else {
                                editing = true;
                                edit_buffer = match focused_field {
                                    0 => config.interval_seconds.to_string(),
                                    1 => config.max_disk_mb.to_string(),
                                    2 => config.output_dir.clone(),
                                    3 => config.image_format.label().to_string(),
                                    4 => config.retention_days.to_string(),
                                    _ => String::new(),
                                };
                            }
                        }
                        _ => {
                            if editing {
                                match key.code {
                                    KeyCode::Char(c) => edit_buffer.push(c),
                                    KeyCode::Backspace => {
                                        edit_buffer.pop();
                                    }
                                    KeyCode::Enter => {
                                        apply_edit(config, focused_field, &edit_buffer);
                                        editing = false;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn ui(
    frame: &mut ratatui::Frame,
    config: &Config,
    focused_field: usize,
    editing: bool,
    _edit_buffer: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(9),   // Settings
            Constraint::Length(3),   // Checkbox
            Constraint::Length(4),   // Status
            Constraint::Length(3),   // Buttons
            Constraint::Length(2),   // Help
        ])
        .split(frame.area());

    // Title
    let title = Paragraph::new(" Self-Awareness Monitor ")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, chunks[0]);

    // Settings panel
    let focused_style = if editing {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let normal_style = Style::default();

    let settings_text = Text::from(vec![
        Line::from(vec![
            Span::styled("  Interval:    ", normal_style),
            Span::styled(
                format!("{}s", config.interval_seconds),
                if focused_field == 0 { focused_style } else { normal_style },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Max Disk:    ", normal_style),
            Span::styled(
                format!("{} MB", config.max_disk_mb),
                if focused_field == 1 { focused_style } else { normal_style },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Output Dir:  ", normal_style),
            Span::styled(
                config.output_dir.as_str(),
                if focused_field == 2 { focused_style } else { normal_style },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Image Format:", normal_style),
            Span::styled(
                config.image_format.label(),
                if focused_field == 3 { focused_style } else { normal_style },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Retention:   ", normal_style),
            Span::styled(
                format!("{} days", config.retention_days),
                if focused_field == 4 { focused_style } else { normal_style },
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Disk Usage:  ", normal_style),
            Span::styled(
                format_usage(config::app_dir().join(&config.output_dir)),
                normal_style,
            ),
        ]),
    ]);

    let settings = Paragraph::new(settings_text)
        .block(Block::default().borders(Borders::ALL).title(" Settings "));
    frame.render_widget(settings, chunks[1]);

    // Checkbox
    let checkbox_label = if config.start_on_boot {
        " [x] Continue logging at startup"
    } else {
        " [ ] Continue logging at startup"
    };
    let checkbox = Paragraph::new(checkbox_label)
        .block(Block::default().borders(Borders::ALL).title(" Startup "));
    frame.render_widget(checkbox, chunks[2]);

    // Status panel
    let daemon_running = daemon::is_daemon_running();
    let startup_enabled = startup::is_enabled().unwrap_or(false);
    let cleanup_enabled = cleanup::is_cleanup_task_enabled().unwrap_or(false);

    let status_text = Text::from(vec![
        Line::from(vec![
            Span::styled("  Daemon:     ", normal_style),
            Span::styled(
                if daemon_running { "Running" } else { "Stopped" },
                if daemon_running {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Startup:    ", normal_style),
            Span::styled(
                if startup_enabled { "Enabled" } else { "Disabled" },
                if startup_enabled {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Cleanup:    ", normal_style),
            Span::styled(
                if cleanup_enabled { "Scheduled" } else { "Not scheduled" },
                if cleanup_enabled {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
        ]),
        Line::from(""),
    ]);

    let status = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    frame.render_widget(status, chunks[3]);

    // Buttons
    let buttons_text = Text::from(vec![
        Line::from(vec![
            Span::styled(" [S]ave Config  ", normal_style),
            Span::styled(" [D]aemon  ", normal_style),
            Span::styled(" [X] Stop  ", normal_style),
            Span::styled(" [C] Boot  ", normal_style),
            Span::styled(" [Q]uit  ", normal_style),
        ]),
    ]);
    let buttons = Paragraph::new(buttons_text)
        .block(Block::default().borders(Borders::ALL).title(" Actions "));
    frame.render_widget(buttons, chunks[4]);

    // Help
    let help = Paragraph::new(" Tab/Shift+Tab: Navigate | Enter: Edit | Esc: Cancel/Quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[5]);
}

fn apply_edit(config: &mut Config, field: usize, buffer: &str) {
    match field {
        0 => {
            if let Ok(val) = buffer.parse::<u64>() {
                config.interval_seconds = val.max(1);
            }
        }
        1 => {
            if let Ok(val) = buffer.parse::<u64>() {
                config.max_disk_mb = val.max(1);
            }
        }
        2 => {
            config.output_dir = buffer.to_string();
        }
        3 => {
            // Cycle through formats
            let formats = ImageFormat::all();
            let current = config.image_format;
            let idx = formats.iter().position(|&f| f == current).unwrap_or(0);
            config.image_format = formats[(idx + 1) % formats.len()];
        }
        4 => {
            if let Ok(val) = buffer.parse::<u64>() {
                config.retention_days = val.max(1);
            }
        }
        _ => {}
    }
}

fn format_usage(dir: std::path::PathBuf) -> String {
    if !dir.exists() {
        return "0 B".to_string();
    }

    let mut total: u64 = 0;
    let mut count: u64 = 0;

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    if ext == "webp" || ext == "jpg" || ext == "jpeg" || ext == "png" {
                        if let Ok(metadata) = path.metadata() {
                            total += metadata.len();
                            count += 1;
                        }
                    }
                }
            }
        }
    }

    if total < 1024 {
        format!("{} files, {} B", count, total)
    } else if total < 1024 * 1024 {
        format!("{} files, {:.1} KB", count, total as f64 / 1024.0)
    } else {
        format!("{} files, {:.1} MB", count, total as f64 / (1024.0 * 1024.0))
    }
}
