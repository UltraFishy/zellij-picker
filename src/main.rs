use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use regex::Regex;
use std::{io, os::unix::process::CommandExt, process::Command, time::Duration};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct App {
    sessions: Vec<String>,
    list_state: ListState,
    /// If the user pressed 'n', we collect input here before exec-ing zellij.
    new_session_input: Option<String>,
}

impl App {
    fn new(sessions: Vec<String>) -> Self {
        let mut list_state = ListState::default();
        if !sessions.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            sessions,
            list_state,
            new_session_input: None,
        }
    }

    // --- helpers -----------------------------------------------------------

    fn selected_session(&self) -> Option<&str> {
        self.list_state
            .selected()
            .and_then(|i| self.sessions.get(i))
            .map(|s| s.as_str())
    }

    fn move_up(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = if i == 0 {
            self.sessions.len() - 1
        } else {
            i - 1
        };
        self.list_state.select(Some(next));
    }

    fn move_down(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1) % self.sessions.len();
        self.list_state.select(Some(next));
    }
}

// ---------------------------------------------------------------------------
// Fetch live sessions from zellij
// ---------------------------------------------------------------------------

fn get_sessions() -> (Vec<String>, usize) {
    let output = Command::new("zellij").args(["list-sessions"]).output();

    let sessions = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        }
        Err(_) => Vec::new(),
    };

    let num = &sessions.iter().len();

    (sessions, *num)
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

