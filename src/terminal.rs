use crate::config::ResolvedTerminalSettings;
use crate::connection::{ConnectionBackend, ConnectionProfile};
use crate::ssh;
use anyhow::{anyhow, Context, Result};
use chrono::Local;
use portable_pty::{native_pty_system, Child, CommandBuilder, ExitStatus, MasterPty, PtySize};
use smol::channel::Receiver as SmolReceiver;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use unicode_width::UnicodeWidthChar;
use wezterm_term::color::ColorPalette;
use wezterm_term::{Terminal, TerminalConfiguration, TerminalSize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    Connecting,
    Connected,
    Attention,
    Error,
    Exited,
}

impl SessionPhase {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Connecting => "status-indicator",
            Self::Connected => "status-indicator connected",
            Self::Attention => "status-indicator",
            Self::Error | Self::Exited => "status-indicator error",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Live",
            Self::Attention => "Attention",
            Self::Error => "Error",
            Self::Exited => "Exited",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub title: String,
    pub subtitle: String,
    pub backend: String,
    pub phase: SessionPhase,
    pub status_line: String,
    pub started_at: String,
    pub updated_at: String,
}

impl SessionSnapshot {
    fn new(title: &str, subtitle: &str, backend: &str) -> Self {
        let now = clock_label();
        Self {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            backend: backend.to_string(),
            phase: SessionPhase::Connecting,
            status_line: "Starting session...".to_string(),
            started_at: now.clone(),
            updated_at: now,
        }
    }

    fn from_profile(profile: &ConnectionProfile) -> Self {
        Self::new(
            &profile.name,
            &profile.host_label(),
            ssh::backend_caption(profile.backend),
        )
    }
}

#[derive(Debug)]
enum SessionCommand {
    InputBytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Shutdown,
}

// Lock ordering (deadlock prevention): terminal → master → snapshot
#[derive(Clone)]
pub struct TerminalSessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    snapshot: Arc<Mutex<SessionSnapshot>>,
    terminal: Arc<Mutex<Terminal>>,
}

impl std::fmt::Debug for TerminalSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalSessionHandle")
            .field("phase", &self.snapshot().phase)
            .finish()
    }
}

impl TerminalSessionHandle {
    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn send_bytes(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::InputBytes(bytes))
            .map_err(|_| anyhow!("session command channel is closed"))?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Resize { cols, rows })
            .map_err(|_| anyhow!("session command channel is closed"))?;
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.command_tx.send(SessionCommand::Shutdown);
    }

    pub fn screen_text(&self, max_lines: usize) -> String {
        let Ok(terminal) = self.terminal.lock() else {
            return String::new();
        };
        let screen = terminal.screen();
        let total = screen.scrollback_rows();
        let start = total.saturating_sub(max_lines);
        let mut text = String::new();
        screen.with_phys_lines(start..total, |lines| {
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    text.push('\n');
                }
                text.push_str(line.as_str().trim_end());
            }
        });
        text
    }

    pub fn screen_text_with_cursor(&self, max_lines: usize) -> (String, Option<i32>) {
        let Ok(terminal) = self.terminal.lock() else {
            return (String::new(), None);
        };
        let cursor = terminal.cursor_pos();
        let screen = terminal.screen();
        let total = screen.scrollback_rows();
        let phys_rows = screen.physical_rows;
        let start = total.saturating_sub(max_lines);

        let visible_start = total.saturating_sub(phys_rows);
        let cursor_abs = (visible_start as i64 + cursor.y) as isize;
        let cursor_text_line = cursor_abs - start as isize;

        let mut text = String::new();
        let mut cursor_char_offset: Option<i32> = None;
        let mut total_chars: i32 = 0;

        screen.with_phys_lines(start..total, |lines| {
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    text.push('\n');
                    total_chars += 1;
                }

                let trimmed = line.as_str();
                let trimmed = trimmed.trim_end();

                if i as isize == cursor_text_line && cursor_text_line >= 0 {
                    let mut cell_col: usize = 0;
                    let mut char_in_line: i32 = 0;
                    for ch in trimmed.chars() {
                        if cell_col >= cursor.x {
                            break;
                        }
                        char_in_line += 1;
                        cell_col += char_display_width(ch);
                    }
                    cursor_char_offset = Some(total_chars + char_in_line);
                }

                total_chars += trimmed.chars().count() as i32;
                text.push_str(trimmed);
            }
        });

        (text, cursor_char_offset)
    }

    pub fn with_terminal<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Terminal) -> R,
    {
        let terminal = self.terminal.lock().ok()?;
        Some(f(&terminal))
    }

    pub fn with_terminal_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Terminal) -> R,
    {
        let mut terminal = self.terminal.lock().ok()?;
        Some(f(&mut terminal))
    }
}

