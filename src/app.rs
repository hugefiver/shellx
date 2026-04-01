use crate::config::{
    AppTheme, BackspaceKeyMode, DeleteKeyMode, GlobalConfig, ResolvedTerminalSettings,
    SettingsRepository, TerminalSettings, TERMINAL_TYPES,
};
use crate::connection::{
    ConnectionBackend, ConnectionProfile, ConnectionRepository, ConnectionStore,
};
use crate::terminal::{launch_local_session, launch_session, SessionPhase, TerminalSessionHandle};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use relm4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::collections::BTreeMap;
use std::thread;
use uuid::Uuid;
use std::collections::HashMap;
use std::sync::Arc;
use wezterm_surface::{CursorShape, CursorVisibility};
use wezterm_term::color::SrgbaTuple;
use wezterm_term::image::{ImageData, ImageDataType};
use wezterm_term::{Intensity, KeyCode, KeyModifiers, MouseButton, MouseEvent as WezMouseEvent, MouseEventKind};

#[derive(Clone, Copy, PartialEq)]
enum SplitLayout {
    Single,
    HSplit,
    VSplit,
    TopBottom3,
    Grid,
}

struct TerminalPane {
    name: String,
    handle: TerminalSessionHandle,
    remote_host: Option<String>,
}

struct TerminalGroup {
    layout: SplitLayout,
    panes: Vec<TerminalPane>,
    active_pane: usize,
}

impl TerminalGroup {
    fn can_split(&self) -> bool {
        self.panes.len() < 4
    }

    fn add_pane(&mut self, pane: TerminalPane, horizontal: bool) {
        self.panes.push(pane);
        self.layout = match self.panes.len() {
            1 => SplitLayout::Single,
            2 => {
                if horizontal {
                    SplitLayout::HSplit
                } else {
                    SplitLayout::VSplit
                }
            }
            3 => SplitLayout::TopBottom3,
            _ => SplitLayout::Grid,
        };
        self.active_pane = self.panes.len() - 1;
    }

    fn remove_pane(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        self.panes[index].handle.shutdown();
        self.panes.remove(index);
        self.layout = match self.panes.len() {
            0 | 1 => SplitLayout::Single,
            2 => match self.layout {
                SplitLayout::VSplit => SplitLayout::VSplit,
                _ => SplitLayout::HSplit,
            },
            3 => SplitLayout::TopBottom3,
            _ => SplitLayout::Grid,
        };
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len().saturating_sub(1);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CellCoord {
    col: usize,
    row: usize,
}

struct PaneRenderState {
    cell_width: f64,
    cell_height: f64,
    selection: Option<(CellCoord, CellCoord)>,
    image_cache: HashMap<[u8; 32], gtk::cairo::ImageSurface>,
}

impl Default for PaneRenderState {
    fn default() -> Self {
        Self {
            cell_width: 0.0,
            cell_height: 0.0,
            selection: None,
            image_cache: HashMap::new(),
        }
    }
}

impl PaneRenderState {
    fn normalized_selection(&self) -> Option<(CellCoord, CellCoord)> {
        self.selection.map(|(a, b)| {
            if a.row < b.row || (a.row == b.row && a.col <= b.col) {
                (a, b)
            } else {
                (b, a)
            }
        })
    }

    fn pixel_to_cell(&self, x: f64, y: f64) -> CellCoord {
        CellCoord {
            col: if self.cell_width > 0.0 {
                (x / self.cell_width).max(0.0) as usize
            } else {
                0
            },
            row: if self.cell_height > 0.0 {
                (y / self.cell_height).max(0.0) as usize
            } else {
                0
            },
        }
    }
}

struct TerminalSettingsWidgets {
    terminal_type: gtk::DropDown,
    cols: gtk::SpinButton,
    rows: gtk::SpinButton,
    scrollback: gtk::SpinButton,
    delete_key: [gtk::CheckButton; 3],
    backspace_key: [gtk::CheckButton; 3],
    left_alt_meta: gtk::CheckButton,
    right_alt_meta: gtk::CheckButton,
    csi_u: gtk::CheckButton,
    kitty_keyboard: gtk::CheckButton,
    kitty_graphics: gtk::CheckButton,
    mouse_reporting: gtk::CheckButton,
    scroll_output: gtk::CheckButton,
    scroll_keypress: gtk::CheckButton,
    answerback: gtk::Entry,
}

fn build_terminal_settings_notebook() -> (gtk::Notebook, TerminalSettingsWidgets) {
    let general = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();

    let term_type_list = gtk::StringList::new(TERMINAL_TYPES);
    let terminal_type = gtk::DropDown::new(Some(term_type_list), gtk::Expression::NONE);
    terminal_type.set_hexpand(true);
    let cols = gtk::SpinButton::with_range(1.0, 999.0, 1.0);
    cols.set_width_chars(5);
    let rows = gtk::SpinButton::with_range(1.0, 999.0, 1.0);
    rows.set_width_chars(5);
    let scrollback = gtk::SpinButton::with_range(0.0, 1_000_000.0, 100.0);
    scrollback.set_width_chars(8);
    scrollback.set_hexpand(true);

    let mut row = 0i32;
    let lbl = gtk::Label::new(Some("Terminal Type"));
    lbl.set_halign(gtk::Align::End);
    general.attach(&lbl, 0, row, 1, 1);
    general.attach(&terminal_type, 1, row, 3, 1);
    row += 1;
    let lbl = gtk::Label::new(Some("Columns"));
    lbl.set_halign(gtk::Align::End);
    general.attach(&lbl, 0, row, 1, 1);
    general.attach(&cols, 1, row, 1, 1);
    let lbl = gtk::Label::new(Some("Rows"));
    lbl.set_halign(gtk::Align::End);
    general.attach(&lbl, 2, row, 1, 1);
    general.attach(&rows, 3, row, 1, 1);
    row += 1;
    let lbl = gtk::Label::new(Some("Scrollback"));
    lbl.set_halign(gtk::Align::End);
    general.attach(&lbl, 0, row, 1, 1);
    general.attach(&scrollback, 1, row, 3, 1);

    let keyboard = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();

    row = 0;
    let lbl = gtk::Label::new(Some("DELETE key"));
    lbl.set_halign(gtk::Align::End);
    lbl.set_valign(gtk::Align::Start);
    keyboard.attach(&lbl, 0, row, 1, 1);
    let del_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    let delete_key: [gtk::CheckButton; 3] = std::array::from_fn(|i| {
        let btn = gtk::CheckButton::with_label(DeleteKeyMode::ALL[i].label());
        if i > 0 {
            btn.set_group(Some(
                del_box
                    .first_child()
                    .unwrap()
                    .downcast_ref::<gtk::CheckButton>()
                    .unwrap(),
            ));
        }
        del_box.append(&btn);
        btn
    });
    keyboard.attach(&del_box, 1, row, 3, 1);
    row += 1;

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.set_margin_top(2);
    sep.set_margin_bottom(2);
    keyboard.attach(&sep, 0, row, 4, 1);
    row += 1;

    let lbl = gtk::Label::new(Some("BACKSPACE key"));
    lbl.set_halign(gtk::Align::End);
    lbl.set_valign(gtk::Align::Start);
    keyboard.attach(&lbl, 0, row, 1, 1);
    let bs_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    let backspace_key: [gtk::CheckButton; 3] = std::array::from_fn(|i| {
        let btn = gtk::CheckButton::with_label(BackspaceKeyMode::ALL[i].label());
        if i > 0 {
            btn.set_group(Some(
                bs_box
                    .first_child()
                    .unwrap()
                    .downcast_ref::<gtk::CheckButton>()
                    .unwrap(),
            ));
        }
        bs_box.append(&btn);
        btn
    });
    keyboard.attach(&bs_box, 1, row, 3, 1);
    row += 1;

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.set_margin_top(2);
    sep.set_margin_bottom(2);
    keyboard.attach(&sep, 0, row, 4, 1);
    row += 1;

    let lbl = gtk::Label::new(Some("Meta key"));
    lbl.set_halign(gtk::Align::End);
    lbl.set_valign(gtk::Align::Start);
    keyboard.attach(&lbl, 0, row, 1, 1);
    let meta_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    let left_alt_meta = gtk::CheckButton::with_label("Left Alt as Meta");
    let right_alt_meta = gtk::CheckButton::with_label("Right Alt as Meta");
    meta_box.append(&left_alt_meta);
    meta_box.append(&right_alt_meta);
    keyboard.attach(&meta_box, 1, row, 3, 1);

    let advanced = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();

    let csi_u = gtk::CheckButton::with_label("CSI u Key Encoding");
    let kitty_keyboard = gtk::CheckButton::with_label("Kitty Keyboard Protocol");
    let kitty_graphics = gtk::CheckButton::with_label("Kitty Graphics Protocol");
    let mouse_reporting = gtk::CheckButton::with_label("Mouse Reporting");
    let scroll_output = gtk::CheckButton::with_label("Scroll on Output");
    let scroll_keypress = gtk::CheckButton::with_label("Scroll on Keypress");
    let answerback = gtk::Entry::new();
    answerback.set_placeholder_text(Some("rsHell"));
    answerback.set_hexpand(true);

    row = 0;
    for cb in [
        &csi_u,
        &kitty_keyboard,
        &kitty_graphics,
        &mouse_reporting,
        &scroll_output,
        &scroll_keypress,
    ] {
        advanced.attach(cb, 0, row, 4, 1);
        row += 1;
    }
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.set_margin_top(2);
    sep.set_margin_bottom(2);
    advanced.attach(&sep, 0, row, 4, 1);
    row += 1;
    let lbl = gtk::Label::new(Some("Answerback"));
    lbl.set_halign(gtk::Align::End);
    advanced.attach(&lbl, 0, row, 1, 1);
    advanced.attach(&answerback, 1, row, 3, 1);

    let notebook = gtk::Notebook::new();
    notebook.set_tab_pos(gtk::PositionType::Top);
    notebook.append_page(&general, Some(&gtk::Label::new(Some("General"))));
    notebook.append_page(&keyboard, Some(&gtk::Label::new(Some("Keyboard"))));
    notebook.append_page(&advanced, Some(&gtk::Label::new(Some("Advanced"))));

    let widgets = TerminalSettingsWidgets {
        terminal_type,
        cols,
        rows,
        scrollback,
        delete_key,
        backspace_key,
        left_alt_meta,
        right_alt_meta,
        csi_u,
        kitty_keyboard,
        kitty_graphics,
        mouse_reporting,
        scroll_output,
        scroll_keypress,
        answerback,
    };

    (notebook, widgets)
}

fn connect_terminal_settings_signals<F>(
    w: &TerminalSettingsWidgets,
    sender: &ComponentSender<RshellApp>,
    wrap: F,
) where
    F: Fn(TerminalSettingChange) -> AppMsg + Clone + 'static,
{
    {
        let s = sender.clone();
        let f = wrap.clone();
        w.terminal_type.connect_selected_notify(move |dd| {
            let idx = dd.selected();
            if idx != gtk::INVALID_LIST_POSITION
                && let Some(name) = TERMINAL_TYPES.get(idx as usize)
            {
                s.input(f(TerminalSettingChange::TerminalType(name.to_string())));
            }
        });
    }
    {
        let s = sender.clone();
        let f = wrap.clone();
        w.cols.connect_value_changed(move |e| {
            s.input(f(TerminalSettingChange::Cols(e.value() as u16)));
        });
    }
    {
        let s = sender.clone();
        let f = wrap.clone();
        w.rows.connect_value_changed(move |e| {
            s.input(f(TerminalSettingChange::Rows(e.value() as u16)));
        });
    }
    {
        let s = sender.clone();
        let f = wrap.clone();
        w.scrollback.connect_value_changed(move |e| {
            s.input(f(TerminalSettingChange::ScrollbackLines(e.value() as usize)));
        });
    }
    for (i, btn) in w.delete_key.iter().enumerate() {
        let s = sender.clone();
        let f = wrap.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                s.input(f(TerminalSettingChange::DeleteKey(DeleteKeyMode::ALL[i])));
            }
        });
    }
    for (i, btn) in w.backspace_key.iter().enumerate() {
        let s = sender.clone();
        let f = wrap.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                s.input(f(TerminalSettingChange::BackspaceKey(
                    BackspaceKeyMode::ALL[i],
                )));
            }
        });
    }
    for (cb, change_fn) in [
        (
            &w.left_alt_meta,
            TerminalSettingChange::LeftAltMeta as fn(bool) -> TerminalSettingChange,
        ),
        (&w.right_alt_meta, TerminalSettingChange::RightAltMeta),
        (&w.csi_u, TerminalSettingChange::CsiU),
        (&w.kitty_keyboard, TerminalSettingChange::KittyKeyboard),
        (&w.kitty_graphics, TerminalSettingChange::KittyGraphics),
        (&w.mouse_reporting, TerminalSettingChange::MouseReporting),
        (&w.scroll_output, TerminalSettingChange::ScrollOnOutput),
        (&w.scroll_keypress, TerminalSettingChange::ScrollOnKeypress),
    ] {
        let s = sender.clone();
        let f = wrap.clone();
        cb.connect_toggled(move |b| {
            s.input(f(change_fn(b.is_active())));
        });
    }
    {
        let s = sender.clone();
        let f = wrap.clone();
        w.answerback.connect_changed(move |e| {
            s.input(f(TerminalSettingChange::Answerback(e.text().to_string())));
        });
    }
}

