use crate::connection::{
    ConnectionBackend, ConnectionProfile, ConnectionRepository, ConnectionStore,
};
use crate::terminal::{launch_session, SessionPhase, TerminalSessionHandle};
use gtk::glib::{self};
use gtk::prelude::*;
use relm4::prelude::*;
use std::cell::Cell;
use std::collections::BTreeMap;
use uuid::Uuid;

pub struct ShellXApp {
    repository: ConnectionRepository,
    store: ConnectionStore,
    selected_connection_id: Option<Uuid>,
    draft: ConnectionDraft,
    sessions: Vec<SessionTab>,
    selected_tab: Option<usize>,
    toast: String,
    connections_dirty: Cell<bool>,
    tabs_dirty: Cell<bool>,
    draft_dirty: Cell<bool>,
}

#[derive(Debug)]
pub enum AppMsg {
    SelectConnection(Uuid),
    NewConnection,
    SaveDraft,
    DeleteSelected,
    LaunchSelected,
    DraftNameChanged(String),
    DraftFolderChanged(String),
    DraftHostChanged(String),
    DraftPortChanged(u16),
    DraftUserChanged(String),
    DraftPasswordChanged(String),
    DraftIdentityChanged(String),
    DraftCommandChanged(String),
    DraftNoteChanged(String),
    DraftBackendChanged(ConnectionBackend),
    DraftAcceptNewHostChanged(bool),
    SelectTab(usize),
    CloseTab(usize),
    SendSelectedInput,
    SelectedInputChanged(String),
    RefreshSessions,
    ShutdownAll,
}

#[derive(Default, Clone)]
struct ConnectionDraft {
    id: Option<Uuid>,
    name: String,
    folder: String,
    host: String,
    port: u16,
    user: String,
    password: String,
    identity_file: String,
    remote_command: String,
    note: String,
    backend: ConnectionBackend,
    accept_new_host: bool,
    terminal_input: String,
}

impl ConnectionDraft {
    fn from_profile(store: &ConnectionStore, profile: &ConnectionProfile) -> Self {
        Self {
            id: Some(profile.id),
            name: profile.name.clone(),
            folder: store
                .folder_name(profile.folder_id)
                .unwrap_or_default()
                .to_string(),
            host: profile.host.clone(),
            port: profile.port,
            user: profile.user.clone(),
            password: profile.password.clone(),
            identity_file: profile.identity_file.clone(),
            remote_command: profile.remote_command.clone(),
            note: profile.note.clone(),
            backend: profile.backend,
            accept_new_host: profile.accept_new_host,
            terminal_input: String::new(),
        }
    }

    fn empty() -> Self {
        Self {
            port: 22,
            backend: ConnectionBackend::SystemOpenSsh,
            accept_new_host: true,
            ..Default::default()
        }
    }

    fn into_profile(self, store: &mut ConnectionStore) -> ConnectionProfile {
        let folder_id = store.ensure_folder_named(&self.folder);

        let mut profile = ConnectionProfile::new(
            if self.name.trim().is_empty() {
                "New connection"
            } else {
                self.name.trim()
            },
            self.host.trim(),
        );
        profile.id = self.id.unwrap_or_else(Uuid::new_v4);
        profile.folder_id = folder_id;
        profile.port = self.port;
        profile.user = self.user;
        profile.password = self.password;
        profile.identity_file = self.identity_file;
        profile.remote_command = self.remote_command;
        profile.note = self.note;
        profile.backend = self.backend;
        profile.accept_new_host = self.accept_new_host;
        profile
    }
}

struct SessionTab {
    connection_name: String,
    handle: TerminalSessionHandle,
}