fn ui(f: &mut Frame, app: &App) {
    let area = f.area();

    // Outer layout: top bar | main content | bottom bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(0),    // session list (fills space)
            Constraint::Length(3), // footer / new-session input
        ])
        .split(area);

    // --- Title bar ---------------------------------------------------------
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "  zellij-picker",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  —  pick or create a session"),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(title, chunks[0]);

    // --- Session list ------------------------------------------------------
    if app.new_session_input.is_some() {
        // When typing a new session name we dim the list
        let items: Vec<ListItem> = app
            .sessions
            .iter()
            .map(|s| ListItem::new(format!("  {}", s)).style(Style::default().fg(Color::DarkGray)))
            .collect();

        let list = List::new(items).block(
            Block::default()
                .title(" sessions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(list, chunks[1]);
    } else {
        let items: Vec<ListItem> = app
            .sessions
            .iter()
            .map(|s| ListItem::new(format!("  {}", s)).style(Style::default().fg(Color::White)))
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(" sessions ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");

        // We need a mutable copy of list_state for render_stateful_widget
        let mut state = app.list_state.clone();
        f.render_stateful_widget(list, chunks[1], &mut state);
    }

    // --- Footer ------------------------------------------------------------
    match &app.new_session_input {
        Some(input) => {
            let para = Paragraph::new(Line::from(vec![
                Span::styled(
                    "  new session name: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    input.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "_",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ]))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::Yellow)),
            );
            f.render_widget(para, chunks[2]);
        }
        None => {
            let para = Paragraph::new(Line::from(vec![
                Span::styled("  ↑↓", Style::default().fg(Color::Cyan)),
                Span::raw(" navigate   "),
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::raw(" attach   "),
                Span::styled("n", Style::default().fg(Color::Cyan)),
                Span::raw(" new session   "),
                Span::styled("d", Style::default().fg(Color::Cyan)),
                Span::raw(" kill and delete session   "),
                Span::styled("q", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ]))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            f.render_widget(para, chunks[2]);
        }
    }
}

// ---------------------------------------------------------------------------
// What to do after the TUI exits
// ---------------------------------------------------------------------------

enum ExitAction {
    AttachSession(String),
    NewSession(Option<String>),
    DeleteSession(String),
    Quit,
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn run_tui() -> Result<ExitAction, Box<dyn std::error::Error>> {
    // Setup terminal
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (sessions, num) = get_sessions();
    let mut app = App::new(sessions);

    let action;

    if num != 0 {
        action = loop {
            terminal.draw(|f| ui(f, &app))?;

            // Poll for key events with a short timeout (we don't need ticks here)
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        // --- New session input mode ----------------------------
                        if let Some(ref mut input) = app.new_session_input {
                            match key.code {
                                KeyCode::Enter => {
                                    let name = input.trim().to_string();
                                    if name.is_empty() {
                                        // Empty name → cancel
                                        app.new_session_input = None;
                                    } else {
                                        break ExitAction::NewSession(Some(name));
                                    }
                                }
                                KeyCode::Esc => {
                                    app.new_session_input = None;
                                }
                                KeyCode::Backspace => {
                                    input.pop();
                                }
                                KeyCode::Char(c) => {
                                    // Only allow valid session-name chars
                                    if c.is_alphanumeric() || c == '-' || c == '_' {
                                        input.push(c);
                                    }
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // --- Normal navigation mode ----------------------------
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                break ExitAction::Quit;
                            }
                            KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                            KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                            KeyCode::Char('d') => {
                                if let Some(name) = app.selected_session() {
                                    break ExitAction::DeleteSession(name.to_string());
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(name) = app.selected_session() {
                                    break ExitAction::AttachSession(name.to_string());
                                }
                            }
                            KeyCode::Char('n') => {
                                app.new_session_input = Some(String::new());
                            }
                            _ => {}
                        }
                    }
                    Event::Resize(_, _) => {
                        // Terminal resized — just redraw on next loop iteration
                    }
                    _ => {}
                }
            }
        };
    } else {
        action = ExitAction::NewSession(None);
    }

    // Cleanup terminal
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(action)
}

// Strip colour from selection

fn strip_ansi_codes(input: &str) -> String {
    let ansi_regex = Regex::new(r"\x1B\[[0-?9;]*[mK]").unwrap();
    ansi_regex.replace_all(input, "").to_string()
}

// Get name ready for exec
fn parse_name(input: &str) -> String {
    // need to split and trim by []
    // easy way TODO refactor
    let mut split_bracket = input.split(" [");
    strip_ansi_codes(split_bracket.next().unwrap().trim())
}

// ---------------------------------------------------------------------------
// Main — run TUI then exec zellij (replaces this process)
// ---------------------------------------------------------------------------

fn main() {
    let action = match run_tui() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("zellij-picker error: {}", e);
            std::process::exit(1);
        }
    };

    match action {
        ExitAction::AttachSession(name) => {
            // Get name ready
            let name = parse_name(&name);

            // .exec() replaces the current process image with zellij,
            // so the picker disappears cleanly and there is no zombie parent.
            let err = Command::new("zellij").args(["attach", &name]).exec();
            // If we're still here, exec failed
            eprintln!("Failed to attach to session '{}': {}", name, err);
            std::process::exit(1);
        }
        ExitAction::DeleteSession(name) => {
            // Get name ready
            let name = parse_name(&name);

            println!("Killing session: {}", &name);
            let status = Command::new("zellij")
                .args(["kill-session", &name])
                .status();

            match status {
                Ok(s) if s.success() => {
                    println!("Session killed successfully");
                }
                Ok(s) => {
                    eprintln!(
                        "Failed to kill session '{}': exit code {:?}",
                        name,
                        s.code()
                    );
                }
                Err(e) => {
                    eprintln!("Failed to run kill-session: {}", e);
                }
            }

            println!("Deleting session: {}", &name);
            let status = Command::new("zellij")
                .args(["delete-session", &name])
                .status();

            match status {
                Ok(s) if s.success() => {
                    println!("Session deleted successfully");
                }
                Ok(s) => {
                    eprintln!(
                        "Failed to delete session '{}': exit code {:?}",
                        name,
                        s.code()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Failed to run delete-session: {}", e);
                    std::process::exit(1);
                }
            }
        }
        ExitAction::NewSession(option) => match option {
            Some(name) => {
                let err = Command::new("zellij").args(["--session", &name]).exec();
                eprintln!("Failed to create session '{}': {}", name, err);
                std::process::exit(1);
            }
            _ => {
                let err = Command::new("zellij").exec();
                eprintln!("Failed to create session: {}", err);
                std::process::exit(1);
            }
        },
        ExitAction::Quit => {
            // Just exit cleanly — shell prompt returns
        }
    }
}
