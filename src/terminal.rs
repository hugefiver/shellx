use crate::connection::{ConnectionBackend, ConnectionProfile};
use crate::ssh;
use anyhow::{anyhow, Context, Result};
use chrono::Local;
use portable_pty::{native_pty_system, Child, ExitStatus, MasterPty, PtySize};
use smol::channel::Receiver as SmolReceiver;
use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
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
    pub transcript: String,
    pub started_at: String,
    pub updated_at: String,
}

impl SessionSnapshot {
    fn new(profile: &ConnectionProfile) -> Self {
        let now = clock_label();
        Self {
            title: profile.name.clone(),
            subtitle: profile.host_label(),
            backend: ssh::backend_caption(profile.backend).to_string(),
            phase: SessionPhase::Connecting,
            status_line: format!("Connecting to {}", profile.destination()),
            transcript: String::new(),
            started_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug)]
enum SessionCommand {
    Input(String),
    Resize { cols: u16, rows: u16 },
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct TerminalSessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    snapshot: Arc<Mutex<SessionSnapshot>>,
}

impl TerminalSessionHandle {
    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn send_input(&self, input: impl Into<String>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(input.into()))
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
}

pub fn launch_session(profile: &ConnectionProfile) -> Result<TerminalSessionHandle> {
    let snapshot = Arc::new(Mutex::new(SessionSnapshot::new(profile)));
    let (command_tx, command_rx) = mpsc::channel();
    let profile = profile.clone();
    let state = Arc::clone(&snapshot);

    let session_parts = create_session_parts(&profile, Arc::clone(&state))?;
    let SessionParts {
        master,
        child,
        backend_guard,
        initial_size,
    } = session_parts;

    let mut reader = master.try_clone_reader().context("failed to clone PTY reader")?;
    let master = Arc::new(Mutex::new(master));

    let terminal_writer = SharedWriter {
        master: Arc::clone(&master),
    };

    let mut terminal = Terminal::new(
        terminal_size(initial_size),
        Arc::new(ShellXTerminalConfig { scrollback: 6_000 }),
        "ShellX",
        env!("CARGO_PKG_VERSION"),
        Box::new(terminal_writer),
    );

    let reader_state = Arc::clone(&state);
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    set_status(
                        &reader_state,
                        SessionPhase::Exited,
                        "Remote stream closed".to_string(),
                    );
                    break;
                }
                Ok(read) => {
                    terminal.advance_bytes(&buffer[..read]);
                    let text = decode_output(&buffer[..read]);
                    let mut snapshot = reader_state.lock().unwrap();
                    if snapshot.phase == SessionPhase::Connecting {
                        snapshot.phase = SessionPhase::Connected;
                        snapshot.status_line = format!(
                            "Terminal stream active · updated {}",
                            clock_label()
                        );
                    } else if snapshot.phase != SessionPhase::Error
                        && snapshot.phase != SessionPhase::Exited
                    {
                        snapshot.phase = SessionPhase::Connected;
                        snapshot.status_line = format!("Last output received at {}", clock_label());
                    }
                    append_transcript(&mut snapshot.transcript, &text);
                    snapshot.updated_at = clock_label();
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

    thread::spawn(move || {
        let mut killer = child.clone_killer();
        let mut child = child;
        let _guard = backend_guard;

        loop {
            match command_rx.recv_timeout(Duration::from_millis(150)) {
                Ok(SessionCommand::Input(mut input)) => {
                    if !input.ends_with('\n') && !input.ends_with('\r') {
                        input.push('\n');
                    }

                    if let Err(error) = write_to_session(&master, input.as_bytes()) {
                        set_status(&state, SessionPhase::Error, format!("Send failed: {error}"));
                        break;
                    }
                }
                Ok(SessionCommand::Resize { cols, rows }) => {
                    let size = PtySize {
                        cols,
                        rows,
                        pixel_width: cols.saturating_mul(8),
                        pixel_height: rows.saturating_mul(16),
                    };
                    if let Err(error) = master.lock().unwrap().resize(size) {
                        set_status(
                            &state,
                            SessionPhase::Attention,
                            format!("Resize request failed: {error}"),
                        );
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
                        &state,
                        SessionPhase::Exited,
                        format!("Session ended with {}", describe_exit_status(&status)),
                    );
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    set_status(
                        &state,
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

fn create_system_session(profile: &ConnectionProfile, initial_size: PtySize) -> Result<SessionParts> {
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

    let command = (!profile.remote_command.trim().is_empty()).then(|| profile.remote_command.trim().to_string());
    let pty_session = session.clone();
    let pty_state = Arc::clone(&state);
    let (parts_tx, parts_rx) = mpsc::sync_channel::<Result<(wezterm_ssh::SshPty, wezterm_ssh::SshChildProcess)>>(1);

    thread::spawn(move || {
        let result = smol::block_on(pty_session.request_pty(
            "xterm-256color",
            initial_size,
            command.as_deref(),
            None,
        ))
        .context("failed to request remote PTY from wezterm-ssh");

        if let Err(ref e) = result {
            set_status(&pty_state, SessionPhase::Error, format!("PTY request failed: {e:#}"));
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
                        format!("Host verification accepted automatically: {}", verification.message)
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
struct ShellXTerminalConfig {
    scrollback: usize,
}

impl TerminalConfiguration for ShellXTerminalConfig {
    fn scrollback_size(&self) -> usize {
        self.scrollback
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

#[derive(Clone)]
struct SharedWriter {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.master.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.master.lock().unwrap().flush()
    }
}

fn write_to_session(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>, bytes: &[u8]) -> Result<()> {
    let mut master = master.lock().unwrap();
    master.write_all(bytes)?;
    master.flush()?;
    Ok(())
}

fn decode_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn append_transcript(transcript: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }

    transcript.push_str(chunk);
    let mut lines = transcript.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        transcript.clear();
    } else {
        let start = lines.len().saturating_sub(400);
        *transcript = lines[start..].join("\n");
    }
}

fn set_status(state: &Arc<Mutex<SessionSnapshot>>, phase: SessionPhase, status_line: String) {
    let mut snapshot = state.lock().unwrap();
    snapshot.phase = phase;
    snapshot.status_line = status_line;
    snapshot.updated_at = clock_label();
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