fn populate_terminal_settings(w: &TerminalSettingsWidgets, resolved: &ResolvedTerminalSettings) {
    let type_idx = TERMINAL_TYPES
        .iter()
        .position(|t| *t == resolved.terminal_type)
        .unwrap_or(0);
    w.terminal_type.set_selected(type_idx as u32);
    w.cols.set_value(resolved.initial_cols as f64);
    w.rows.set_value(resolved.initial_rows as f64);
    w.scrollback.set_value(resolved.scrollback_lines as f64);
    for (i, mode) in DeleteKeyMode::ALL.iter().enumerate() {
        w.delete_key[i].set_active(*mode == resolved.delete_key);
    }
    for (i, mode) in BackspaceKeyMode::ALL.iter().enumerate() {
        w.backspace_key[i].set_active(*mode == resolved.backspace_key);
    }
    w.left_alt_meta.set_active(resolved.left_alt_as_meta);
    w.right_alt_meta.set_active(resolved.right_alt_as_meta);
    w.csi_u.set_active(resolved.enable_csi_u);
    w.kitty_keyboard.set_active(resolved.enable_kitty_keyboard);
    w.kitty_graphics.set_active(resolved.enable_kitty_graphics);
    w.mouse_reporting.set_active(resolved.mouse_reporting);
    w.scroll_output.set_active(resolved.scroll_on_output);
    w.scroll_keypress.set_active(resolved.scroll_on_keypress);
    w.answerback.set_text(&resolved.answerback);
}

pub struct RshellApp {
    repository: ConnectionRepository,
    store: ConnectionStore,
    settings_repo: SettingsRepository,
    global_config: GlobalConfig,
    selected_connection_id: Option<Uuid>,
    draft: ConnectionDraft,
    groups: Vec<TerminalGroup>,
    selected_group: Option<usize>,
    toast: String,
    sidebar_visible: bool,
    editor_visible: bool,
    settings_visible: bool,
    broadcast_mode: bool,
    connections_dirty: Cell<bool>,
    groups_dirty: Cell<bool>,
    terminal_dirty: Cell<bool>,
    draft_dirty: Cell<bool>,
    updating_draft: Cell<bool>,
    settings_dirty: Cell<bool>,
    updating_settings: Cell<bool>,
}

#[derive(Debug)]
pub enum AppMsg {
    SelectConnection(Uuid),
    NewConnection,
    SaveDraft,
    DeleteSelected,
    ToggleSidebar,
    ToggleEditor,
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
    LaunchSelected,
    NewLocalTab,
    SelectGroup(usize),
    CloseGroup(usize),
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    FocusPane(usize),
    PaneKeyPress(usize, gdk::Key, gdk::ModifierType),
    SessionLaunched(String, TerminalSessionHandle, Option<String>),
    SessionFailed(String),
    RefreshSessions,
    ShutdownAll,
    ToggleFullscreen,
    ToggleBroadcast,
    PasteRemoteIp(usize),
    OpenInEditor(usize),
    DraftTerminalChanged(TerminalSettingChange),
    GlobalTerminalChanged(TerminalSettingChange),
    ThemeChanged(AppTheme),
    OpenGlobalSettings,
    SaveGlobalSettings,
}

#[derive(Debug)]
pub enum TerminalSettingChange {
    TerminalType(String),
    Cols(u16),
    Rows(u16),
    ScrollbackLines(usize),
    DeleteKey(DeleteKeyMode),
    BackspaceKey(BackspaceKeyMode),
    LeftAltMeta(bool),
    RightAltMeta(bool),
    CsiU(bool),
    KittyKeyboard(bool),
    KittyGraphics(bool),
    MouseReporting(bool),
    ScrollOnOutput(bool),
    ScrollOnKeypress(bool),
    Answerback(String),
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
    terminal: TerminalSettings,
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
            terminal: profile.terminal.clone(),
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
        profile.terminal = self.terminal;
        profile
    }
}

pub struct AppWidgets {
    sidebar_revealer: gtk::Revealer,
    connection_list: gtk::ListBox,
    editor_dialog: gtk::Window,
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
    draft_terminal: TerminalSettingsWidgets,
    settings_dialog: gtk::Window,
    global_terminal: TerminalSettingsWidgets,
    connect_btn: gtk::Button,
    split_h_btn: gtk::Button,
    split_v_btn: gtk::Button,
    close_pane_btn: gtk::Button,
    tab_bar: gtk::Box,
    terminal_container: gtk::Box,
    pane_views: Vec<gtk::DrawingArea>,
    pane_sizes: Vec<(u16, u16)>,
    status_label: gtk::Label,
    toast_label: gtk::Label,
}

impl RshellApp {
    fn resolve_settings(&self, session: &TerminalSettings) -> ResolvedTerminalSettings {
        session.merge_over(&self.global_config.terminal).resolve()
    }

    fn default_resolved_settings(&self) -> ResolvedTerminalSettings {
        self.global_config.terminal.resolve()
    }

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
            Err(e) => {
                self.toast = format!("Save failed: {e:#}");
            }
        }
    }

    fn selected_group(&self) -> Option<&TerminalGroup> {
        self.selected_group.and_then(|i| self.groups.get(i))
    }

    fn selected_group_mut(&mut self) -> Option<&mut TerminalGroup> {
        self.selected_group.and_then(|i| self.groups.get_mut(i))
    }

    fn live_count(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|g| &g.panes)
            .filter(|p| {
                matches!(
                    p.handle.snapshot().phase,
                    SessionPhase::Connecting | SessionPhase::Connected | SessionPhase::Attention
                )
            })
            .count()
    }

    fn status_text(&self) -> String {
        if let Some(group) = self.selected_group()
            && let Some(pane) = group.panes.get(group.active_pane)
        {
            let snap = pane.handle.snapshot();
            return format!(
                "{}  ·  {}  ·  {}  ·  Panes: {}",
                snap.phase.label(),
                pane.name,
                snap.status_line,
                group.panes.len()
            );
        }
        format!(
            "Sessions: {}  ·  Live: {}",
            self.groups.len(),
            self.live_count()
        )
    }
}