pub struct AppWidgets {
    connection_list: gtk::ListBox,
    draft_name: gtk::Entry,
    draft_folder: gtk::Entry,
    draft_host: gtk::Entry,
    draft_port: gtk::SpinButton,
    draft_user: gtk::Entry,
    draft_password: gtk::PasswordEntry,
    draft_identity: gtk::Entry,
    draft_command: gtk::Entry,
    draft_note: gtk::TextView,
    accept_new_host: gtk::CheckButton,
    backend_system: gtk::CheckButton,
    backend_wezterm: gtk::CheckButton,
    toast_label: gtk::Label,
    stat_total: gtk::Label,
    stat_live: gtk::Label,
    stat_backend: gtk::Label,
    tab_list: gtk::ListBox,
    session_title: gtk::Label,
    session_subtitle: gtk::Label,
    session_phase: gtk::Label,
    session_status: gtk::Label,
    session_body: gtk::TextView,
    input_entry: gtk::Entry,
    launch_button: gtk::Button,
}

impl ShellXApp {
    fn selected_profile(&self) -> Option<&ConnectionProfile> {
        self.selected_connection_id
            .and_then(|id| self.store.connection(id))
    }

    fn load_draft_from_selection(&mut self) {
        if let Some(profile) = self.selected_profile() {
            self.draft = ConnectionDraft::from_profile(&self.store, profile);
        }
    }

    fn save_store(&mut self) {
        match self.repository.save(&self.store) {
            Ok(()) => {
                self.toast = format!("Saved {} connections", self.store.connections.len());
            }
            Err(error) => {
                self.toast = format!("Save failed: {error:#}");
            }
        }
    }

    fn open_selected_connection(&mut self) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.toast = "Select a connection first".into();
            return;
        };

        match launch_session(&profile) {
            Ok(handle) => {
                self.sessions.push(SessionTab {
                    connection_name: profile.name.clone(),
                    handle,
                });
                self.selected_tab = Some(self.sessions.len().saturating_sub(1));
                self.toast = format!("Opened terminal for {}", profile.name);
            }
            Err(error) => {
                self.toast = format!("Failed to open {}: {error:#}", profile.name);
            }
        }
    }

    fn selected_session(&self) -> Option<&SessionTab> {
        self.selected_tab.and_then(|index| self.sessions.get(index))
    }

    fn live_session_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|tab| {
                matches!(
                    tab.handle.snapshot().phase,
                    SessionPhase::Connecting | SessionPhase::Connected | SessionPhase::Attention
                )
            })
            .count()
    }
}

impl SimpleComponent for ShellXApp {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Root = gtk::ApplicationWindow;
    type Widgets = AppWidgets;

    fn init_root() -> Self::Root {
        gtk::ApplicationWindow::builder()
            .title("ShellX")
            .default_width(1480)
            .default_height(920)
            .build()
    }

    fn init(
        _init: Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        crate::theme::apply_global_css();

        let repository = ConnectionRepository::default();
        let mut store = repository.load().unwrap_or_default();
        if store.connections.is_empty() {
            let mut sample = ConnectionProfile::new("Demo host", "127.0.0.1");
            sample.note = "Preloaded sample profile for quick smoke tests".into();
            store.upsert(sample.clone());
            let _ = repository.save(&store);
        }

        let selected_connection_id = store.connections.first().map(|profile| profile.id);
        let draft = selected_connection_id
            .and_then(|id| store.connection(id))
            .map(|profile| ConnectionDraft::from_profile(&store, profile))
            .unwrap_or_else(ConnectionDraft::empty);

        let model = ShellXApp {
            repository,
            store,
            selected_connection_id,
            draft,
            sessions: Vec::new(),
            selected_tab: None,
            toast: "Ready".into(),
            connections_dirty: Cell::new(true),
            tabs_dirty: Cell::new(true),
            draft_dirty: Cell::new(true),
        };

        let root_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .build();
        root_box.add_css_class("shellx-root");

        let sidebar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .width_request(320)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(12)
            .build();
        sidebar.add_css_class("sidebar");
        sidebar.add_css_class("card");

        let sidebar_title = gtk::Label::new(Some("Connections"));
        sidebar_title.set_halign(gtk::Align::Start);
        sidebar_title.add_css_class("sidebar-title");

        let sidebar_caption = gtk::Label::new(Some("Folders, hosts, and launch targets"));
        sidebar_caption.set_halign(gtk::Align::Start);
        sidebar_caption.add_css_class("caption");

        let toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        let new_button = gtk::Button::with_label("New");
        new_button.add_css_class("pill-button");
        let save_button = gtk::Button::with_label("Save");
        save_button.add_css_class("pill-button");
        let remove_button = gtk::Button::with_label("Delete");
        remove_button.add_css_class("muted-button");
        toolbar.append(&new_button);
        toolbar.append(&save_button);
        toolbar.append(&remove_button);

        let connection_list = gtk::ListBox::new();
        connection_list.set_selection_mode(gtk::SelectionMode::Single);
        connection_list.add_css_class("connection-list");

        let connection_scroll = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&connection_list)
            .build();