pub fn find_local_shell() -> PathBuf {
    if let Ok(shell) = std::env::var("RSHELL_SHELL") {
        return PathBuf::from(shell);
    }

    #[cfg(windows)]
    {
        PathBuf::from("powershell.exe")
    }

    #[cfg(not(windows))]
    {
        if let Ok(shell) = std::env::var("SHELL") {
            return PathBuf::from(shell);
        }
        PathBuf::from("/bin/sh")
    }
}

pub fn launch_local_session(settings: ResolvedTerminalSettings) -> Result<TerminalSessionHandle> {
    let shell = find_local_shell();
    let shell_name = shell
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "shell".into());

    let snapshot = Arc::new(Mutex::new(SessionSnapshot::new(
        "Local Shell",
        &shell_name,
        "local",
    )));

    let initial_size = PtySize {
        rows: settings.initial_rows,
        cols: settings.initial_cols,
        pixel_width: settings.initial_cols * 8,
        pixel_height: settings.initial_rows * 16,
    };

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(initial_size)
        .context("failed to allocate PTY for local shell")?;

    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    cmd.env("TERM", &settings.terminal_type);

    let child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn local shell")?;

    {
        let mut snap = snapshot.lock().unwrap();
        snap.phase = SessionPhase::Connected;
        snap.status_line = format!("Local shell ({shell_name}) running");
    }

    start_session_threads(
        pair.master,
        child,
        BackendGuard::System,
        initial_size,
        snapshot,
        settings,
    )
}

pub fn launch_session(
    profile: &ConnectionProfile,
    settings: ResolvedTerminalSettings,
) -> Result<TerminalSessionHandle> {
    let snapshot = Arc::new(Mutex::new(SessionSnapshot::from_profile(profile)));

    let session_parts = create_session_parts(profile, Arc::clone(&snapshot))?;
    let SessionParts {
        master,
        child,
        backend_guard,
        initial_size,
    } = session_parts;

    start_session_threads(master, child, backend_guard, initial_size, snapshot, settings)
}

fn start_session_threads(
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send>,
    backend_guard: BackendGuard,
    initial_size: PtySize,
    snapshot: Arc<Mutex<SessionSnapshot>>,
    settings: ResolvedTerminalSettings,
) -> Result<TerminalSessionHandle> {
    let (command_tx, command_rx) = mpsc::channel();

    let mut reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let master = Arc::new(Mutex::new(master));

    let terminal_writer = SharedWriter {
        master: Arc::clone(&master),
    };

    let terminal = Terminal::new(
        terminal_size(initial_size),
        Arc::new(RshellTerminalConfig { settings }),
        "rsHell",
        env!("CARGO_PKG_VERSION"),
        Box::new(terminal_writer),
    );
    let terminal = Arc::new(Mutex::new(terminal));

    let reader_terminal = Arc::clone(&terminal);
    let reader_state = Arc::clone(&snapshot);
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    set_status(
                        &reader_state,
                        SessionPhase::Exited,
                        "Stream closed".to_string(),
                    );
                    break;
                }
                Ok(read) => {
                    {
                        let Ok(mut term) = reader_terminal.lock() else {
                            set_status(
                                &reader_state,
                                SessionPhase::Error,
                                "Terminal state corrupted".into(),
                            );
                            break;
                        };
                        term.advance_bytes(&buffer[..read]);
                    }
                    {
                        let Ok(mut snap) = reader_state.lock() else {
                            break;
                        };
                        if snap.phase == SessionPhase::Connecting {
                            snap.phase = SessionPhase::Connected;
                            snap.status_line = format!(
                                "Terminal stream active · updated {}",
                                clock_label()
                            );
                        } else if snap.phase != SessionPhase::Error
                            && snap.phase != SessionPhase::Exited
                        {
                            snap.phase = SessionPhase::Connected;
                            snap.status_line =
                                format!("Last output received at {}", clock_label());
                        }
                        snap.updated_at = clock_label();
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    set_status(
                        &reader_state,
                        SessionPhase::Error,
                        format!("PTY read failed: {error}"),
                    );
                    break;
                }
            }
        }
    });

    let cmd_state = Arc::clone(&snapshot);
    let cmd_terminal = Arc::clone(&terminal);
    let cmd_master = Arc::clone(&master);
    thread::spawn(move || {
        let mut killer = child.clone_killer();
        let mut child = child;
        let _guard = backend_guard;

        loop {
            match command_rx.recv_timeout(Duration::from_millis(150)) {
                Ok(SessionCommand::InputBytes(bytes)) => {
                    if let Err(error) = write_to_session(&cmd_master, &bytes) {
                        set_status(
                            &cmd_state,
                            SessionPhase::Error,
                            format!("Send failed: {error}"),
                        );
                        break;
                    }
                }
                Ok(SessionCommand::Resize { cols, rows }) => {
                    if let Ok(mut term) = cmd_terminal.lock() {
                        term.resize(TerminalSize {
                            rows: rows as usize,
                            cols: cols as usize,
                            pixel_width: (cols as usize).saturating_mul(8),
                            pixel_height: (rows as usize).saturating_mul(16),
                            dpi: 96,
                        });
                    }
                    {
                        let size = PtySize {
                            cols,
                            rows,
                            pixel_width: cols.saturating_mul(8),
                            pixel_height: rows.saturating_mul(16),
                        };
                        if let Ok(master) = cmd_master.lock()
                            && let Err(error) = master.resize(size)
                        {
                            set_status(
                                &cmd_state,
                                SessionPhase::Attention,
                                format!("Resize request failed: {error}"),
                            );
                        }
                    }
                }
                Ok(SessionCommand::Shutdown) => {
                    let _ = killer.kill();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = killer.kill();
                    break;
                }
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    set_status(
                        &cmd_state,
                        SessionPhase::Exited,
                        format!("Session ended with {}", describe_exit_status(&status)),
                    );
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    set_status(
                        &cmd_state,
                        SessionPhase::Error,
                        format!("Failed to poll child process: {error}"),
                    );
                    break;
                }
            }
        }

        let _ = reader_thread.join();
    });

    Ok(TerminalSessionHandle {
        command_tx,
        snapshot,
        terminal,
    })
}