impl RshellApp {
    fn update_impl(&mut self, message: AppMsg, sender: &ComponentSender<Self>) {
        match message {
            AppMsg::SelectConnection(id) => {
                self.selected_connection_id = Some(id);
                self.load_draft_from_selection();
                self.draft_dirty.set(true);
            }
            AppMsg::NewConnection => {
                self.selected_connection_id = None;
                self.draft = ConnectionDraft::empty();
                self.editor_visible = true;
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
                self.editor_visible = false;
                self.connections_dirty.set(true);
                self.draft_dirty.set(true);
            }
            AppMsg::DeleteSelected => {
                if let Some(id) = self.selected_connection_id
                    && let Some(removed) = self.store.remove(id)
                {
                    self.toast = format!("Deleted {}", removed.name);
                    self.selected_connection_id = self.store.connections.first().map(|p| p.id);
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
            AppMsg::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.terminal_dirty.set(true);
            }
            AppMsg::ToggleEditor => {
                if self.editor_visible {
                    self.editor_visible = false;
                } else {
                    self.load_draft_from_selection();
                    self.editor_visible = true;
                    self.draft_dirty.set(true);
                }
            }
            AppMsg::DraftNameChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.name = v;
                }
            }
            AppMsg::DraftFolderChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.folder = v;
                }
            }
            AppMsg::DraftHostChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.host = v;
                }
            }
            AppMsg::DraftPortChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.port = v;
                }
            }
            AppMsg::DraftUserChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.user = v;
                }
            }
            AppMsg::DraftPasswordChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.password = v;
                }
            }
            AppMsg::DraftIdentityChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.identity_file = v;
                }
            }
            AppMsg::DraftCommandChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.remote_command = v;
                }
            }
            AppMsg::DraftNoteChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.note = v;
                }
            }
            AppMsg::DraftBackendChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.backend = v;
                }
            }
            AppMsg::DraftAcceptNewHostChanged(v) => {
                if !self.updating_draft.get() {
                    self.draft.accept_new_host = v;
                }
            }
            AppMsg::LaunchSelected => {
                let Some(profile) = self.selected_profile().cloned() else {
                    self.toast = "Select a connection first".into();
                    return;
                };
                self.toast = format!("Connecting to {}...", profile.name);
                let name = profile.name.clone();
                let host = profile.host.clone();
                let input_tx = sender.input_sender().clone();
                let settings = self.resolve_settings(&profile.terminal);
                thread::spawn(move || match launch_session(&profile, settings) {
                    Ok(handle) => {
                        let _ =
                            input_tx.send(AppMsg::SessionLaunched(name, handle, Some(host)));
                    }
                    Err(e) => {
                        let _ = input_tx.send(AppMsg::SessionFailed(format!("{name}: {e:#}")));
                    }
                });
            }
            AppMsg::NewLocalTab => match launch_local_session(self.default_resolved_settings()) {
                Ok(handle) => {
                    let mut group = TerminalGroup {
                        layout: SplitLayout::Single,
                        panes: Vec::new(),
                        active_pane: 0,
                    };
                    group.panes.push(TerminalPane {
                        name: "Local Shell".into(),
                        handle,
                        remote_host: None,
                    });
                    self.groups.push(group);
                    self.selected_group = Some(self.groups.len() - 1);
                    self.groups_dirty.set(true);
                    self.terminal_dirty.set(true);
                }
                Err(e) => {
                    self.toast = format!("Local shell failed: {e:#}");
                }
            },
            AppMsg::SelectGroup(index) => {
                if index < self.groups.len() {
                    self.selected_group = Some(index);
                    self.terminal_dirty.set(true);
                }
            }
            AppMsg::CloseGroup(index) => {
                if index < self.groups.len() {
                    let group = &self.groups[index];
                    for pane in &group.panes {
                        pane.handle.shutdown();
                    }
                    self.groups.remove(index);
                    self.selected_group = if self.groups.is_empty() {
                        None
                    } else if let Some(sel) = self.selected_group {
                        if sel > index {
                            Some(sel - 1)
                        } else if sel >= self.groups.len() {
                            Some(self.groups.len() - 1)
                        } else {
                            Some(sel)
                        }
                    } else {
                        None
                    };
                    self.groups_dirty.set(true);
                    self.terminal_dirty.set(true);
                }
            }
            AppMsg::SplitHorizontal => {
                let settings = self.default_resolved_settings();
                if let Some(group) = self.selected_group_mut() {
                    if group.can_split() {
                        match launch_local_session(settings) {
                            Ok(handle) => {
                                group.add_pane(
                                    TerminalPane {
                                        name: "Local Shell".into(),
                                        handle,
                                        remote_host: None,
                                    },
                                    true,
                                );
                                self.terminal_dirty.set(true);
                            }
                            Err(e) => {
                                self.toast = format!("Split failed: {e:#}");
                            }
                        }
                    } else {
                        self.toast = "Max 4 panes per tab".into();
                    }
                }
            }
            AppMsg::SplitVertical => {
                let settings = self.default_resolved_settings();
                if let Some(group) = self.selected_group_mut() {
                    if group.can_split() {
                        match launch_local_session(settings) {
                            Ok(handle) => {
                                group.add_pane(
                                    TerminalPane {
                                        name: "Local Shell".into(),
                                        handle,
                                        remote_host: None,
                                    },
                                    false,
                                );
                                self.terminal_dirty.set(true);
                            }
                            Err(e) => {
                                self.toast = format!("Split failed: {e:#}");
                            }
                        }
                    } else {
                        self.toast = "Max 4 panes per tab".into();
                    }
                }
            }
            AppMsg::ClosePane => {
                let should_remove_group = if let Some(group) = self.selected_group_mut() {
                    if group.panes.len() > 1 {
                        let idx = group.active_pane;
                        group.remove_pane(idx);
                        self.terminal_dirty.set(true);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                };
                if should_remove_group
                    && let Some(gi) = self.selected_group
                {
                    self.update_impl(AppMsg::CloseGroup(gi), sender);
                }
            }
            AppMsg::FocusPane(index) => {
                if let Some(group) = self.selected_group_mut()
                    && index < group.panes.len()
                {
                    group.active_pane = index;
                }
            }
            AppMsg::PaneKeyPress(pane_index, key, modifiers) => {
                if let Some((keycode, keymods)) = gdk_key_to_wezterm(key, modifiers) {
                    if self.broadcast_mode {
                        for group in &self.groups {
                            for pane in &group.panes {
                        let _ = pane
                            .handle
                            .with_terminal_mut(|t| t.key_down(keycode, keymods));
                            }
                        }
                    } else if let Some(group) = self.selected_group()
                        && let Some(pane) = group.panes.get(pane_index)
                    {
                        let _ = pane.handle.with_terminal_mut(|t| t.key_down(keycode, keymods));
                    }
                }
            }
            AppMsg::RefreshSessions => {
                // No-op: exists solely to trigger view refresh via the 250ms timer.
                // Relm4 calls update_view() after each update(), so receiving this
                // message causes terminal content, status bar, etc. to refresh.
            }
            AppMsg::SessionLaunched(name, handle, remote_host) => {
                let mut group = TerminalGroup {
                    layout: SplitLayout::Single,
                    panes: Vec::new(),
                    active_pane: 0,
                };
                group.panes.push(TerminalPane {
                    name: name.clone(),
                    handle,
                    remote_host,
                });
                self.groups.push(group);
                self.selected_group = Some(self.groups.len() - 1);
                self.toast = format!("Connected to {name}");
                self.groups_dirty.set(true);
                self.terminal_dirty.set(true);
            }
            AppMsg::SessionFailed(msg) => {
                self.toast = format!("Failed: {msg}");
            }
            AppMsg::ShutdownAll => {
                for group in &self.groups {
                    for pane in &group.panes {
                        pane.handle.shutdown();
                    }
                }
                self.groups.clear();
                self.selected_group = None;
                self.groups_dirty.set(true);
                self.terminal_dirty.set(true);
            }
            AppMsg::ToggleFullscreen => {}
            AppMsg::ToggleBroadcast => {
                self.broadcast_mode = !self.broadcast_mode;
                self.toast = if self.broadcast_mode {
                    "Broadcast ON — keys sent to all sessions".into()
                } else {
                    "Broadcast OFF".into()
                };
            }
            AppMsg::PasteRemoteIp(pane_index) => {
                if let Some(group) = self.selected_group()
                    && let Some(pane) = group.panes.get(pane_index)
                {
                    if let Some(host) = &pane.remote_host {
                        let _ = pane.handle.send_bytes(host.as_bytes().to_vec());
                    } else {
                        self.toast = "No remote host for this session".into();
                    }
                }
            }
            AppMsg::OpenInEditor(pane_index) => {
                if let Some(group) = self.selected_group()
                    && let Some(pane) = group.panes.get(pane_index)
                {
                    let text = extract_screen_text(&pane.handle);
                    open_text_in_editor(&text);
                }
            }
            AppMsg::DraftTerminalChanged(change) => {
                if !self.updating_draft.get() {
                    let t = &mut self.draft.terminal;
                    match change {
                        TerminalSettingChange::TerminalType(v) => t.terminal_type = Some(v),
                        TerminalSettingChange::Cols(v) => t.initial_cols = Some(v),
                        TerminalSettingChange::Rows(v) => t.initial_rows = Some(v),
                        TerminalSettingChange::ScrollbackLines(v) => t.scrollback_lines = Some(v),
                        TerminalSettingChange::DeleteKey(v) => t.delete_key = Some(v),
                        TerminalSettingChange::BackspaceKey(v) => t.backspace_key = Some(v),
                        TerminalSettingChange::LeftAltMeta(v) => t.left_alt_as_meta = Some(v),
                        TerminalSettingChange::RightAltMeta(v) => t.right_alt_as_meta = Some(v),
                        TerminalSettingChange::CsiU(v) => t.enable_csi_u = Some(v),
                        TerminalSettingChange::KittyKeyboard(v) => t.enable_kitty_keyboard = Some(v),
                        TerminalSettingChange::KittyGraphics(v) => t.enable_kitty_graphics = Some(v),
                        TerminalSettingChange::MouseReporting(v) => t.mouse_reporting = Some(v),
                        TerminalSettingChange::ScrollOnOutput(v) => t.scroll_on_output = Some(v),
                        TerminalSettingChange::ScrollOnKeypress(v) => t.scroll_on_keypress = Some(v),
                        TerminalSettingChange::Answerback(v) => t.answerback = Some(v),
                    }
                }
            }
            AppMsg::GlobalTerminalChanged(change) => {
                if !self.updating_settings.get() {
                    let t = &mut self.global_config.terminal;
                    match change {
                        TerminalSettingChange::TerminalType(v) => t.terminal_type = Some(v),
                        TerminalSettingChange::Cols(v) => t.initial_cols = Some(v),
                        TerminalSettingChange::Rows(v) => t.initial_rows = Some(v),
                        TerminalSettingChange::ScrollbackLines(v) => t.scrollback_lines = Some(v),
                        TerminalSettingChange::DeleteKey(v) => t.delete_key = Some(v),
                        TerminalSettingChange::BackspaceKey(v) => t.backspace_key = Some(v),
                        TerminalSettingChange::LeftAltMeta(v) => t.left_alt_as_meta = Some(v),
                        TerminalSettingChange::RightAltMeta(v) => t.right_alt_as_meta = Some(v),
                        TerminalSettingChange::CsiU(v) => t.enable_csi_u = Some(v),
                        TerminalSettingChange::KittyKeyboard(v) => t.enable_kitty_keyboard = Some(v),
                        TerminalSettingChange::KittyGraphics(v) => t.enable_kitty_graphics = Some(v),
                        TerminalSettingChange::MouseReporting(v) => t.mouse_reporting = Some(v),
                        TerminalSettingChange::ScrollOnOutput(v) => t.scroll_on_output = Some(v),
                        TerminalSettingChange::ScrollOnKeypress(v) => t.scroll_on_keypress = Some(v),
                        TerminalSettingChange::Answerback(v) => t.answerback = Some(v),
                    }
                    self.settings_dirty.set(true);
                }
            }
            AppMsg::ThemeChanged(theme) => {
                self.global_config.theme = theme;
                crate::theme::apply_theme(theme);
                self.settings_dirty.set(true);
            }
            AppMsg::OpenGlobalSettings => {
                self.editor_visible = false;
                self.settings_visible = !self.settings_visible;
                self.settings_dirty.set(true);
            }
            AppMsg::SaveGlobalSettings => {
                if let Err(e) = self.settings_repo.save(&self.global_config) {
                    self.toast = format!("Failed to save settings: {e}");
                } else {
                    self.toast = "Settings saved".into();
                }
            }
        }
    }

    fn view_impl(&self, widgets: &mut AppWidgets, sender: ComponentSender<Self>) {
        widgets.toast_label.set_label(&self.toast);
        widgets
            .sidebar_revealer
            .set_visible(self.sidebar_visible);
        if self.editor_visible {
            widgets.editor_dialog.present();
        } else {
            widgets.editor_dialog.set_visible(false);
        }

        widgets
            .connect_btn
            .set_sensitive(self.selected_connection_id.is_some());
        let has_group = self.selected_group.is_some();
        let can_split = self.selected_group().is_some_and(|g| g.can_split());
        widgets.split_h_btn.set_sensitive(has_group && can_split);
        widgets.split_v_btn.set_sensitive(has_group && can_split);
        widgets.close_pane_btn.set_sensitive(has_group);

        widgets.status_label.set_label(&self.status_text());

        if self.draft_dirty.get() {
            self.updating_draft.set(true);
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

            let resolved = self.resolve_settings(&self.draft.terminal);
            populate_terminal_settings(&widgets.draft_terminal, &resolved);

            self.updating_draft.set(false);
            self.draft_dirty.set(false);
        }

        if self.settings_dirty.get() {
            if self.settings_visible {
                self.updating_settings.set(true);
                let resolved = self.default_resolved_settings();
                populate_terminal_settings(&widgets.global_terminal, &resolved);
                self.updating_settings.set(false);
                widgets.settings_dialog.present();
            } else {
                widgets.settings_dialog.set_visible(false);
            }
            self.settings_dirty.set(false);
        }

        if self.connections_dirty.get() {
            while let Some(row) = widgets.connection_list.row_at_index(0) {
                widgets.connection_list.remove(&row);
            }

            let grouped = self.store.sorted_connections().into_iter().fold(
                BTreeMap::<String, Vec<&ConnectionProfile>>::new(),
                |mut acc, conn| {
                    acc.entry(
                        self.store
                            .folder_name(conn.folder_id)
                            .unwrap_or("Ungrouped")
                            .to_string(),
                    )
                    .or_default()
                    .push(conn);
                    acc
                },
            );

            for (folder, connections) in grouped {
                let header = gtk::Label::new(Some(&folder));
                header.set_halign(gtk::Align::Start);
                header.add_css_class("folder-header");
                widgets.connection_list.append(&header);

                for conn in connections {
                    let row = gtk::ListBoxRow::new();
                    row.set_tooltip_text(Some(&conn.id.to_string()));
                    let card = gtk::Box::builder()
                        .orientation(gtk::Orientation::Vertical)
                        .spacing(2)
                        .build();
                    card.add_css_class("connection-row");
                    let title = gtk::Label::new(Some(&conn.name));
                    title.set_halign(gtk::Align::Start);
                    title.add_css_class("connection-name");
                    let meta = gtk::Label::new(Some(&format!(
                        "{} · {}",
                        conn.host_label(),
                        conn.backend.label()
                    )));
                    meta.set_halign(gtk::Align::Start);
                    meta.add_css_class("connection-meta");
                    card.append(&title);
                    card.append(&meta);
                    row.set_child(Some(&card));
                    widgets.connection_list.append(&row);
                    if self.selected_connection_id == Some(conn.id) {
                        widgets.connection_list.select_row(Some(&row));
                    }
                }
            }
            self.connections_dirty.set(false);
        }

        if self.groups_dirty.get() {
            while let Some(child) = widgets.tab_bar.first_child() {
                widgets.tab_bar.remove(&child);
            }

            for (i, group) in self.groups.iter().enumerate() {
                let label_text = if let Some(first) = group.panes.first() {
                    first.name.clone()
                } else {
                    format!("Tab {}", i + 1)
                };

                let tab_box = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(4)
                    .build();

                let label = gtk::Label::new(Some(&label_text));
                tab_box.append(&label);

                let close_btn = gtk::Button::with_label("✕");
                close_btn.add_css_class("tab-close");
                let s = sender.clone();
                close_btn.connect_clicked(move |_| {
                    s.input(AppMsg::CloseGroup(i));
                });
                tab_box.append(&close_btn);

                let btn = gtk::Button::new();
                btn.set_child(Some(&tab_box));
                btn.add_css_class("tab-button");
                if self.selected_group == Some(i) {
                    btn.add_css_class("active-tab");
                }
                let s = sender.clone();
                btn.connect_clicked(move |_| {
                    s.input(AppMsg::SelectGroup(i));
                });
                widgets.tab_bar.append(&btn);
            }

            let add_btn = gtk::Button::with_label("+");
            add_btn.add_css_class("tab-add");
            let s = sender.clone();
            add_btn.connect_clicked(move |_| {
                s.input(AppMsg::NewLocalTab);
            });
            widgets.tab_bar.append(&add_btn);

            self.groups_dirty.set(false);
        }

        if self.terminal_dirty.get() {
            if let Some(group) = self.selected_group() {
                widgets.pane_views =
                    rebuild_terminal_panes(&widgets.terminal_container, group, &sender);
                widgets.pane_sizes = vec![(0, 0); widgets.pane_views.len()];
            } else {
                while let Some(child) = widgets.terminal_container.first_child() {
                    widgets.terminal_container.remove(&child);
                }
                let placeholder =
                    gtk::Label::new(Some("Press \"+ Terminal\" or \"Connect\" to start"));
                placeholder.set_vexpand(true);
                placeholder.set_hexpand(true);
                widgets.terminal_container.append(&placeholder);
                widgets.pane_views.clear();
                widgets.pane_sizes.clear();
            }
            self.terminal_dirty.set(false);
        }

        if let Some(group) = self.selected_group() {
            for (i, pane) in group.panes.iter().enumerate() {
                if let Some(area) = widgets.pane_views.get(i) {
                    area.queue_draw();

                    let w = area.width();
                    let h = area.height();
                    if w > 0 && h > 0 {
                        let pad = 4;
                        let font_desc = pango::FontDescription::from_string("Monospace 11");
                        let pango_ctx = area.pango_context();
                        let layout = pango::Layout::new(&pango_ctx);
                        layout.set_font_description(Some(&font_desc));
                        layout.set_text("M");
                        let (char_w, char_h) = layout.pixel_size();
                        if char_w > 0 && char_h > 0 {
                            let cols = ((w - pad * 2) / char_w) as u16;
                            let rows = ((h - pad * 2) / char_h) as u16;
                            if cols > 0 && rows > 0 {
                                let last = widgets.pane_sizes.get(i).copied().unwrap_or((0, 0));
                                if (cols, rows) != last {
                                    let _ = pane.handle.resize(cols, rows);
                                    if i < widgets.pane_sizes.len() {
                                        widgets.pane_sizes[i] = (cols, rows);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn gdk_mods_to_wezterm(modifiers: gdk::ModifierType) -> KeyModifiers {
    let mut mods = KeyModifiers::NONE;
    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        mods |= KeyModifiers::SHIFT;
    }
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        mods |= KeyModifiers::CTRL;
    }
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        mods |= KeyModifiers::ALT;
    }
    if modifiers.contains(gdk::ModifierType::SUPER_MASK) {
        mods |= KeyModifiers::SUPER;
    }
    mods
}

fn gdk_key_to_wezterm(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
) -> Option<(KeyCode, KeyModifiers)> {
    let mods = gdk_mods_to_wezterm(modifiers);

    let keycode = match key {
        gdk::Key::Return | gdk::Key::KP_Enter => KeyCode::Enter,
        gdk::Key::BackSpace => KeyCode::Backspace,
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => KeyCode::Tab,
        gdk::Key::Escape => KeyCode::Escape,
        gdk::Key::Up => KeyCode::UpArrow,
        gdk::Key::Down => KeyCode::DownArrow,
        gdk::Key::Left => KeyCode::LeftArrow,
        gdk::Key::Right => KeyCode::RightArrow,
        gdk::Key::Home => KeyCode::Home,
        gdk::Key::End => KeyCode::End,
        gdk::Key::Page_Up => KeyCode::PageUp,
        gdk::Key::Page_Down => KeyCode::PageDown,
        gdk::Key::Insert => KeyCode::Insert,
        gdk::Key::Delete | gdk::Key::KP_Delete => KeyCode::Delete,
        gdk::Key::F1 => KeyCode::Function(1),
        gdk::Key::F2 => KeyCode::Function(2),
        gdk::Key::F3 => KeyCode::Function(3),
        gdk::Key::F4 => KeyCode::Function(4),
        gdk::Key::F5 => KeyCode::Function(5),
        gdk::Key::F6 => KeyCode::Function(6),
        gdk::Key::F7 => KeyCode::Function(7),
        gdk::Key::F8 => KeyCode::Function(8),
        gdk::Key::F9 => KeyCode::Function(9),
        gdk::Key::F10 => KeyCode::Function(10),
        gdk::Key::F11 => KeyCode::Function(11),
        gdk::Key::F12 => KeyCode::Function(12),
        gdk::Key::F13 => KeyCode::Function(13),
        gdk::Key::F14 => KeyCode::Function(14),
        gdk::Key::F15 => KeyCode::Function(15),
        gdk::Key::F16 => KeyCode::Function(16),
        gdk::Key::F17 => KeyCode::Function(17),
        gdk::Key::F18 => KeyCode::Function(18),
        gdk::Key::F19 => KeyCode::Function(19),
        gdk::Key::F20 => KeyCode::Function(20),
        gdk::Key::F21 => KeyCode::Function(21),
        gdk::Key::F22 => KeyCode::Function(22),
        gdk::Key::F23 => KeyCode::Function(23),
        gdk::Key::F24 => KeyCode::Function(24),
        gdk::Key::KP_0 => KeyCode::Numpad0,
        gdk::Key::KP_1 => KeyCode::Numpad1,
        gdk::Key::KP_2 => KeyCode::Numpad2,
        gdk::Key::KP_3 => KeyCode::Numpad3,
        gdk::Key::KP_4 => KeyCode::Numpad4,
        gdk::Key::KP_5 => KeyCode::Numpad5,
        gdk::Key::KP_6 => KeyCode::Numpad6,
        gdk::Key::KP_7 => KeyCode::Numpad7,
        gdk::Key::KP_8 => KeyCode::Numpad8,
        gdk::Key::KP_9 => KeyCode::Numpad9,
        gdk::Key::KP_Multiply => KeyCode::Multiply,
        gdk::Key::KP_Add => KeyCode::Add,
        gdk::Key::KP_Subtract => KeyCode::Subtract,
        gdk::Key::KP_Decimal => KeyCode::Decimal,
        gdk::Key::KP_Divide => KeyCode::Divide,
        gdk::Key::Shift_L | gdk::Key::Shift_R
        | gdk::Key::Control_L | gdk::Key::Control_R
        | gdk::Key::Alt_L | gdk::Key::Alt_R
        | gdk::Key::Super_L | gdk::Key::Super_R
        | gdk::Key::Meta_L | gdk::Key::Meta_R
        | gdk::Key::Caps_Lock | gdk::Key::Num_Lock => {
            return None;
        }
        _ => {
            if let Some(ch) = key.to_unicode() {
                KeyCode::Char(ch)
            } else {
                return None;
            }
        }
    };

    Some((keycode, mods))
}

fn rgba_to_cairo_surface(rgba: &[u8], width: u32, height: u32) -> gtk::cairo::ImageSurface {
    use gtk::cairo::{Format, ImageSurface};
    let mut surface = ImageSurface::create(Format::ARgb32, width as i32, height as i32)
        .unwrap_or_else(|_| ImageSurface::create(Format::ARgb32, 1, 1).unwrap());
    let stride = surface.stride() as usize;
    {
        if let Ok(mut data) = surface.data() {
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let src = (y * width as usize + x) * 4;
                    if src + 3 >= rgba.len() {
                        break;
                    }
                    let dst = y * stride + x * 4;
                    let r = rgba[src] as u32;
                    let g = rgba[src + 1] as u32;
                    let b = rgba[src + 2] as u32;
                    let a = rgba[src + 3] as u32;
                    data[dst] = (b * a / 255) as u8;
                    data[dst + 1] = (g * a / 255) as u8;
                    data[dst + 2] = (r * a / 255) as u8;
                    data[dst + 3] = a as u8;
                }
            }
        }
    }
    surface
}

fn cairo_fallback_surface() -> gtk::cairo::ImageSurface {
    gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 1, 1).unwrap()
}

fn draw_terminal(
    handle: &TerminalSessionHandle,
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    render_state: &Rc<RefCell<PaneRenderState>>,
) {
    let Some((rows, cols, cursor_x, cursor_y, cursor_shape, cursor_vis, _palette_fg, palette_bg, cursor_bg_color, lines_data, image_cells)) =
        handle.with_terminal(|terminal| {
            let screen = terminal.screen();
            let palette = terminal.palette();
            let cursor = terminal.cursor_pos();
            let phys_rows = screen.physical_rows;
            let phys_cols = screen.physical_cols;

            let palette_fg = palette.foreground;
            let palette_bg = palette.background;
            let cursor_bg = palette.cursor_bg;

            let total = screen.scrollback_rows();
            let start = total.saturating_sub(phys_rows);
            let range = start..total;

            #[allow(clippy::type_complexity)]
            let mut lines_data: Vec<Vec<(usize, String, usize, SrgbaTuple, SrgbaTuple, bool, bool, bool, bool, bool, bool)>> = Vec::new();
            #[allow(clippy::type_complexity)]
            let mut image_cells: Vec<(usize, usize, usize, f32, f32, f32, f32, i32, u16, u16, u16, u16, Arc<ImageData>)> = Vec::new();

            screen.with_phys_lines(range, |phys_lines| {
                for (row_idx, line) in phys_lines.iter().enumerate() {
                    let mut cells = Vec::new();
                    for cell in line.visible_cells() {
                        let attrs = cell.attrs();
                        let intensity = attrs.intensity();
                        let italic = attrs.italic();
                        let underline = attrs.underline() != wezterm_term::Underline::None;
                        let strikethrough = attrs.strikethrough();
                        let hyperlink = attrs.hyperlink().is_some();
                        let reverse = attrs.reverse();

                        let mut fg = palette.resolve_fg(attrs.foreground());
                        let mut bg = palette.resolve_bg(attrs.background());

                        if reverse {
                            std::mem::swap(&mut fg, &mut bg);
                        }

                        let bold = intensity == Intensity::Bold;
                        let half = intensity == Intensity::Half;
                        if half {
                            fg = SrgbaTuple(fg.0 * 0.5, fg.1 * 0.5, fg.2 * 0.5, fg.3);
                        }

                        if let Some(images) = attrs.images() {
                            for img in &images {
                                let tl = img.top_left();
                                let br = img.bottom_right();
                                let (pl, pt, pr, pb) = img.padding();
                                image_cells.push((
                                    row_idx,
                                    cell.cell_index(),
                                    cell.width(),
                                    tl.x.into_inner(),
                                    tl.y.into_inner(),
                                    br.x.into_inner(),
                                    br.y.into_inner(),
                                    img.z_index(),
                                    pl, pt, pr, pb,
                                    img.image_data().clone(),
                                ));
                            }
                        }

                        cells.push((
                            cell.cell_index(),
                            cell.str().to_string(),
                            cell.width(),
                            fg,
                            bg,
                            bold,
                            italic,
                            underline,
                            strikethrough,
                            cell.attrs().invisible(),
                            hyperlink,
                        ));
                    }
                    lines_data.push(cells);
                }
            });

            (
                phys_rows,
                phys_cols,
                cursor.x,
                cursor.y as usize,
                cursor.shape,
                cursor.visibility,
                palette_fg,
                palette_bg,
                cursor_bg,
                lines_data,
                image_cells,
            )
        })
    else {
        cr.set_source_rgb(0.11, 0.11, 0.14);
        let _ = cr.paint();
        return;
    };

    let pad = 4.0_f64;

    let mut font_opts = gtk::cairo::FontOptions::new().unwrap();
    font_opts.set_antialias(gtk::cairo::Antialias::Subpixel);
    font_opts.set_hint_style(gtk::cairo::HintStyle::Full);
    font_opts.set_hint_metrics(gtk::cairo::HintMetrics::On);
    font_opts.set_subpixel_order(gtk::cairo::SubpixelOrder::Rgb);

    let font_desc = pango::FontDescription::from_string("Monospace 11");
    let pango_ctx = pangocairo::functions::create_context(cr);
    pango_ctx.set_font_description(Some(&font_desc));
    pangocairo::functions::context_set_font_options(&pango_ctx, Some(&font_opts));
    let layout = pango::Layout::new(&pango_ctx);
    layout.set_font_description(Some(&font_desc));
    layout.set_text("M");
    let (cell_w, cell_h) = layout.pixel_size();
    let cell_w = cell_w as f64;
    let cell_h = cell_h as f64;

    {
        let mut rs = render_state.borrow_mut();
        rs.cell_width = cell_w;
        rs.cell_height = cell_h;
    }

    cr.set_source_rgb(palette_bg.0 as f64, palette_bg.1 as f64, palette_bg.2 as f64);
    let _ = cr.paint();

    let _ = cr.save();
    cr.translate(pad, pad);
    let draw_w = width as f64 - pad * 2.0;
    let draw_h = height as f64 - pad * 2.0;
    cr.rectangle(0.0, 0.0, draw_w, draw_h);
    cr.clip();

    for (row_idx, line_cells) in lines_data.iter().enumerate() {
        let y = row_idx as f64 * cell_h;
        if y > draw_h {
            break;
        }

        for &(col, ref text, cell_width, fg, bg, bold, italic, underline, strikethrough, invisible, hyperlink) in line_cells {
            let x = col as f64 * cell_w;
            let w = cell_width as f64 * cell_w;

            if x > width as f64 {
                break;
            }

            let bg_differs = (bg.0 - palette_bg.0).abs() > 0.001
                || (bg.1 - palette_bg.1).abs() > 0.001
                || (bg.2 - palette_bg.2).abs() > 0.001;
            if bg_differs {
                cr.set_source_rgb(bg.0 as f64, bg.1 as f64, bg.2 as f64);
                cr.rectangle(x, y, w, cell_h);
                let _ = cr.fill();
            }

            if invisible || text.is_empty() || text == " " {
                continue;
            }

            let mut fd = font_desc.clone();
            if bold {
                fd.set_weight(pango::Weight::Bold);
            }
            if italic {
                fd.set_style(pango::Style::Italic);
            }
            layout.set_font_description(Some(&fd));
            layout.set_text(text);

            cr.set_source_rgb(fg.0 as f64, fg.1 as f64, fg.2 as f64);
            cr.move_to(x, y);
            pangocairo::functions::show_layout(cr, &layout);

            if underline {
                cr.set_source_rgb(fg.0 as f64, fg.1 as f64, fg.2 as f64);
                cr.set_line_width(1.0);
                cr.move_to(x, y + cell_h - 1.0);
                cr.line_to(x + w, y + cell_h - 1.0);
                let _ = cr.stroke();
            }

            if strikethrough {
                cr.set_source_rgb(fg.0 as f64, fg.1 as f64, fg.2 as f64);
                cr.set_line_width(1.0);
                cr.move_to(x, y + cell_h / 2.0);
                cr.line_to(x + w, y + cell_h / 2.0);
                let _ = cr.stroke();
            }

            if hyperlink {
                cr.set_source_rgba(0.4, 0.6, 1.0, 0.9);
                cr.set_line_width(1.0);
                cr.move_to(x, y + cell_h - 1.0);
                cr.line_to(x + w, y + cell_h - 1.0);
                let _ = cr.stroke();
            }
        }
    }

    if !image_cells.is_empty() {
        let mut rs = render_state.borrow_mut();
        for &(row, col, cw, tl_x, tl_y, br_x, br_y, _z_index, pl, pt, pr, pb, ref img_data) in &image_cells {
            let hash = img_data.hash();
            let surface = rs.image_cache.entry(hash).or_insert_with(|| {
                let data_guard = img_data.data();
                let (rgba, iw, ih) = match &*data_guard {
                    ImageDataType::Rgba8 { data, width, height, .. } => {
                        (data.as_slice(), *width, *height)
                    }
                    ImageDataType::AnimRgba8 { frames, width, height, .. } => {
                        if let Some(frame) = frames.first() {
                            (frame.as_slice(), *width, *height)
                        } else {
                            return cairo_fallback_surface();
                        }
                    }
                    _ => {
                        return cairo_fallback_surface();
                    }
                };
                rgba_to_cairo_surface(rgba, iw, ih)
            });

            let dest_x = col as f64 * cell_w + pl as f64;
            let dest_y = row as f64 * cell_h + pt as f64;
            let dest_w = cw as f64 * cell_w - pl as f64 - pr as f64;
            let dest_h = cell_h - pt as f64 - pb as f64;

            if dest_w <= 0.0 || dest_h <= 0.0 {
                continue;
            }

            let img_w = surface.width() as f64;
            let img_h = surface.height() as f64;
            if img_w <= 0.0 || img_h <= 0.0 {
                continue;
            }

            let src_x = tl_x as f64 * img_w;
            let src_y = tl_y as f64 * img_h;
            let src_w = (br_x - tl_x) as f64 * img_w;
            let src_h = (br_y - tl_y) as f64 * img_h;

            if src_w <= 0.0 || src_h <= 0.0 {
                continue;
            }

            let _ = cr.save();
            cr.rectangle(dest_x, dest_y, dest_w, dest_h);
            cr.clip();
            let scale_x = dest_w / src_w;
            let scale_y = dest_h / src_h;
            cr.translate(dest_x, dest_y);
            cr.scale(scale_x, scale_y);
            let _ = cr.set_source_surface(surface, -src_x, -src_y);
            let _ = cr.paint();
            let _ = cr.restore();
        }
    }

    if let Some((sel_start, sel_end)) = render_state.borrow().normalized_selection() {
        cr.set_source_rgba(0.2, 0.4, 0.8, 0.3);
        for row in sel_start.row..=sel_end.row {
            let y = row as f64 * cell_h;
            if y > height as f64 {
                break;
            }
            let (sc, ec) = if sel_start.row == sel_end.row {
                (sel_start.col, sel_end.col + 1)
            } else if row == sel_start.row {
                (sel_start.col, cols)
            } else if row == sel_end.row {
                (0, sel_end.col + 1)
            } else {
                (0, cols)
            };
            let x = sc as f64 * cell_w;
            let w = ec.saturating_sub(sc) as f64 * cell_w;
            cr.rectangle(x, y, w, cell_h);
            let _ = cr.fill();
        }
    }

    if cursor_vis == CursorVisibility::Visible && cursor_y < rows {
        let cx = cursor_x as f64 * cell_w;
        let cy = cursor_y as f64 * cell_h;

        match cursor_shape {
            CursorShape::BlinkingBlock | CursorShape::SteadyBlock | CursorShape::Default => {
                cr.set_source_rgba(
                    cursor_bg_color.0 as f64,
                    cursor_bg_color.1 as f64,
                    cursor_bg_color.2 as f64,
                    0.7,
                );
                cr.rectangle(cx, cy, cell_w, cell_h);
                let _ = cr.fill();
            }
            CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                cr.set_source_rgb(
                    cursor_bg_color.0 as f64,
                    cursor_bg_color.1 as f64,
                    cursor_bg_color.2 as f64,
                );
                cr.set_line_width(2.0);
                cr.move_to(cx, cy + cell_h - 1.0);
                cr.line_to(cx + cell_w, cy + cell_h - 1.0);
                let _ = cr.stroke();
            }
            CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                cr.set_source_rgb(
                    cursor_bg_color.0 as f64,
                    cursor_bg_color.1 as f64,
                    cursor_bg_color.2 as f64,
                );
                cr.set_line_width(2.0);
                cr.move_to(cx, cy);
                cr.line_to(cx, cy + cell_h);
                let _ = cr.stroke();
            }
        }
    }

    let _ = cr.restore();
}

fn extract_selection_text(
    handle: &TerminalSessionHandle,
    render_state: &Rc<RefCell<PaneRenderState>>,
) -> Option<String> {
    let (start, end) = render_state.borrow().normalized_selection()?;
    if start.col == end.col && start.row == end.row {
        return None;
    }
    handle.with_terminal(|terminal| {
        let screen = terminal.screen();
        let total = screen.scrollback_rows();
        let phys_rows = screen.physical_rows;
        let view_start = total.saturating_sub(phys_rows);
        let phys_start = view_start + start.row;
        let phys_end = view_start + end.row + 1;
        let mut result = String::new();
        screen.with_phys_lines(phys_start..phys_end, |lines| {
            for (i, line) in lines.iter().enumerate() {
                let row = start.row + i;
                let col_start = if row == start.row { start.col } else { 0 };
                let col_end = if row == end.row {
                    end.col + 1
                } else {
                    usize::MAX
                };
                for cell in line.visible_cells() {
                    let ci = cell.cell_index();
                    if ci >= col_start && ci < col_end {
                        result.push_str(cell.str());
                    }
                }
                let trimmed = result.trim_end_matches(' ').len();
                result.truncate(trimmed);
                if row != end.row {
                    result.push('\n');
                }
            }
        });
        result
    })
}

fn extract_screen_text(handle: &TerminalSessionHandle) -> String {
    handle
        .with_terminal(|terminal| {
            let screen = terminal.screen();
            let total = screen.scrollback_rows();
            let phys_rows = screen.physical_rows;
            let view_start = total.saturating_sub(phys_rows);
            let mut result = String::new();
            screen.with_phys_lines(view_start..total, |lines| {
                for line in lines {
                    for cell in line.visible_cells() {
                        result.push_str(cell.str());
                    }
                    let trimmed = result.trim_end_matches(' ').len();
                    result.truncate(trimmed);
                    result.push('\n');
                }
            });
            result
        })
        .unwrap_or_default()
}

fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}