        sidebar.append(&sidebar_title);
        sidebar.append(&sidebar_caption);
        sidebar.append(&toolbar);
        sidebar.append(&connection_scroll);

        let main_column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .hexpand(true)
            .vexpand(true)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(0)
            .margin_end(16)
            .build();

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();

        let hero_card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .hexpand(true)
            .margin_top(0)
            .margin_bottom(0)
            .margin_start(0)
            .margin_end(0)
            .build();
        hero_card.add_css_class("header-card");
        hero_card.add_css_class("card");
        hero_card.set_margin_top(8);
        hero_card.set_margin_bottom(8);
        hero_card.set_margin_start(8);
        hero_card.set_margin_end(8);

        let hero_title = gtk::Label::new(Some("ShellX · Cross-platform SSH terminal manager"));
        hero_title.set_halign(gtk::Align::Start);
        hero_title.add_css_class("terminal-title");

        let hero_caption = gtk::Label::new(Some(
            "Relm4 + GTK shell with WezTerm terminal core and dual SSH backends",
        ));
        hero_caption.set_halign(gtk::Align::Start);
        hero_caption.add_css_class("caption");
        hero_caption.set_wrap(true);

        let toast_label = gtk::Label::new(None);
        toast_label.set_halign(gtk::Align::Start);
        toast_label.add_css_class("status-chip");

        hero_card.append(&hero_title);
        hero_card.append(&hero_caption);
        hero_card.append(&toast_label);

        let stats_card = gtk::Grid::builder()
            .column_spacing(16)
            .row_spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        stats_card.add_css_class("status-card");
        stats_card.add_css_class("card");

        let stat_total_title = gtk::Label::new(Some("Saved"));
        stat_total_title.add_css_class("section-title");
        let stat_total = gtk::Label::new(None);
        stat_total.add_css_class("metric-value");
        let stat_live_title = gtk::Label::new(Some("Live tabs"));
        stat_live_title.add_css_class("section-title");
        let stat_live = gtk::Label::new(None);
        stat_live.add_css_class("metric-value");
        let stat_backend_title = gtk::Label::new(Some("Primary backend"));
        stat_backend_title.add_css_class("section-title");
        let stat_backend = gtk::Label::new(None);
        stat_backend.add_css_class("metric-value");

        stats_card.attach(&stat_total_title, 0, 0, 1, 1);
        stats_card.attach(&stat_total, 0, 1, 1, 1);
        stats_card.attach(&stat_live_title, 1, 0, 1, 1);
        stats_card.attach(&stat_live, 1, 1, 1, 1);
        stats_card.attach(&stat_backend_title, 2, 0, 1, 1);
        stats_card.attach(&stat_backend, 2, 1, 1, 1);

        header.append(&hero_card);
        header.append(&stats_card);

        let content = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .wide_handle(true)
            .position(520)
            .hexpand(true)
            .vexpand(true)
            .build();

        let editor_scroll = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        editor.add_css_class("card");
        editor.add_css_class("editor-group");
        editor_scroll.set_child(Some(&editor));

        let editor_title = gtk::Label::new(Some("Connection editor"));
        editor_title.set_halign(gtk::Align::Start);
        editor_title.add_css_class("section-title");
        editor.append(&editor_title);