struct SessionParts {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send>,
    backend_guard: BackendGuard,
    initial_size: PtySize,
}

enum BackendGuard {
    System,
    WezTerm { _session: wezterm_ssh::Session },
}

fn create_session_parts(
    profile: &ConnectionProfile,
    state: Arc<Mutex<SessionSnapshot>>,
) -> Result<SessionParts> {
    let initial_size = PtySize {
        rows: 36,
        cols: 120,
        pixel_width: 960,
        pixel_height: 640,
    };

    match profile.backend {
        ConnectionBackend::SystemOpenSsh => create_system_session(profile, initial_size),
        ConnectionBackend::WezTermSsh => create_wezterm_session(profile, state, initial_size),
    }
}

fn create_system_session(
    profile: &ConnectionProfile,
    initial_size: PtySize,
) -> Result<SessionParts> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(initial_size)
        .context("failed to allocate PTY for system OpenSSH")?;
    let command = ssh::build_system_command(profile);
    let child = pair
        .slave
        .spawn_command(command)
        .context("failed to spawn system OpenSSH")?;

    Ok(SessionParts {
        master: pair.master,
        child,
        backend_guard: BackendGuard::System,
        initial_size,
    })
}

fn create_wezterm_session(
    profile: &ConnectionProfile,
    state: Arc<Mutex<SessionSnapshot>>,
    initial_size: PtySize,
) -> Result<SessionParts> {
    let config = ssh::build_wezterm_config(profile);
    let (session, events) = wezterm_ssh::Session::connect(config)
        .context("failed to initialize wezterm-ssh session")?;

    let password = if profile.password.trim().is_empty() {
        None
    } else {
        Some(profile.password.trim().to_string())
    };

    let auto_accept = profile.accept_new_host;
    let event_state = Arc::clone(&state);
    thread::spawn(move || {
        pump_wezterm_events(events, event_state, password, auto_accept);
    });

    let command = (!profile.remote_command.trim().is_empty())
        .then(|| profile.remote_command.trim().to_string());
    let pty_session = session.clone();
    let pty_state = Arc::clone(&state);
    let (parts_tx, parts_rx) =
        mpsc::sync_channel::<Result<(wezterm_ssh::SshPty, wezterm_ssh::SshChildProcess)>>(1);

    thread::spawn(move || {
        let result = smol::block_on(pty_session.request_pty(
            "xterm-256color",
            initial_size,
            command.as_deref(),
            None,
        ))
        .context("failed to request remote PTY from wezterm-ssh");

        if let Err(ref e) = result {
            set_status(
                &pty_state,
                SessionPhase::Error,
                format!("PTY request failed: {e:#}"),
            );
        }
        let _ = parts_tx.send(result);
    });

    let (pty, child) = parts_rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| anyhow!("timed out waiting for remote PTY allocation"))??;

    Ok(SessionParts {
        master: Box::new(pty),
        child: Box::new(child),
        backend_guard: BackendGuard::WezTerm { _session: session },
        initial_size,
    })
}

