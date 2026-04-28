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
    widgets::{Block, Borders, Paragraph, Row, Table},
    Terminal,
};
use std::io;
use std::time::Duration;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::config::{self, Config, ImageFormat};
use crate::daemon;
use crate::startup;
use crate::cleanup;

/// Which page the TUI is currently showing.
#[derive(Debug, PartialEq)]
enum Page {
    Main,
    Tasks,
}

/// Action returned by the TUI after the user exits.
#[derive(Debug, PartialEq)]
pub enum TuiAction {
    Quit,            // Q: stop daemon and exit
    QuitNoStop,      // Esc: exit without stopping daemon
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
    let mut page = Page::Main;
    let mut message: Option<String> = None;
    let mut message_timeout = std::time::Instant::now();
    let mut confirm_restart: bool = false;
    let mut restarting: bool = false;
    let mut restart_start: Option<std::time::Instant> = None;

    loop {
        terminal.draw(|frame| {
            match page {
                Page::Main => ui(frame, config, focused_field, editing, &edit_buffer, &message, &message_timeout, confirm_restart, restarting, &restart_start),
                Page::Tasks => tasks_ui(frame, config),
            }
        })?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // --- TASKS PAGE ---
                if page == Page::Tasks {
                    match key.code {
                        KeyCode::Esc => {
                            page = Page::Main;
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            // Toggle tasks page
                            page = Page::Main;
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            // Toggle startup task
                            if startup::is_enabled().unwrap_or(false) {
                                let _ = startup::disable_startup();
                            } else {
                                let _ = startup::enable_startup();
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            // Toggle cleanup task
                            if cleanup::is_cleanup_task_enabled().unwrap_or(false) {
                                let _ = cleanup::remove_cleanup_task();
                            } else {
                                let _ = cleanup::create_cleanup_task(config);
                            }
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            // Clear ALL tasks
                            let _ = startup::disable_startup();
                            let _ = cleanup::remove_cleanup_task();
                        }
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            let _ = daemon::stop_daemon();
                            return Ok(TuiAction::Quit);
                        }
                        KeyCode::Char('e') | KeyCode::Char('E') => {
                            // Exit without stopping daemon
                            return Ok(TuiAction::QuitNoStop);
                        }
                        _ => {}
                    }
                    continue;
                }

                // --- EDIT MODE: only process editing keys ---
                if editing {
                    match key.code {
                        KeyCode::Enter => {
                            apply_edit(config, focused_field, &edit_buffer);
                            editing = false;
                        }
                        KeyCode::Esc => {
                            // Cancel edit, go back to navigation
                            editing = false;
                        }
                        KeyCode::Backspace => {
                            edit_buffer.pop();
                        }
                        KeyCode::Char(c) => {
                            edit_buffer.push(c);
                        }
                        KeyCode::Tab => {
                            // In edit mode, Tab moves to next field and starts editing it
                            focused_field = (focused_field + 1) % 5;
                            edit_buffer = match focused_field {
                                0 => config.interval_seconds.to_string(),
                                1 => config.max_disk_mb.to_string(),
                                2 => config.output_dir.clone(),
                                3 => config.image_format.label().to_string(),
                                4 => config.retention_days.to_string(),
                                _ => String::new(),
                            };
                        }
                        KeyCode::BackTab => {
                            // In edit mode, BackTab moves to previous field and starts editing it
                            focused_field = if focused_field == 0 { 4 } else { focused_field - 1 };
                            edit_buffer = match focused_field {
                                0 => config.interval_seconds.to_string(),
                                1 => config.max_disk_mb.to_string(),
                                2 => config.output_dir.clone(),
                                3 => config.image_format.label().to_string(),
                                4 => config.retention_days.to_string(),
                                _ => String::new(),
                            };
                        }
                        _ => {}
                    }
                    continue;
                }

                // --- RESTART CONFIRMATION: handle Y/N before other keys ---
                if confirm_restart {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // Confirm restart: stop daemon, regenerate cleanup, start daemon
                            confirm_restart = false;
                            restarting = true;
                            restart_start = Some(std::time::Instant::now());

                            // Stop the old daemon
                            let _ = daemon::stop_daemon();

                            // Regenerate cleanup.bat with current config
                            let _ = cleanup::create_cleanup_task(config);

                            // Give a moment for the old daemon to shut down
                            std::thread::sleep(Duration::from_millis(500));
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            // Cancel restart
                            confirm_restart = false;
                        }
                        _ => {}
                    }
                    continue;
                }

                // --- NAVIGATION MODE: only process navigation and action keys ---
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                        // Quit: stop daemon, then exit
                        let _ = daemon::stop_daemon();
                        return Ok(TuiAction::Quit);
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        // Save config, then ask to restart daemon
                        config.save()?;
                        confirm_restart = true;
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        // Start daemon if not already running
                        if daemon::is_daemon_running() {
                            message = Some("Daemon is already running.".to_string());
                            message_timeout = std::time::Instant::now();
                        } else {
                            let exe_path = std::env::current_exe().unwrap();
                            #[cfg(target_os = "windows")]
                            let child = std::process::Command::new(&exe_path)
                                .creation_flags(0x00000008) // DETACHED_PROCESS — not attached to TUI console
                                .spawn();
                            #[cfg(not(target_os = "windows"))]
                            let child = std::process::Command::new(&exe_path)
                                .spawn();

                            match child {
                                Ok(_child) => {
                                    message = Some("Daemon started.".to_string());
                                    message_timeout = std::time::Instant::now();
                                    // Give the new daemon a moment to write its PID file
                                    std::thread::sleep(Duration::from_millis(300));
                                }
                                Err(e) => {
                                    message = Some(format!("Failed to start daemon: {}", e));
                                    message_timeout = std::time::Instant::now();
                                }
                            }
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Char('X') => {
                        // Stop daemon, stay in TUI (no save)
                        if daemon::is_daemon_running() {
                            match daemon::stop_daemon() {
                                Ok(()) => {
                                    message = Some("Daemon stopped.".to_string());
                                    message_timeout = std::time::Instant::now();
                                }
                                Err(e) => {
                                    message = Some(format!("Daemon stop error: {}", e));
                                    message_timeout = std::time::Instant::now();
                                }
                            }
                        } else {
                            message = Some("No daemon is running.".to_string());
                            message_timeout = std::time::Instant::now();
                        }
                    }
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        // Toggle startup persistence (no save)
                        config.start_on_boot = !config.start_on_boot;
                        // Update the actual Task Scheduler tasks immediately
                        if config.start_on_boot {
                            match startup::enable_startup() {
                                Ok(()) => {
                                    let _ = cleanup::create_cleanup_task(config);
                                    message = Some("Startup task enabled.".to_string());
                                    message_timeout = std::time::Instant::now();
                                }
                                Err(e) => {
                                    message = Some(format!("Failed to enable startup: {}", e));
                                    message_timeout = std::time::Instant::now();
                                    config.start_on_boot = false;
                                }
                            }
                        } else {
                            match startup::disable_startup() {
                                Ok(()) => {
                                    let _ = cleanup::remove_cleanup_task();
                                    message = Some("Startup task disabled.".to_string());
                                    message_timeout = std::time::Instant::now();
                                }
                                Err(e) => {
                                    message = Some(format!("Failed to disable startup: {}", e));
                                    message_timeout = std::time::Instant::now();
                                    config.start_on_boot = true;
                                }
                            }
                        }
                    }
                    KeyCode::Char('t') | KeyCode::Char('T') => {
                        // Switch to tasks page
                        page = Page::Tasks;
                    }
                    KeyCode::Enter => {
                        // Start editing the focused field
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
                    KeyCode::Esc => {
                        // Just close TUI, don't stop daemon, don't save
                        return Ok(TuiAction::QuitNoStop);
                    }
                    KeyCode::Tab => {
                        focused_field = (focused_field + 1) % 5;
                    }
                    KeyCode::BackTab => {
                        focused_field = if focused_field == 0 { 4 } else { focused_field - 1 };
                    }
                    _ => {}
                }
            }
        }

        // Check if restart wait period has elapsed
        if restarting {
            if let Some(start) = restart_start {
                if start.elapsed() >= Duration::from_millis(500) {
                    // Start the new daemon
                    let exe_path = std::env::current_exe().unwrap();
                    #[cfg(target_os = "windows")]
                    let child = std::process::Command::new(&exe_path)
                        .creation_flags(0x00000008) // DETACHED_PROCESS
                        .spawn();
                    #[cfg(not(target_os = "windows"))]
                    let child = std::process::Command::new(&exe_path)
                        .spawn();

                    match child {
                        Ok(_child) => {
                            message = Some("Daemon restarted.".to_string());
                            message_timeout = std::time::Instant::now();
                            // Give the new daemon a moment to write its PID file
                            std::thread::sleep(Duration::from_millis(300));
                        }
                        Err(e) => {
                            message = Some(format!("Failed to restart daemon: {}", e));
                            message_timeout = std::time::Instant::now();
                        }
                    }

                    restarting = false;
                    restart_start = None;
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
    edit_buffer: &str,
    message: &Option<String>,
    message_timeout: &std::time::Instant,
    confirm_restart: bool,
    restarting: bool,
    _restart_start: &Option<std::time::Instant>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(9),   // Settings
            Constraint::Length(3),   // Checkbox
            Constraint::Length(4),   // Status
            Constraint::Length(3),   // Buttons
            Constraint::Length(2),   // Message
            Constraint::Length(1),   // Help
        ])
        .split(frame.area());

    // Title
    let title = Paragraph::new(" Self-Awareness Monitor ")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, chunks[0]);

    // Settings panel
    let focused_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let editing_style = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let normal_style = Style::default();

    // Determine the display value for the focused field
    let field_values: Vec<String> = vec![
        if focused_field == 0 && editing {
            format!("{}s [{}]", config.interval_seconds, edit_buffer)
        } else {
            format!("{}s", config.interval_seconds)
        },
        if focused_field == 1 && editing {
            format!("{} MB [{}]", config.max_disk_mb, edit_buffer)
        } else {
            format!("{} MB", config.max_disk_mb)
        },
        if focused_field == 2 && editing {
            format!("[{}]", edit_buffer)
        } else {
            config.output_dir.clone()
        },
        if focused_field == 3 && editing {
            // Show current format, cycling on Enter
            config.image_format.label().to_string()
        } else {
            config.image_format.label().to_string()
        },
        if focused_field == 4 && editing {
            format!("{} days [{}]", config.retention_days, edit_buffer)
        } else {
            format!("{} days", config.retention_days)
        },
    ];

    let settings_text = Text::from(vec![
        Line::from(vec![
            Span::styled("  Interval:    ", normal_style),
            Span::styled(
                &field_values[0],
                if focused_field == 0 {
                    if editing { editing_style } else { focused_style }
                } else {
                    normal_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Max Disk:    ", normal_style),
            Span::styled(
                &field_values[1],
                if focused_field == 1 {
                    if editing { editing_style } else { focused_style }
                } else {
                    normal_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Output Dir:  ", normal_style),
            Span::styled(
                &field_values[2],
                if focused_field == 2 {
                    if editing { editing_style } else { focused_style }
                } else {
                    normal_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Image Format:", normal_style),
            Span::styled(
                &field_values[3],
                if focused_field == 3 {
                    if editing { editing_style } else { focused_style }
                } else {
                    normal_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Retention:   ", normal_style),
            Span::styled(
                &field_values[4],
                if focused_field == 4 {
                    if editing { editing_style } else { focused_style }
                } else {
                    normal_style
                },
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Disk Usage:  ", normal_style),
            Span::styled(
                format_usage(&config.output_dir),
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
    let pid_file_exists = config::daemon_pid_path().exists();
    let startup_enabled = startup::is_enabled().unwrap_or(false);
    let cleanup_enabled = cleanup::is_cleanup_task_enabled().unwrap_or(false);

    let (daemon_label, daemon_color) = if restarting {
        ("Restarting...", Color::Yellow)
    } else if daemon_running {
        ("Running", Color::Green)
    } else if pid_file_exists {
        ("Stopped (Died)", Color::Yellow)
    } else {
        ("Stopped", Color::Red)
    };

    let status_text = Text::from(vec![
        Line::from(vec![
            Span::styled("  Daemon:     ", normal_style),
            Span::styled(daemon_label, Style::default().fg(daemon_color)),
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

    // Buttons / help
    let buttons_text = if confirm_restart {
        Text::from(vec![
            Line::from(vec![
                Span::styled(" Restart daemon? [Y/N]  ", Style::default().fg(Color::Yellow)),
            ]),
        ])
    } else if restarting {
        Text::from(vec![
            Line::from(vec![
                Span::styled(" Restarting...  ", Style::default().fg(Color::Yellow)),
            ]),
        ])
    } else if editing {
        Text::from(vec![
            Line::from(vec![
                Span::styled(" [Enter] Save  ", Style::default().fg(Color::Yellow)),
                Span::styled(" [Esc] Cancel  ", Style::default().fg(Color::Yellow)),
                Span::styled(" [Tab] Next  ", Style::default().fg(Color::Yellow)),
            ]),
        ])
    } else {
        Text::from(vec![
            Line::from(vec![
                Span::styled(" [S]ave Config  ", normal_style),
                Span::styled(" [D]aemon  ", normal_style),
                Span::styled(" [X] Stop  ", normal_style),
                Span::styled(" [C] Boot  ", normal_style),
                Span::styled(" [T]asks  ", normal_style),
                Span::styled(" [Q]uit  ", normal_style),
            ]),
        ])
    };
    let buttons = Paragraph::new(buttons_text)
        .block(Block::default().borders(Borders::ALL).title(" Actions "));
    frame.render_widget(buttons, chunks[4]);

    // Message area
    let message_text = match message {
        Some(msg) if message_timeout.elapsed().as_secs() < 5 => {
            Paragraph::new(msg.as_str())
                .style(Style::default().fg(Color::Yellow))
        }
        _ => Paragraph::new("")
            .style(Style::default().fg(Color::DarkGray)),
    };
    frame.render_widget(message_text, chunks[5]);

    // Help
    let help = if editing {
        Paragraph::new(" Enter: Confirm | Esc: Cancel | Tab: Next field")
            .style(Style::default().fg(Color::DarkGray))
    } else {
        Paragraph::new(" Tab/Shift+Tab: Navigate | Enter: Edit | Esc: Quit")
            .style(Style::default().fg(Color::DarkGray))
    };
    frame.render_widget(help, chunks[6]);
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
            // Convert relative paths to absolute
            let path = std::path::Path::new(buffer);
            if path.is_relative() {
                if let Ok(cwd) = std::env::current_dir() {
                    config.output_dir = cwd.join(path).to_string_lossy().to_string();
                } else {
                    config.output_dir = buffer.to_string();
                }
            } else {
                config.output_dir = buffer.to_string();
            }
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

fn format_usage(dir: &str) -> String {
    let path = std::path::Path::new(dir);
    if !path.exists() {
        return "N/A (dir does not exist)".to_string();
    }

    let mut total: u64 = 0;
    let mut count: u64 = 0;

    if let Ok(entries) = std::fs::read_dir(path) {
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

    if count == 0 {
        "0 files".to_string()
    } else if total < 1024 {
        format!("{} files, {} B", count, total)
    } else if total < 1024 * 1024 {
        format!("{} files, {:.1} KB", count, total as f64 / 1024.0)
    } else {
        format!("{} files, {:.1} MB", count, total as f64 / (1024.0 * 1024.0))
    }
}

/// Render the scheduled tasks management page.
fn tasks_ui(frame: &mut ratatui::Frame, _config: &Config) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(1),   // Spacer
            Constraint::Length(11),  // Tasks table
            Constraint::Length(1),   // Spacer
            Constraint::Length(5),   // Actions
            Constraint::Length(2),   // Help
        ])
        .split(frame.area());

    // Title
    let title = Paragraph::new(" Scheduled Tasks ")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, chunks[0]);

    // Build task rows
    let startup_enabled = startup::is_enabled().unwrap_or(false);
    let cleanup_enabled = cleanup::is_cleanup_task_enabled().unwrap_or(false);

    let rows = vec![
        Row::new(vec![
            "SelfAwarenessStartup".to_string(),
            if startup_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
            "Runs at logon".to_string(),
        ]),
        Row::new(vec![
            "SelfAwarenessCleanup".to_string(),
            if cleanup_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
            "Daily at 02:00".to_string(),
        ]),
    ];

    let task_table = Table::new(
        rows,
        [Constraint::Percentage(35), Constraint::Percentage(25), Constraint::Percentage(40)],
    )
    .header(
        Row::new(vec![
            Span::styled("  Task Name", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled("  Status", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled("  Schedule", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ])
    )
    .block(Block::default().borders(Borders::ALL).title(" Task List "));
    frame.render_widget(task_table, chunks[2]);

    // Actions
    let actions_text = Text::from(vec![
        Line::from(vec![
            Span::styled(" [S]tartup  ", Style::default().fg(Color::Yellow)),
            Span::styled(" [C]leanup  ", Style::default().fg(Color::Yellow)),
            Span::styled(" [A]ll Clear  ", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled(" [T]ab Back  ", Style::default().fg(Color::Yellow)),
            Span::styled(" [Q]uit  ", Style::default().fg(Color::Yellow)),
            Span::styled(" [E]xit  ", Style::default().fg(Color::Yellow)),
        ]),
    ]);
    let actions = Paragraph::new(actions_text)
        .block(Block::default().borders(Borders::ALL).title(" Controls "));
    frame.render_widget(actions, chunks[4]);

    // Help
    let help = Paragraph::new(" Esc/T: Back to main | S: Toggle startup | C: Toggle cleanup | A: Clear all")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[5]);
}