fn open_text_in_editor(text: &str) {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "rshell-{}.txt",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    if std::fs::write(&path, text).is_ok() {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", &path.to_string_lossy()])
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&path).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        }
    }
}

fn build_pane_view(
    index: usize,
    handle: TerminalSessionHandle,
    sender: &ComponentSender<RshellApp>,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.add_css_class("terminal-view");
    area.set_can_focus(true);
    area.set_focusable(true);

    let render_state = Rc::new(RefCell::new(PaneRenderState::default()));

    let draw_handle = handle.clone();
    let rs_draw = render_state.clone();
    area.set_draw_func(move |_area, cr, w, h| {
        draw_terminal(&draw_handle, cr, w, h, &rs_draw);
    });

    let s = sender.clone();
    let copy_handle = handle.clone();
    let paste_handle = handle.clone();
    let kb_handle = handle.clone();
    let rs_copy = render_state.clone();
    let kc = gtk::EventControllerKey::new();
    kc.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        if (key == gdk::Key::C && ctrl && shift) || (key == gdk::Key::Insert && ctrl) {
            if let Some(text) = extract_selection_text(&copy_handle, &rs_copy)
                && let Some(display) = gdk::Display::default()
            {
                display.clipboard().set_text(&text);
            }
            return glib::Propagation::Stop;
        }
        if (key == gdk::Key::V && ctrl && shift)
            || (key == gdk::Key::Insert && shift && !ctrl && !alt)
            || (key == gdk::Key::Insert && alt && !ctrl)
        {
            let ph = paste_handle.clone();
            if let Some(display) = gdk::Display::default() {
                let cb = display.clipboard();
                cb.read_text_async(None::<&gio::Cancellable>, move |result| {
                    if let Ok(Some(text)) = result {
                        let _ = ph.send_bytes(text.as_bytes().to_vec());
                    }
                });
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::L && ctrl && shift {
            let _ = kb_handle.send_bytes(b"\x0c".to_vec());
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::B && ctrl && shift {
            kb_handle.with_terminal_mut(|t| t.advance_bytes(b"\x1b[3J"));
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::A && ctrl && shift {
            let _ = kb_handle.send_bytes(b"\x0c".to_vec());
            kb_handle.with_terminal_mut(|t| t.advance_bytes(b"\x1b[3J"));
            return glib::Propagation::Stop;
        }
        s.input(AppMsg::PaneKeyPress(index, key, mods));
        glib::Propagation::Stop
    });
    area.add_controller(kc);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    let drag_start = Rc::new(Cell::new((0.0f64, 0.0f64)));

    {
        let rs = render_state.clone();
        let dh = handle.clone();
        let da = area.clone();
        let ds = sender.clone();
        let start = drag_start.clone();
        drag.connect_drag_begin(move |_, x, y| {
            da.grab_focus();
            ds.input(AppMsg::FocusPane(index));
            start.set((x, y));
            rs.borrow_mut().selection = None;
            let coord = rs.borrow().pixel_to_cell(x, y);
            let event = WezMouseEvent {
                kind: MouseEventKind::Press,
                x: coord.col,
                y: coord.row as i64,
                x_pixel_offset: 0,
                y_pixel_offset: 0,
                button: MouseButton::Left,
                modifiers: KeyModifiers::NONE,
            };
            let _ = dh.with_terminal_mut(|t| t.mouse_event(event));
            da.queue_draw();
        });
    }

    {
        let rs = render_state.clone();
        let da = area.clone();
        let dh = handle.clone();
        let start = drag_start.clone();
        drag.connect_drag_update(move |_, ox, oy| {
            let (sx, sy) = start.get();
            let start_coord = rs.borrow().pixel_to_cell(sx, sy);
            let end_coord =
                rs.borrow()
                    .pixel_to_cell((sx + ox).max(0.0), (sy + oy).max(0.0));
            if start_coord.col != end_coord.col || start_coord.row != end_coord.row {
                rs.borrow_mut().selection = Some((start_coord, end_coord));
            }
            let event = WezMouseEvent {
                kind: MouseEventKind::Move,
                x: end_coord.col,
                y: end_coord.row as i64,
                x_pixel_offset: 0,
                y_pixel_offset: 0,
                button: MouseButton::Left,
                modifiers: KeyModifiers::NONE,
            };
            let _ = dh.with_terminal_mut(|t| t.mouse_event(event));
            da.queue_draw();
        });
    }

    {
        let rs = render_state.clone();
        let dh = handle.clone();
        let da = area.clone();
        let start = drag_start;
        drag.connect_drag_end(move |_, ox, oy| {
            let (sx, sy) = start.get();
            let start_coord = rs.borrow().pixel_to_cell(sx, sy);
            let end_coord =
                rs.borrow()
                    .pixel_to_cell((sx + ox).max(0.0), (sy + oy).max(0.0));
            if start_coord.col == end_coord.col && start_coord.row == end_coord.row {
                rs.borrow_mut().selection = None;
            } else if let Some(ref mut sel) = rs.borrow_mut().selection {
                sel.1 = end_coord;
            }
            let event = WezMouseEvent {
                kind: MouseEventKind::Release,
                x: end_coord.col,
                y: end_coord.row as i64,
                x_pixel_offset: 0,
                y_pixel_offset: 0,
                button: MouseButton::Left,
                modifiers: KeyModifiers::NONE,
            };
            let _ = dh.with_terminal_mut(|t| t.mouse_event(event));
            da.queue_draw();
        });
    }
    area.add_controller(drag);

    let scroll_handle = handle.clone();
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll(move |_, _dx, dy| {
        let button = if dy < 0.0 {
            MouseButton::WheelUp((-dy).ceil() as usize)
        } else {
            MouseButton::WheelDown(dy.ceil() as usize)
        };
        let event = WezMouseEvent {
            kind: MouseEventKind::Press,
            x: 0,
            y: 0,
            x_pixel_offset: 0,
            y_pixel_offset: 0,
            button,
            modifiers: KeyModifiers::NONE,
        };
        let _ = scroll_handle.with_terminal_mut(|t| t.mouse_event(event));
        glib::Propagation::Stop
    });
    area.add_controller(scroll);

    let actions = gio::SimpleActionGroup::new();

    let copy_action = gio::SimpleAction::new("copy", None);
    {
        let ch = handle.clone();
        let crs = render_state.clone();
        copy_action.connect_activate(move |_, _| {
            if let Some(text) = extract_selection_text(&ch, &crs)
                && let Some(display) = gdk::Display::default()
            {
                display.clipboard().set_text(&text);
            }
        });
    }
    actions.add_action(&copy_action);

    let paste_action = gio::SimpleAction::new("paste", None);
    {
        let ph = handle.clone();
        paste_action.connect_activate(move |_, _| {
            let ph2 = ph.clone();
            if let Some(display) = gdk::Display::default() {
                let cb = display.clipboard();
                cb.read_text_async(None::<&gio::Cancellable>, move |result| {
                    if let Ok(Some(text)) = result {
                        let _ = ph2.send_bytes(text.as_bytes().to_vec());
                    }
                });
            }
        });
    }
    actions.add_action(&paste_action);

    let paste_sel_action = gio::SimpleAction::new("paste-selection", None);
    {
        let ph = handle.clone();
        paste_sel_action.connect_activate(move |_, _| {
            let ph2 = ph.clone();
            if let Some(display) = gdk::Display::default() {
                #[cfg(target_os = "linux")]
                let cb = display.primary_clipboard();
                #[cfg(not(target_os = "linux"))]
                let cb = display.clipboard();
                cb.read_text_async(None::<&gio::Cancellable>, move |result| {
                    if let Ok(Some(text)) = result {
                        let _ = ph2.send_bytes(text.as_bytes().to_vec());
                    }
                });
            }
        });
    }
    actions.add_action(&paste_sel_action);

    let selectall_action = gio::SimpleAction::new("select-all", None);
    {
        let sah = handle.clone();
        let sars = render_state.clone();
        let sa_area = area.clone();
        selectall_action.connect_activate(move |_, _| {
            if let Some(phys_rows) = sah.with_terminal(|t| t.screen().physical_rows) {
                sars.borrow_mut().selection = Some((
                    CellCoord { col: 0, row: 0 },
                    CellCoord {
                        col: usize::MAX,
                        row: phys_rows.saturating_sub(1),
                    },
                ));
                sa_area.queue_draw();
            }
        });
    }
    actions.add_action(&selectall_action);

    let selectscreen_action = gio::SimpleAction::new("select-screen", None);
    {
        let ssh = handle.clone();
        let ssrs = render_state.clone();
        let ss_area = area.clone();
        selectscreen_action.connect_activate(move |_, _| {
            if let Some(phys_rows) = ssh.with_terminal(|t| t.screen().physical_rows) {
                ssrs.borrow_mut().selection = Some((
                    CellCoord { col: 0, row: 0 },
                    CellCoord {
                        col: usize::MAX,
                        row: phys_rows.saturating_sub(1),
                    },
                ));
                ss_area.queue_draw();
            }
        });
    }
    actions.add_action(&selectscreen_action);

    let editor_screen_action = gio::SimpleAction::new("editor-screen", None);
    {
        let esh = handle.clone();
        editor_screen_action.connect_activate(move |_, _| {
            let text = extract_screen_text(&esh);
            open_text_in_editor(&text);
        });
    }
    actions.add_action(&editor_screen_action);

    let editor_sel_action = gio::SimpleAction::new("editor-selection", None);
    {
        let eseh = handle.clone();
        let esrs = render_state.clone();
        editor_sel_action.connect_activate(move |_, _| {
            if let Some(text) = extract_selection_text(&eseh, &esrs) {
                open_text_in_editor(&text);
            }
        });
    }
    actions.add_action(&editor_sel_action);

    let local_ip_action = gio::SimpleAction::new("paste-local-ip", None);
    {
        let lih = handle.clone();
        local_ip_action.connect_activate(move |_, _| {
            if let Some(ip) = get_local_ip() {
                let _ = lih.send_bytes(ip.into_bytes());
            }
        });
    }
    actions.add_action(&local_ip_action);

    let remote_ip_action = gio::SimpleAction::new("paste-remote-ip", None);
    {
        let ris = sender.clone();
        remote_ip_action.connect_activate(move |_, _| {
            ris.input(AppMsg::PasteRemoteIp(index));
        });
    }
    actions.add_action(&remote_ip_action);

    let break_action = gio::SimpleAction::new("send-break", None);
    {
        let bh = handle.clone();
        break_action.connect_activate(move |_, _| {
            let _ = bh.send_bytes(b"\x03".to_vec());
        });
    }
    actions.add_action(&break_action);

    let reset_cursor_action = gio::SimpleAction::new("reset-cursor", None);
    {
        let rch = handle.clone();
        reset_cursor_action.connect_activate(move |_, _| {
            rch.with_terminal_mut(|t| t.advance_bytes(b"\x1b[?25h\x1b[0 q"));
        });
    }
    actions.add_action(&reset_cursor_action);

    let reset_term_action = gio::SimpleAction::new("reset-terminal", None);
    {
        let rth = handle.clone();
        reset_term_action.connect_activate(move |_, _| {
            rth.with_terminal_mut(|t| t.advance_bytes(b"\x1bc"));
        });
    }
    actions.add_action(&reset_term_action);

    let clear_action = gio::SimpleAction::new("clear-screen", None);
    {
        let clh = handle.clone();
        clear_action.connect_activate(move |_, _| {
            let _ = clh.send_bytes(b"\x0c".to_vec());
        });
    }
    actions.add_action(&clear_action);

    let clrsb_action = gio::SimpleAction::new("clear-scrollback", None);
    {
        let csh = handle.clone();
        clrsb_action.connect_activate(move |_, _| {
            csh.with_terminal_mut(|t| t.advance_bytes(b"\x1b[3J"));
        });
    }
    actions.add_action(&clrsb_action);

    let clear_both_action = gio::SimpleAction::new("clear-both", None);
    {
        let cbh = handle.clone();
        clear_both_action.connect_activate(move |_, _| {
            let _ = cbh.send_bytes(b"\x0c".to_vec());
            cbh.with_terminal_mut(|t| t.advance_bytes(b"\x1b[3J"));
        });
    }
    actions.add_action(&clear_both_action);

    let fullscreen_action = gio::SimpleAction::new("fullscreen", None);
    {
        let fa = area.clone();
        fullscreen_action.connect_activate(move |_, _| {
            if let Some(root) = fa.root()
                && let Some(window) = root.downcast_ref::<gtk::Window>()
            {
                if window.is_fullscreen() {
                    window.unfullscreen();
                } else {
                    window.fullscreen();
                }
            }
        });
    }
    actions.add_action(&fullscreen_action);

    let broadcast_action = gio::SimpleAction::new("broadcast", None);
    {
        let bs = sender.clone();
        broadcast_action.connect_activate(move |_, _| {
            bs.input(AppMsg::ToggleBroadcast);
        });
    }
    actions.add_action(&broadcast_action);

    area.insert_action_group("term", Some(&actions));

    let menu = gio::Menu::new();

    let edit_section = gio::Menu::new();
    let copy_item = gio::MenuItem::new(Some("复制(_C)"), Some("term.copy"));
    copy_item.set_attribute_value("accel", Some(&"<Control>Insert".to_variant()));
    edit_section.append_item(&copy_item);
    let paste_item = gio::MenuItem::new(Some("粘贴(_P)"), Some("term.paste"));
    paste_item.set_attribute_value("accel", Some(&"<Shift>Insert".to_variant()));
    edit_section.append_item(&paste_item);
    let paste_sel_item = gio::MenuItem::new(Some("粘贴选择内容(_E)"), Some("term.paste-selection"));
    paste_sel_item.set_attribute_value("accel", Some(&"<Alt>Insert".to_variant()));
    edit_section.append_item(&paste_sel_item);
    menu.append_section(None, &edit_section);

    let sel_section = gio::Menu::new();
    sel_section.append(Some("全选(_A)"), Some("term.select-all"));
    sel_section.append(Some("选择屏幕(_S)"), Some("term.select-screen"));
    menu.append_section(None, &sel_section);

    let editor_submenu = gio::Menu::new();
    editor_submenu.append(Some("屏幕内容"), Some("term.editor-screen"));
    editor_submenu.append(Some("选中内容"), Some("term.editor-selection"));
    let editor_section = gio::Menu::new();
    editor_section.append_submenu(Some("到文本编辑器(_X)"), &editor_submenu);
    menu.append_section(None, &editor_section);

    let ip_section = gio::Menu::new();
    ip_section.append(Some("粘贴本地IP地址(_L)"), Some("term.paste-local-ip"));
    ip_section.append(Some("粘贴远程IP地址(_I)"), Some("term.paste-remote-ip"));
    menu.append_section(None, &ip_section);

    let break_section = gio::Menu::new();
    break_section.append(Some("发送Break(_B)"), Some("term.send-break"));
    menu.append_section(None, &break_section);

    let reset_section = gio::Menu::new();
    reset_section.append(Some("重置游标(_R)"), Some("term.reset-cursor"));
    reset_section.append(Some("重置终端(_M)"), Some("term.reset-terminal"));
    menu.append_section(None, &reset_section);

    let clear_section = gio::Menu::new();
    let clear_item = gio::MenuItem::new(Some("清屏(_L)"), Some("term.clear-screen"));
    clear_item.set_attribute_value("accel", Some(&"<Control><Shift>l".to_variant()));
    clear_section.append_item(&clear_item);
    let clrsb_item =
        gio::MenuItem::new(Some("滚动缓冲区清除(_O)"), Some("term.clear-scrollback"));
    clrsb_item.set_attribute_value("accel", Some(&"<Control><Shift>b".to_variant()));
    clear_section.append_item(&clrsb_item);
    let clear_both_item =
        gio::MenuItem::new(Some("屏幕和滚动缓冲区清除(_N)"), Some("term.clear-both"));
    clear_both_item.set_attribute_value("accel", Some(&"<Control><Shift>a".to_variant()));
    clear_section.append_item(&clear_both_item);
    menu.append_section(None, &clear_section);

    let window_section = gio::Menu::new();
    window_section.append(Some("全屏(_U)"), Some("term.fullscreen"));
    window_section.append(Some("发送键输入到所有会话(_K)"), Some("term.broadcast"));
    menu.append_section(None, &window_section);

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(&area);
    popover.set_has_arrow(false);

    let pop_cleanup = popover.clone();
    area.connect_destroy(move |_| {
        pop_cleanup.unparent();
    });

    let rclick = gtk::GestureClick::new();
    rclick.set_button(3);
    rclick.connect_pressed(move |_, _, x, y| {
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.popup();
    });
    area.add_controller(rclick);

    let mclick_handle = handle;
    let mclick_rs = render_state;
    let mclick = gtk::GestureClick::new();
    mclick.set_button(2);
    mclick.connect_pressed(move |_, _, x, y| {
        let coord = mclick_rs.borrow().pixel_to_cell(x, y);
        let event = WezMouseEvent {
            kind: MouseEventKind::Press,
            x: coord.col,
            y: coord.row as i64,
            x_pixel_offset: 0,
            y_pixel_offset: 0,
            button: MouseButton::Middle,
            modifiers: KeyModifiers::NONE,
        };
        let _ = mclick_handle.with_terminal_mut(|t| t.mouse_event(event));
    });
    area.add_controller(mclick);

    area
}

fn rebuild_terminal_panes(
    container: &gtk::Box,
    group: &TerminalGroup,
    sender: &ComponentSender<RshellApp>,
) -> Vec<gtk::DrawingArea> {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    if group.panes.is_empty() {
        let label = gtk::Label::new(Some("No terminal panes"));
        label.set_vexpand(true);
        label.set_hexpand(true);
        container.append(&label);
        return vec![];
    }

    let views: Vec<gtk::DrawingArea> = group
        .panes
        .iter()
        .enumerate()
        .map(|(i, pane)| build_pane_view(i, pane.handle.clone(), sender))
        .collect();

    match group.layout {
        SplitLayout::Single => {
            container.append(&views[0]);
        }
        SplitLayout::HSplit => {
            let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
            paned.set_start_child(Some(&views[0]));
            paned.set_end_child(Some(&views[1]));
            paned.set_hexpand(true);
            paned.set_vexpand(true);
            container.append(&paned);
        }
        SplitLayout::VSplit => {
            let paned = gtk::Paned::new(gtk::Orientation::Vertical);
            paned.set_start_child(Some(&views[0]));
            paned.set_end_child(Some(&views[1]));
            paned.set_hexpand(true);
            paned.set_vexpand(true);
            container.append(&paned);
        }
        SplitLayout::TopBottom3 => {
            let outer = gtk::Paned::new(gtk::Orientation::Vertical);
            let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
            inner.set_start_child(Some(&views[1]));
            inner.set_end_child(Some(&views[2]));
            outer.set_start_child(Some(&views[0]));
            outer.set_end_child(Some(&inner));
            outer.set_hexpand(true);
            outer.set_vexpand(true);
            container.append(&outer);
        }
        SplitLayout::Grid => {
            let outer = gtk::Paned::new(gtk::Orientation::Vertical);
            let top = gtk::Paned::new(gtk::Orientation::Horizontal);
            let bottom = gtk::Paned::new(gtk::Orientation::Horizontal);
            top.set_start_child(Some(&views[0]));
            top.set_end_child(Some(&views[1]));
            bottom.set_start_child(Some(&views[2]));
            bottom.set_end_child(Some(&views[3]));
            outer.set_start_child(Some(&top));
            outer.set_end_child(Some(&bottom));
            outer.set_hexpand(true);
            outer.set_vexpand(true);
            container.append(&outer);
        }
    }

    if let Some(v) = views.get(group.active_pane) {
        v.grab_focus();
    }

    views
}

impl SimpleComponent for RshellApp {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Root = gtk::Window;
    type Widgets = AppWidgets;

    fn init_root() -> Self::Root {
        gtk::Window::builder()
            .title("rsHell")
            .default_width(1280)
            .default_height(800)
            .build()
    }

    fn init(
        _init: Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let settings_repo = SettingsRepository::default();
        let global_config = settings_repo.load().unwrap_or_default();
        crate::theme::apply_theme(global_config.theme);

        let repository = ConnectionRepository::default();
        let mut store = repository.load().unwrap_or_default();
        if store.connections.is_empty() {
            let mut sample = ConnectionProfile::new("Demo host", "127.0.0.1");
            sample.note = "Sample profile".into();
            store.upsert(sample.clone());
            let _ = repository.save(&store);
        }

        let selected_connection_id = store.connections.first().map(|p| p.id);
        let draft = selected_connection_id
            .and_then(|id| store.connection(id))
            .map(|p| ConnectionDraft::from_profile(&store, p))
            .unwrap_or_else(ConnectionDraft::empty);

        let mut model = RshellApp {
            repository,
            store,
            settings_repo,
            global_config,
            selected_connection_id,
            draft,
            groups: Vec::new(),
            selected_group: None,
            toast: "Ready".into(),
            sidebar_visible: true,
            editor_visible: false,
            broadcast_mode: false,
            connections_dirty: Cell::new(true),
            groups_dirty: Cell::new(true),
            terminal_dirty: Cell::new(true),
            draft_dirty: Cell::new(true),
            updating_draft: Cell::new(false),
            settings_visible: false,
            settings_dirty: Cell::new(true),
            updating_settings: Cell::new(false),
        };

        match launch_local_session(model.default_resolved_settings()) {
            Ok(handle) => {
                let mut group = TerminalGroup {
                    layout: SplitLayout::Single,
                    panes: Vec::new(),
                    active_pane: 0,
                };
                group.panes.push(TerminalPane {
                    name: "Local Shell".into(),
                    handle,
                    remote_host: None,
                });
                model.groups.push(group);
                model.selected_group = Some(0);
                model.toast = "Local shell launched".into();
            }
            Err(e) => {
                model.toast = format!("Local shell failed: {e:#}");
            }
        }

        let root_vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root_vbox.add_css_class("rshell-root");

        let header_bar = gtk::HeaderBar::new();
        header_bar.add_css_class("rshell-toolbar");

        let menu = gio::Menu::new();

        let session_submenu = gio::Menu::new();
        session_submenu.append(Some("New Session"), Some("win.new-session"));
        session_submenu.append(Some("New Local Tab"), Some("win.new-local-tab"));
        menu.append_submenu(Some("Session"), &session_submenu);

        let view_submenu = gio::Menu::new();
        view_submenu.append(Some("Toggle Sidebar"), Some("win.toggle-sidebar"));
        view_submenu.append(Some("Fullscreen"), Some("win.toggle-fullscreen"));
        menu.append_submenu(Some("View"), &view_submenu);

        let theme_submenu = gio::Menu::new();
        theme_submenu.append(Some("Light"), Some("win.theme-light"));
        theme_submenu.append(Some("Dark"), Some("win.theme-dark"));
        theme_submenu.append(Some("System"), Some("win.theme-system"));
        menu.append_submenu(Some("Theme"), &theme_submenu);

        let settings_section = gio::Menu::new();
        settings_section.append(Some("Settings"), Some("win.open-settings"));
        menu.append_section(None, &settings_section);

        let about_section = gio::Menu::new();
        about_section.append(Some("About rsHell"), Some("win.about"));
        menu.append_section(None, &about_section);

        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_tooltip_text(Some("Menu"));
        let popover_menu = gtk::PopoverMenu::from_model(Some(&menu));
        menu_btn.set_popover(Some(&popover_menu));

        let action_toggle_sidebar = gio::SimpleAction::new("toggle-sidebar", None);
        {
            let s = sender.clone();
            action_toggle_sidebar.connect_activate(move |_, _| {
                s.input(AppMsg::ToggleSidebar);
            });
        }

        let action_new_session = gio::SimpleAction::new("new-session", None);
        {
            let s = sender.clone();
            action_new_session.connect_activate(move |_, _| {
                s.input(AppMsg::NewConnection);
            });
        }

        let action_new_local = gio::SimpleAction::new("new-local-tab", None);
        {
            let s = sender.clone();
            action_new_local.connect_activate(move |_, _| {
                s.input(AppMsg::NewLocalTab);
            });
        }

        let action_about = gio::SimpleAction::new("about", None);
        {
            let win_ref = window.clone();
            action_about.connect_activate(move |_, _| {
                let about = gtk::AboutDialog::builder()
                    .program_name("rsHell")
                    .version("0.1.0")
                    .comments("Cross-platform SSH Terminal Manager")
                    .transient_for(&win_ref)
                    .modal(true)
                    .build();
                about.present();
            });
        }

        let action_toggle_fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
        {
            let win_ref = window.clone();
            action_toggle_fullscreen.connect_activate(move |_, _| {
                win_ref.set_fullscreened(!win_ref.is_fullscreen());
            });
        }

        let action_theme_light = gio::SimpleAction::new("theme-light", None);
        {
            let s = sender.clone();
            action_theme_light.connect_activate(move |_, _| {
                s.input(AppMsg::ThemeChanged(AppTheme::Light));
            });
        }

        let action_theme_dark = gio::SimpleAction::new("theme-dark", None);
        {
            let s = sender.clone();
            action_theme_dark.connect_activate(move |_, _| {
                s.input(AppMsg::ThemeChanged(AppTheme::Dark));
            });
        }

        let action_theme_system = gio::SimpleAction::new("theme-system", None);
        {
            let s = sender.clone();
            action_theme_system.connect_activate(move |_, _| {
                s.input(AppMsg::ThemeChanged(AppTheme::System));
            });
        }

        let action_open_settings = gio::SimpleAction::new("open-settings", None);
        {
            let s = sender.clone();
            action_open_settings.connect_activate(move |_, _| {
                s.input(AppMsg::OpenGlobalSettings);
            });
        }

        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&action_toggle_sidebar);
        actions.add_action(&action_new_session);
        actions.add_action(&action_new_local);
        actions.add_action(&action_about);
        actions.add_action(&action_toggle_fullscreen);
        actions.add_action(&action_theme_light);
        actions.add_action(&action_theme_dark);
        actions.add_action(&action_theme_system);
        actions.add_action(&action_open_settings);
        window.insert_action_group("win", Some(&actions));

        let connect_btn = gtk::Button::with_label("Connect");
        connect_btn.add_css_class("connect-button");

        let sep1 = gtk::Separator::new(gtk::Orientation::Vertical);
        sep1.set_margin_start(4);
        sep1.set_margin_end(4);

        let split_h_btn = gtk::Button::with_label("H-Split");
        let split_v_btn = gtk::Button::with_label("V-Split");
        let close_pane_btn = gtk::Button::with_label("Close Pane");

        let toast_label = gtk::Label::new(None);
        toast_label.add_css_class("toast-label");

        let title_label = gtk::Label::new(Some("rsHell"));
        title_label.add_css_class("title");
        let title_box = gtk::CenterBox::new();
        title_box.set_hexpand(true);
        title_box.set_center_widget(Some(&title_label));
        title_box.set_end_widget(Some(&toast_label));

        header_bar.pack_start(&menu_btn);
        header_bar.pack_start(&connect_btn);
        header_bar.pack_start(&sep1);
        header_bar.pack_start(&split_h_btn);
        header_bar.pack_start(&split_v_btn);
        header_bar.pack_start(&close_pane_btn);
        header_bar.set_title_widget(Some(&title_box));

        window.set_titlebar(Some(&header_bar));

        let main_paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .position(160)
            .wide_handle(false)
            .shrink_start_child(false)
            .resize_start_child(false)
            .shrink_end_child(false)
            .resize_end_child(true)
            .build();

        let sidebar_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .reveal_child(true)
            .build();

        let sidebar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(140)
            .build();
        sidebar.add_css_class("sidebar");

        let sidebar_header = gtk::Label::new(Some("SESSIONS"));
        sidebar_header.set_halign(gtk::Align::Start);
        sidebar_header.add_css_class("sidebar-header");

        let sidebar_toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .build();
        sidebar_toolbar.add_css_class("sidebar-toolbar");
        let btn_new = gtk::Button::with_label("New");
        let btn_edit = gtk::Button::with_label("Edit");
        let btn_del = gtk::Button::with_label("Del");
        let btn_settings = gtk::Button::with_label("⚙");
        sidebar_toolbar.append(&btn_new);
        sidebar_toolbar.append(&btn_edit);
        sidebar_toolbar.append(&btn_del);
        sidebar_toolbar.append(&btn_settings);

        let connection_list = gtk::ListBox::new();
        connection_list.set_selection_mode(gtk::SelectionMode::Single);
        connection_list.add_css_class("connection-list");

        let connection_scroll = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&connection_list)
            .build();

        let editor_dialog = gtk::Window::builder()
            .title("Session Editor")
            .modal(true)
            .transient_for(&window)
            .default_width(480)
            .default_height(520)
            .resizable(false)
            .build();
        editor_dialog.add_css_class("editor-dialog");

        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        editor.add_css_class("editor-group");

        let draft_name = gtk::Entry::new();
        draft_name.set_placeholder_text(Some("Session name"));
        draft_name.set_hexpand(true);
        let draft_folder = gtk::Entry::new();
        draft_folder.set_placeholder_text(Some("Group folder"));
        draft_folder.set_hexpand(true);

        let draft_host = gtk::Entry::new();
        draft_host.set_placeholder_text(Some("hostname or IP"));
        draft_host.set_hexpand(true);
        let draft_port = gtk::SpinButton::with_range(1.0, 65535.0, 1.0);
        draft_port.set_width_chars(6);

        let draft_user = gtk::Entry::new();
        draft_user.set_placeholder_text(Some("root"));
        draft_user.set_hexpand(true);
        let draft_password = gtk::PasswordEntry::new();
        draft_password.set_hexpand(true);
        let draft_identity = gtk::Entry::new();
        draft_identity.set_placeholder_text(Some("~/.ssh/id_rsa"));
        draft_identity.set_hexpand(true);
        let draft_command = gtk::Entry::new();
        draft_command.set_placeholder_text(Some("optional"));
        draft_command.set_hexpand(true);
        let draft_note = gtk::TextView::new();
        draft_note.set_wrap_mode(gtk::WrapMode::WordChar);
        draft_note.set_vexpand(false);
        let note_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(40)
            .child(&draft_note)
            .build();

        let accept_new_host = gtk::CheckButton::with_label("Accept new host keys");
        let backend_system = gtk::CheckButton::with_label("OpenSSH");
        let backend_wezterm = gtk::CheckButton::with_label("WezTerm");
        backend_wezterm.set_group(Some(&backend_system));

        // === Connection tab ===
        let conn_grid = gtk::Grid::builder()
            .row_spacing(3)
            .column_spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let mut row = 0;
        for (label_text, widget, col_span) in [
            ("Name", draft_name.upcast_ref::<gtk::Widget>(), 3),
            ("Folder", draft_folder.upcast_ref::<gtk::Widget>(), 3),
        ] {
            let lbl = gtk::Label::new(Some(label_text));
            lbl.set_halign(gtk::Align::End);
            lbl.set_valign(gtk::Align::Center);
            conn_grid.attach(&lbl, 0, row, 1, 1);
            conn_grid.attach(widget, 1, row, col_span, 1);
            row += 1;
        }

        let sep1 = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep1.set_margin_top(2);
        sep1.set_margin_bottom(2);
        conn_grid.attach(&sep1, 0, row, 4, 1);
        row += 1;

        let lbl_host = gtk::Label::new(Some("Host"));
        lbl_host.set_halign(gtk::Align::End);
        lbl_host.set_valign(gtk::Align::Center);
        conn_grid.attach(&lbl_host, 0, row, 1, 1);
        conn_grid.attach(&draft_host, 1, row, 1, 1);
        let lbl_port = gtk::Label::new(Some("Port"));
        lbl_port.set_halign(gtk::Align::End);
        lbl_port.set_valign(gtk::Align::Center);
        conn_grid.attach(&lbl_port, 2, row, 1, 1);
        conn_grid.attach(&draft_port, 3, row, 1, 1);
        row += 1;

        for (label_text, widget) in [
            ("User", draft_user.upcast_ref::<gtk::Widget>()),
            ("Password", draft_password.upcast_ref::<gtk::Widget>()),
            ("Key file", draft_identity.upcast_ref::<gtk::Widget>()),
        ] {
            let lbl = gtk::Label::new(Some(label_text));
            lbl.set_halign(gtk::Align::End);
            lbl.set_valign(gtk::Align::Center);
            conn_grid.attach(&lbl, 0, row, 1, 1);
            conn_grid.attach(widget, 1, row, 3, 1);
            row += 1;
        }

        let (term_notebook, draft_terminal) = build_terminal_settings_notebook();

        // === SSH tab ===
        let ssh_grid = gtk::Grid::builder()
            .row_spacing(3)
            .column_spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        row = 0;
        let backend_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        backend_box.append(&backend_system);
        backend_box.append(&backend_wezterm);
        let lbl_backend = gtk::Label::new(Some("Backend"));
        lbl_backend.set_halign(gtk::Align::End);
        lbl_backend.set_valign(gtk::Align::Center);
        ssh_grid.attach(&lbl_backend, 0, row, 1, 1);
        ssh_grid.attach(&backend_box, 1, row, 3, 1);
        row += 1;

        ssh_grid.attach(&accept_new_host, 1, row, 3, 1);
        row += 1;

        let lbl_command = gtk::Label::new(Some("Command"));
        lbl_command.set_halign(gtk::Align::End);
        lbl_command.set_valign(gtk::Align::Center);
        ssh_grid.attach(&lbl_command, 0, row, 1, 1);
        ssh_grid.attach(&draft_command, 1, row, 3, 1);
        row += 1;

        let lbl_notes = gtk::Label::new(Some("Notes"));
        lbl_notes.set_halign(gtk::Align::End);
        lbl_notes.set_valign(gtk::Align::Start);
        ssh_grid.attach(&lbl_notes, 0, row, 1, 1);
        ssh_grid.attach(&note_scroll, 1, row, 3, 1);

        // === Top-level notebook ===
        let editor_notebook = gtk::Notebook::new();
        editor_notebook.set_tab_pos(gtk::PositionType::Top);
        editor_notebook.append_page(&conn_grid, Some(&gtk::Label::new(Some("Connection"))));
        editor_notebook.append_page(&term_notebook, Some(&gtk::Label::new(Some("Terminal"))));
        editor_notebook.append_page(&ssh_grid, Some(&gtk::Label::new(Some("SSH"))));
        editor.append(&editor_notebook);

        let save_draft_btn = gtk::Button::with_label("Save");
        save_draft_btn.add_css_class("connect-button");
        let cancel_draft_btn = gtk::Button::with_label("Cancel");

        let btn_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(4)
            .build();
        btn_row.append(&cancel_draft_btn);
        btn_row.append(&save_draft_btn);
        editor.append(&btn_row);

        let editor_scroll = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&editor)
            .build();

        editor_dialog.set_child(Some(&editor_scroll));

        sidebar.append(&sidebar_header);
        sidebar.append(&sidebar_toolbar);
        sidebar.append(&connection_scroll);
        sidebar_revealer.set_child(Some(&sidebar));

        let right_vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();

        let tab_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(1)
            .build();
        tab_bar.add_css_class("tab-bar");

        let terminal_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        terminal_container.add_css_class("terminal-container");

        let status_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        status_bar.add_css_class("status-bar");
        let status_label = gtk::Label::new(Some("Ready"));
        status_label.set_halign(gtk::Align::Start);
        status_label.set_hexpand(true);
        status_label.add_css_class("status-label");
        status_bar.append(&status_label);

        right_vbox.append(&tab_bar);
        right_vbox.append(&terminal_container);
        right_vbox.append(&status_bar);

        main_paned.set_start_child(Some(&sidebar_revealer));
        main_paned.set_end_child(Some(&right_vbox));

        root_vbox.append(&main_paned);
        window.set_child(Some(&root_vbox));

        {
            let s = sender.clone();
            connect_btn.connect_clicked(move |_| {
                s.input(AppMsg::LaunchSelected);
            });
        }
        {
            let s = sender.clone();
            split_h_btn.connect_clicked(move |_| {
                s.input(AppMsg::SplitHorizontal);
            });
        }
        {
            let s = sender.clone();
            split_v_btn.connect_clicked(move |_| {
                s.input(AppMsg::SplitVertical);
            });
        }
        {
            let s = sender.clone();
            close_pane_btn.connect_clicked(move |_| {
                s.input(AppMsg::ClosePane);
            });
        }
        {
            let s = sender.clone();
            btn_new.connect_clicked(move |_| {
                s.input(AppMsg::NewConnection);
            });
        }
        {
            let s = sender.clone();
            btn_edit.connect_clicked(move |_| {
                s.input(AppMsg::ToggleEditor);
            });
        }
        {
            let s = sender.clone();
            btn_del.connect_clicked(move |_| {
                s.input(AppMsg::DeleteSelected);
            });
        }
        {
            let s = sender.clone();
            btn_settings.connect_clicked(move |_| {
                s.input(AppMsg::OpenGlobalSettings);
            });
        }
        {
            let s = sender.clone();
            save_draft_btn.connect_clicked(move |_| {
                s.input(AppMsg::SaveDraft);
            });
        }
        {
            let s = sender.clone();
            cancel_draft_btn.connect_clicked(move |_| {
                s.input(AppMsg::ToggleEditor);
            });
        }
        {
            let s = sender.clone();
            editor_dialog.connect_close_request(move |_| {
                s.input(AppMsg::ToggleEditor);
                glib::Propagation::Stop
            });
        }
        {
            let s = sender.clone();
            connection_list.connect_row_selected(move |_, row| {
                if let Some(row) = row
                    && let Some(id) = row.tooltip_text()
                    && let Ok(id) = Uuid::parse_str(id.as_str())
                {
                    s.input(AppMsg::SelectConnection(id));
                }
            });
        }
        {
            let s = sender.clone();
            connection_list.connect_row_activated(move |_, row| {
                if let Some(id) = row.tooltip_text()
                    && let Ok(id) = Uuid::parse_str(id.as_str())
                {
                    s.input(AppMsg::SelectConnection(id));
                    s.input(AppMsg::LaunchSelected);
                }
            });
        }

        {
            let s = sender.clone();
            draft_name.connect_changed(move |e| {
                s.input(AppMsg::DraftNameChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_folder.connect_changed(move |e| {
                s.input(AppMsg::DraftFolderChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_host.connect_changed(move |e| {
                s.input(AppMsg::DraftHostChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_port.connect_value_changed(move |e| {
                s.input(AppMsg::DraftPortChanged(e.value() as u16));
            });
        }
        {
            let s = sender.clone();
            draft_user.connect_changed(move |e| {
                s.input(AppMsg::DraftUserChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_password.connect_changed(move |e| {
                s.input(AppMsg::DraftPasswordChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_identity.connect_changed(move |e| {
                s.input(AppMsg::DraftIdentityChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            draft_command.connect_changed(move |e| {
                s.input(AppMsg::DraftCommandChanged(e.text().to_string()));
            });
        }
        {
            let s = sender.clone();
            accept_new_host.connect_toggled(move |b| {
                s.input(AppMsg::DraftAcceptNewHostChanged(b.is_active()));
            });
        }
        {
            let s = sender.clone();
            backend_system.connect_toggled(move |b| {
                if b.is_active() {
                    s.input(AppMsg::DraftBackendChanged(
                        ConnectionBackend::SystemOpenSsh,
                    ));
                }
            });
        }
        {
            let s = sender.clone();
            backend_wezterm.connect_toggled(move |b| {
                if b.is_active() {
                    s.input(AppMsg::DraftBackendChanged(ConnectionBackend::WezTermSsh));
                }
            });
        }
        {
            let s = sender.clone();
            let note_buf = draft_note.buffer();
            note_buf.connect_changed(move |buf| {
                let txt = buf.text(&buf.start_iter(), &buf.end_iter(), true);
                s.input(AppMsg::DraftNoteChanged(txt.to_string()));
            });
        }

        connect_terminal_settings_signals(&draft_terminal, &sender, AppMsg::DraftTerminalChanged);

        let (global_term_notebook, global_terminal) = build_terminal_settings_notebook();

        let theme_list = gtk::StringList::new(&AppTheme::ALL.map(|t| t.label()));
        let theme_dropdown = gtk::DropDown::new(Some(theme_list), gtk::Expression::NONE);
        theme_dropdown.set_selected(
            AppTheme::ALL
                .iter()
                .position(|t| *t == model.global_config.theme)
                .unwrap_or(0) as u32,
        );
        {
            let s = sender.clone();
            theme_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                if idx < AppTheme::ALL.len() {
                    s.input(AppMsg::ThemeChanged(AppTheme::ALL[idx]));
                }
            });
        }

        let theme_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_start(16)
            .margin_end(16)
            .margin_top(12)
            .build();
        let theme_label = gtk::Label::new(Some("Theme"));
        theme_label.set_halign(gtk::Align::End);
        theme_label.set_width_chars(10);
        theme_dropdown.set_hexpand(true);
        theme_row.append(&theme_label);
        theme_row.append(&theme_dropdown);

        let settings_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_bottom(12)
            .margin_start(6)
            .margin_end(6)
            .build();
        settings_content.append(&theme_row);
        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_margin_start(10);
        sep.set_margin_end(10);
        settings_content.append(&sep);
        settings_content.append(&global_term_notebook);

        let btn_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .spacing(8)
            .margin_end(16)
            .margin_bottom(8)
            .build();
        let save_settings_btn = gtk::Button::with_label("Save");
        save_settings_btn.add_css_class("suggested-action");
        btn_box.append(&save_settings_btn);
        settings_content.append(&btn_box);
        {
            let s = sender.clone();
            save_settings_btn.connect_clicked(move |_| {
                s.input(AppMsg::SaveGlobalSettings);
            });
        }

        let settings_dialog = gtk::Window::builder()
            .title("Global Settings")
            .modal(true)
            .transient_for(&window)
            .default_width(480)
            .default_height(400)
            .resizable(false)
            .build();
        settings_dialog.add_css_class("editor-dialog");
        settings_dialog.set_child(Some(&settings_content));
        connect_terminal_settings_signals(&global_terminal, &sender, AppMsg::GlobalTerminalChanged);
        {
            let s = sender.clone();
            settings_dialog.connect_close_request(move |_| {
                s.input(AppMsg::OpenGlobalSettings);
                glib::Propagation::Stop
            });
        }
        {
            let s = sender.clone();
            window.connect_close_request(move |_| {
                s.input(AppMsg::ShutdownAll);
                glib::Propagation::Proceed
            });
        }

        glib::timeout_add_local(std::time::Duration::from_millis(250), {
            let s = sender.clone();
            move || {
                s.input(AppMsg::RefreshSessions);
                glib::ControlFlow::Continue
            }
        });

        let widgets = AppWidgets {
            sidebar_revealer,
            connection_list,
            editor_dialog,
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
            draft_terminal,
            global_terminal,
            settings_dialog,
            connect_btn,
            split_h_btn,
            split_v_btn,
            close_pane_btn,
            tab_bar,
            terminal_container,
            pane_views: Vec::new(),
            pane_sizes: Vec::new(),
            status_label,
            toast_label,
        };

        let mut parts = ComponentParts { model, widgets };
        relm4::SimpleComponent::update_view(&parts.model, &mut parts.widgets, sender);
        parts
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        self.update_impl(message, &sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        self.view_impl(widgets, sender);
    }
}