fn pump_wezterm_events(
    events: SmolReceiver<wezterm_ssh::SessionEvent>,
    state: Arc<Mutex<SessionSnapshot>>,
    password: Option<String>,
    auto_accept: bool,
) {
    smol::block_on(async move {
        while let Ok(event) = events.recv().await {
            match event {
                wezterm_ssh::SessionEvent::Banner(message) => {
                    if let Some(message) = message {
                        set_status(&state, SessionPhase::Connecting, message);
                    }
                }
                wezterm_ssh::SessionEvent::HostVerify(verification) => {
                    let message = if auto_accept {
                        format!(
                            "Host verification accepted automatically: {}",
                            verification.message
                        )
                    } else {
                        format!("Host verification rejected: {}", verification.message)
                    };
                    set_status(
                        &state,
                        if auto_accept {
                            SessionPhase::Connecting
                        } else {
                            SessionPhase::Error
                        },
                        message,
                    );
                    let _ = verification.answer(auto_accept).await;
                }
                wezterm_ssh::SessionEvent::Authenticate(authentication) => {
                    let answers = authentication
                        .prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.echo {
                                String::new()
                            } else {
                                password.clone().unwrap_or_default()
                            }
                        })
                        .collect::<Vec<_>>();

                    set_status(
                        &state,
                        SessionPhase::Connecting,
                        if answers.iter().all(|value| value.is_empty()) {
                            "Authentication challenge received but no password is configured"
                                .to_string()
                        } else {
                            "Authentication challenge answered using saved credentials".to_string()
                        },
                    );
                    let _ = authentication.answer(answers).await;
                }
                wezterm_ssh::SessionEvent::Error(error) => {
                    set_status(&state, SessionPhase::Error, error);
                }
                wezterm_ssh::SessionEvent::Authenticated => {
                    set_status(
                        &state,
                        SessionPhase::Connected,
                        "Authenticated via wezterm-ssh".to_string(),
                    );
                }
            }
        }
    });
}

#[derive(Debug)]
struct RshellTerminalConfig {
    settings: ResolvedTerminalSettings,
}

impl TerminalConfiguration for RshellTerminalConfig {
    fn scrollback_size(&self) -> usize {
        self.settings.scrollback_lines
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }

    fn enable_csi_u_key_encoding(&self) -> bool {
        self.settings.enable_csi_u
    }

    fn enable_kitty_keyboard(&self) -> bool {
        self.settings.enable_kitty_keyboard
    }

    fn enable_kitty_graphics(&self) -> bool {
        self.settings.enable_kitty_graphics
    }

    fn enq_answerback(&self) -> String {
        self.settings.answerback.clone()
    }
}

#[derive(Clone)]
struct SharedWriter {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.master
            .lock()
            .map_err(|_| std::io::Error::other("PTY lock poisoned"))?
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.master
            .lock()
            .map_err(|_| std::io::Error::other("PTY lock poisoned"))?
            .flush()
    }
}

fn write_to_session(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>, bytes: &[u8]) -> Result<()> {
    let mut master = master
        .lock()
        .map_err(|_| anyhow!("PTY lock poisoned"))?;
    master.write_all(bytes)?;
    master.flush()?;
    Ok(())
}

fn set_status(state: &Arc<Mutex<SessionSnapshot>>, phase: SessionPhase, status_line: String) {
    if let Ok(mut snapshot) = state.lock() {
        snapshot.phase = phase;
        snapshot.status_line = status_line;
        snapshot.updated_at = clock_label();
    }
}

fn describe_exit_status(status: &ExitStatus) -> String {
    format!("{status:?}")
}

fn terminal_size(size: PtySize) -> TerminalSize {
    TerminalSize {
        rows: size.rows as usize,
        cols: size.cols as usize,
        pixel_width: size.pixel_width as usize,
        pixel_height: size.pixel_height as usize,
        dpi: 96,
    }
}

fn clock_label() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

fn char_display_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}