        let draft_name = gtk::Entry::new();
        let draft_folder = gtk::Entry::new();
        let draft_host = gtk::Entry::new();
        let draft_port = gtk::SpinButton::with_range(1.0, 65535.0, 1.0);
        let draft_user = gtk::Entry::new();
        let draft_password = gtk::PasswordEntry::new();
        let draft_identity = gtk::Entry::new();
        let draft_command = gtk::Entry::new();
        let draft_note = gtk::TextView::new();
        draft_note.set_wrap_mode(gtk::WrapMode::WordChar);
        draft_note.set_monospace(false);
        draft_note.set_vexpand(true);
        let note_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(120)
            .vexpand(true)
            .child(&draft_note)
            .build();

        let accept_new_host = gtk::CheckButton::with_label("Accept new host keys automatically");
        let backend_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        let backend_system = gtk::CheckButton::with_label("System OpenSSH");
        let backend_wezterm = gtk::CheckButton::with_label("WezTerm SSH");
        backend_wezterm.set_group(Some(&backend_system));
        backend_box.append(&backend_system);
        backend_box.append(&backend_wezterm);

        for (label, widget) in [
            ("Name", draft_name.upcast_ref::<gtk::Widget>()),
            ("Folder", draft_folder.upcast_ref::<gtk::Widget>()),
            ("Host", draft_host.upcast_ref::<gtk::Widget>()),
            ("Port", draft_port.upcast_ref::<gtk::Widget>()),
            ("User", draft_user.upcast_ref::<gtk::Widget>()),
            ("Password", draft_password.upcast_ref::<gtk::Widget>()),
            ("Identity file", draft_identity.upcast_ref::<gtk::Widget>()),
            ("Remote command", draft_command.upcast_ref::<gtk::Widget>()),
        ] {
            let label_widget = gtk::Label::new(Some(label));
            label_widget.set_halign(gtk::Align::Start);
            label_widget.add_css_class("caption");
            editor.append(&label_widget);
            editor.append(widget);
        }

        let backend_label = gtk::Label::new(Some("Backend"));
        backend_label.set_halign(gtk::Align::Start);
        backend_label.add_css_class("caption");
        editor.append(&backend_label);
        editor.append(&backend_box);
        editor.append(&accept_new_host);

        let note_label = gtk::Label::new(Some("Notes"));
        note_label.set_halign(gtk::Align::Start);
        note_label.add_css_class("caption");
        editor.append(&note_label);
        editor.append(&note_scroll);

        let terminal_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        terminal_panel.add_css_class("terminal-card");

        let terminal_toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        terminal_toolbar.add_css_class("terminal-toolbar");

        let session_title = gtk::Label::new(Some("No session selected"));
        session_title.set_halign(gtk::Align::Start);
        session_title.add_css_class("terminal-title");
        let session_subtitle = gtk::Label::new(Some("Launch a saved connection to start a tab"));
        session_subtitle.set_halign(gtk::Align::Start);
        session_subtitle.add_css_class("caption");
        let session_phase = gtk::Label::new(Some("Idle"));
        session_phase.add_css_class("status-indicator");
        let launch_button = gtk::Button::with_label("Launch selected");
        launch_button.add_css_class("primary-pill");
        terminal_toolbar.append(&session_title);
        terminal_toolbar.append(&session_phase);
        terminal_toolbar.append(&launch_button);

        let session_status = gtk::Label::new(None);
        session_status.set_halign(gtk::Align::Start);
        session_status.add_css_class("caption");

        let tab_list = gtk::ListBox::new();
        tab_list.set_selection_mode(gtk::SelectionMode::Single);
        tab_list.add_css_class("tab-strip");

        let session_body = gtk::TextView::new();
        session_body.set_editable(false);
        session_body.set_cursor_visible(false);
        session_body.set_monospace(true);
        session_body.set_wrap_mode(gtk::WrapMode::None);
        session_body.add_css_class("terminal-view");
        let session_scroll = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&session_body)
            .build();

        let input_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        let input_entry = gtk::Entry::new();
        input_entry.set_placeholder_text(Some("Type a command and press Send"));
        input_entry.set_hexpand(true);
        let send_button = gtk::Button::with_label("Send");
        send_button.add_css_class("pill-button");
        input_bar.append(&input_entry);
        input_bar.append(&send_button);

        terminal_panel.append(&terminal_toolbar);
        terminal_panel.append(&session_subtitle);
        terminal_panel.append(&session_status);
        terminal_panel.append(&tab_list);
        terminal_panel.append(&session_scroll);
        terminal_panel.append(&input_bar);

        content.set_start_child(Some(&editor_scroll));
        content.set_end_child(Some(&terminal_panel));

        main_column.append(&header);
        main_column.append(&content);
        root_box.append(&sidebar);
        root_box.append(&main_column);
        window.set_child(Some(&root_box));

        {
            let sender = sender.clone();
            new_button.connect_clicked(move |_| {
                sender.input(AppMsg::NewConnection);
            });
        }
        {
            let sender = sender.clone();
            save_button.connect_clicked(move |_| {
                sender.input(AppMsg::SaveDraft);
            });
        }
        {
            let sender = sender.clone();
            remove_button.connect_clicked(move |_| {
                sender.input(AppMsg::DeleteSelected);
            });
        }
        {
            let sender = sender.clone();
            launch_button.connect_clicked(move |_| {
                sender.input(AppMsg::LaunchSelected);
            });
        }
        {
            let sender = sender.clone();
            send_button.connect_clicked(move |_| {
                sender.input(AppMsg::SendSelectedInput);
            });
        }

        {
            let sender = sender.clone();
            draft_name.connect_changed(move |entry| {
                sender.input(AppMsg::DraftNameChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_folder.connect_changed(move |entry| {
                sender.input(AppMsg::DraftFolderChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_host.connect_changed(move |entry| {
                sender.input(AppMsg::DraftHostChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_port.connect_value_changed(move |spin| {
                sender.input(AppMsg::DraftPortChanged(spin.value() as u16));
            });
        }
        {
            let sender = sender.clone();
            draft_user.connect_changed(move |entry| {
                sender.input(AppMsg::DraftUserChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_password.connect_changed(move |entry| {
                sender.input(AppMsg::DraftPasswordChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_identity.connect_changed(move |entry| {
                sender.input(AppMsg::DraftIdentityChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            draft_command.connect_changed(move |entry| {
                sender.input(AppMsg::DraftCommandChanged(entry.text().to_string()));
            });
        }
        {
            let sender = sender.clone();
            accept_new_host.connect_toggled(move |button| {
                sender.input(AppMsg::DraftAcceptNewHostChanged(button.is_active()));
            });
        }
        {
            let sender = sender.clone();
            backend_system.connect_toggled(move |button| {
                if button.is_active() {
                    sender.input(AppMsg::DraftBackendChanged(
                        ConnectionBackend::SystemOpenSsh,
                    ));
                }
            });
        }
        {
            let sender = sender.clone();
            backend_wezterm.connect_toggled(move |button| {
                if button.is_active() {
                    sender.input(AppMsg::DraftBackendChanged(ConnectionBackend::WezTermSsh));
                }
            });
        }
        {
            let sender = sender.clone();
            input_entry.connect_changed(move |entry| {
                sender.input(AppMsg::SelectedInputChanged(entry.text().to_string()));
            });
        }

        {
            let sender = sender.clone();
            input_entry.connect_activate(move |_| {
                sender.input(AppMsg::SendSelectedInput);
            });
        }

        let note_buffer = draft_note.buffer();
        {
            let sender = sender.clone();
            note_buffer.connect_changed(move |buffer| {
                let start = buffer.start_iter();
                let end = buffer.end_iter();
                sender.input(AppMsg::DraftNoteChanged(
                    buffer.text(&start, &end, true).to_string(),
                ));
            });
        }

        {
            let sender = sender.clone();
            connection_list.connect_row_selected(move |_list, row| {
                if let Some(row) = row {
                    if let Some(id) = row.tooltip_text() {
                        if let Ok(id) = Uuid::parse_str(id.as_str()) {
                            sender.input(AppMsg::SelectConnection(id));
                        }
                    }
                }
            });
        }

        {
            let sender = sender.clone();
            tab_list.connect_row_selected(move |_list, row| {
                if let Some(row) = row {
                    let index = row.index();
                    if index >= 0 {
                        sender.input(AppMsg::SelectTab(index as usize));
                    }
                }
            });
        }

        {
            let sender = sender.clone();
            window.connect_close_request(move |_| {
                sender.input(AppMsg::ShutdownAll);
                glib::Propagation::Proceed
            });
        }

        glib::timeout_add_local(std::time::Duration::from_millis(250), {
            let sender = sender.clone();
            move || {
                sender.input(AppMsg::RefreshSessions);
                glib::ControlFlow::Continue
            }
        });

        let widgets = AppWidgets {
            connection_list,
            draft_name,
            draft_folder,
            draft_host,
            draft_port,
            draft_user,
            draft_password,
            draft_identity,
            draft_command,
            draft_note,
            accept_new_host,
            backend_system,
            backend_wezterm,
            toast_label,
            stat_total,
            stat_live,
            stat_backend,
            tab_list,
            session_title,
            session_subtitle,
            session_phase,
            session_status,
            session_body,
            input_entry,
            launch_button,
        };

        let mut parts = ComponentParts { model, widgets };
        relm4::SimpleComponent::update_view(&parts.model, &mut parts.widgets, sender);
        parts
    }

    fn update(&mut self, message: Self::Input, _sender: ComponentSender<Self>) {
        match message {
            AppMsg::SelectConnection(id) => {
                self.selected_connection_id = Some(id);
                self.load_draft_from_selection();
                self.toast = "Connection loaded into editor".into();
                self.connections_dirty.set(true);
                self.draft_dirty.set(true);
            }
            AppMsg::NewConnection => {
                self.selected_connection_id = None;
                self.draft = ConnectionDraft::empty();
                self.toast = "Draft reset for a new connection".into();
                self.connections_dirty.set(true);
                self.draft_dirty.set(true);
            }
            AppMsg::SaveDraft => {
                let draft = std::mem::take(&mut self.draft);
                let profile = draft.into_profile(&mut self.store);
                self.selected_connection_id = Some(profile.id);
                self.store.upsert(profile.clone());
                self.draft = ConnectionDraft::from_profile(&self.store, &profile);
                self.save_store();
                self.connections_dirty.set(true);
                self.draft_dirty.set(true);
            }
            AppMsg::DeleteSelected => {
                if let Some(id) = self.selected_connection_id {
                    if let Some(removed) = self.store.remove(id) {
                        self.toast = format!("Deleted {}", removed.name);
                        self.selected_connection_id =
                            self.store.connections.first().map(|item| item.id);
                        if self.selected_connection_id.is_some() {
                            self.load_draft_from_selection();
                        } else {
                            self.draft = ConnectionDraft::empty();
                        }
                        self.save_store();
                        self.connections_dirty.set(true);
                        self.draft_dirty.set(true);
                    }
                }
            }
            AppMsg::LaunchSelected => {
                self.open_selected_connection();
                self.tabs_dirty.set(true);
            }
            AppMsg::DraftNameChanged(value) => self.draft.name = value,
            AppMsg::DraftFolderChanged(value) => self.draft.folder = value,
            AppMsg::DraftHostChanged(value) => self.draft.host = value,
            AppMsg::DraftPortChanged(value) => self.draft.port = value,
            AppMsg::DraftUserChanged(value) => self.draft.user = value,
            AppMsg::DraftPasswordChanged(value) => self.draft.password = value,
            AppMsg::DraftIdentityChanged(value) => self.draft.identity_file = value,
            AppMsg::DraftCommandChanged(value) => self.draft.remote_command = value,
            AppMsg::DraftNoteChanged(value) => self.draft.note = value,
            AppMsg::DraftBackendChanged(value) => self.draft.backend = value,
            AppMsg::DraftAcceptNewHostChanged(value) => self.draft.accept_new_host = value,
            AppMsg::SelectTab(index) => {
                if index < self.sessions.len() {
                    self.selected_tab = Some(index);
                    self.tabs_dirty.set(true);
                }
            }
            AppMsg::CloseTab(index) => {
                if index < self.sessions.len() {
                    self.sessions[index].handle.shutdown();
                    self.sessions.remove(index);
                    self.selected_tab = self.selected_tab.and_then(|selected| {
                        if self.sessions.is_empty() {
                            None
                        } else if selected > index {
                            Some(selected - 1)
                        } else if selected >= self.sessions.len() {
                            Some(self.sessions.len() - 1)
                        } else {
                            Some(selected)
                        }
                    });
                    self.tabs_dirty.set(true);
                }
            }
            AppMsg::SelectedInputChanged(value) => self.draft.terminal_input = value,
            AppMsg::SendSelectedInput => {
                let input = self.draft.terminal_input.trim().to_string();
                if input.is_empty() {
                    self.toast = "Type a command before sending".into();
                } else if let Some(session) = self.selected_session() {
                    match session.handle.send_input(input.clone()) {
                        Ok(()) => {
                            self.toast = format!("Sent `{input}`");
                            self.draft.terminal_input.clear();
                        }
                        Err(error) => {
                            self.toast = format!("Failed to send command: {error:#}");
                        }
                    }
                } else {
                    self.toast = "Launch a session first".into();
                }
            }
            AppMsg::RefreshSessions => {
                if let Some(selected) = self.selected_tab {
                    if selected >= self.sessions.len() {
                        self.selected_tab = (!self.sessions.is_empty()).then_some(0);
                        self.tabs_dirty.set(true);
                    }
                }
            }
            AppMsg::ShutdownAll => {
                for session in &self.sessions {
                    session.handle.shutdown();
                }
                self.sessions.clear();
                self.selected_tab = None;
                self.tabs_dirty.set(true);
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        widgets.toast_label.set_label(&self.toast);
        widgets
            .stat_total
            .set_label(&self.store.connections.len().to_string());
        widgets
            .stat_live
            .set_label(&self.live_session_count().to_string());
        widgets.stat_backend.set_label(self.draft.backend.label());
        widgets
            .launch_button
            .set_sensitive(self.selected_connection_id.is_some());

        if widgets.input_entry.text().as_str() != self.draft.terminal_input {
            widgets.input_entry.set_text(&self.draft.terminal_input);
        }

        if self.draft_dirty.get() {
            widgets.draft_name.set_text(&self.draft.name);
            widgets.draft_folder.set_text(&self.draft.folder);
            widgets.draft_host.set_text(&self.draft.host);
            widgets.draft_port.set_value(self.draft.port as f64);
            widgets.draft_user.set_text(&self.draft.user);
            widgets.draft_password.set_text(&self.draft.password);
            widgets.draft_identity.set_text(&self.draft.identity_file);
            widgets.draft_command.set_text(&self.draft.remote_command);
            widgets
                .accept_new_host
                .set_active(self.draft.accept_new_host);
            widgets.backend_system.set_active(matches!(
                self.draft.backend,
                ConnectionBackend::SystemOpenSsh
            ));
            widgets
                .backend_wezterm
                .set_active(matches!(self.draft.backend, ConnectionBackend::WezTermSsh));
            widgets.draft_note.buffer().set_text(&self.draft.note);
            self.draft_dirty.set(false);
        }

        if self.connections_dirty.get() {
            while let Some(row) = widgets.connection_list.row_at_index(0) {
                widgets.connection_list.remove(&row);
            }

            let grouped = self.store.sorted_connections().into_iter().fold(
                BTreeMap::<String, Vec<&ConnectionProfile>>::new(),
                |mut acc, connection| {
                    acc.entry(
                        self.store
                            .folder_name(connection.folder_id)
                            .unwrap_or("Ungrouped")
                            .to_string(),
                    )
                    .or_default()
                    .push(connection);
                    acc
                },
            );

            for (folder, connections) in grouped {
                let header = gtk::Label::new(Some(&folder));
                header.set_halign(gtk::Align::Start);
                header.add_css_class("section-title");
                widgets.connection_list.append(&header);

                for connection in connections {
                    let row = gtk::ListBoxRow::new();
                    row.set_tooltip_text(Some(&connection.id.to_string()));
                    let card = gtk::Box::builder()
                        .orientation(gtk::Orientation::Vertical)
                        .spacing(4)
                        .build();
                    card.add_css_class("connection-row");
                    let title = gtk::Label::new(Some(&connection.name));
                    title.set_halign(gtk::Align::Start);
                    title.add_css_class("connection-name");
                    let meta = gtk::Label::new(Some(&format!(
                        "{} · {}",
                        connection.host_label(),
                        connection.backend.label()
                    )));
                    meta.set_halign(gtk::Align::Start);
                    meta.add_css_class("connection-meta");
                    card.append(&title);
                    card.append(&meta);
                    row.set_child(Some(&card));
                    widgets.connection_list.append(&row);
                    if self.selected_connection_id == Some(connection.id) {
                        widgets.connection_list.select_row(Some(&row));
                    }
                }
            }
            self.connections_dirty.set(false);
        }

        if self.tabs_dirty.get() {
            while let Some(row) = widgets.tab_list.row_at_index(0) {
                widgets.tab_list.remove(&row);
            }

            for (index, session) in self.sessions.iter().enumerate() {
                let snapshot = session.handle.snapshot();
                let row = gtk::ListBoxRow::new();
                let tab = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(8)
                    .build();
                tab.add_css_class("tab-row");

                let title = gtk::Label::new(Some(&session.connection_name));
                title.set_halign(gtk::Align::Start);
                title.add_css_class("connection-name");
                let status = gtk::Label::new(Some(snapshot.phase.label()));
                status.add_css_class("status-indicator");
                status.add_css_class(match snapshot.phase {
                    SessionPhase::Connected => "connected",
                    SessionPhase::Error | SessionPhase::Exited => "error",
                    _ => "",
                });
                tab.append(&title);
                tab.append(&status);
                row.set_child(Some(&tab));
                widgets.tab_list.append(&row);
                if self.selected_tab == Some(index) {
                    widgets.tab_list.select_row(Some(&row));
                }
            }
            self.tabs_dirty.set(false);
        }

        if let Some(session) = self.selected_session() {
            let snapshot = session.handle.snapshot();
            widgets.session_title.set_label(&snapshot.title);
            widgets.session_subtitle.set_label(&format!(
                "{} · {} · started {}",
                snapshot.subtitle, snapshot.backend, snapshot.started_at
            ));
            widgets.session_phase.set_label(snapshot.phase.label());
            widgets.session_phase.remove_css_class("connected");
            widgets.session_phase.remove_css_class("error");
            match snapshot.phase {
                SessionPhase::Connected => widgets.session_phase.add_css_class("connected"),
                SessionPhase::Error | SessionPhase::Exited => {
                    widgets.session_phase.add_css_class("error")
                }
                _ => {}
            }
            widgets.session_status.set_label(&snapshot.status_line);
            widgets.session_body.buffer().set_text(&snapshot.transcript);

            let buffer = widgets.session_body.buffer();
            let end_iter = buffer.end_iter();
            widgets
                .session_body
                .scroll_to_iter(&mut end_iter.clone(), 0.0, false, 0.0, 1.0);
        } else {
            widgets.session_title.set_label("No session selected");
            widgets
                .session_subtitle
                .set_label("Launch a connection to create a terminal tab");
            widgets.session_phase.set_label("Idle");
            widgets.session_status.set_label("No terminal activity yet");
            widgets
                .session_body
                .buffer()
                .set_text("ShellX uses wezterm-term to render remote output here.");
        }
    }
}
