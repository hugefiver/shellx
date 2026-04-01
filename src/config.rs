use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    #[default]
    Light,
    Dark,
    System,
}

impl AppTheme {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::System => "System",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeleteKeyMode {
    #[default]
    Vt220Del,
    Ascii127,
    Backspace,
}

impl DeleteKeyMode {
    pub const ALL: [Self; 3] = [Self::Vt220Del, Self::Ascii127, Self::Backspace];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vt220Del => "VT220 Del (Esc[3~)",
            Self::Ascii127 => "ASCII 127 (Ctrl+?)",
            Self::Backspace => "Backspace (Ctrl+H)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackspaceKeyMode {
    Vt220Del,
    Ascii127,
    #[default]
    CtrlH,
}

impl BackspaceKeyMode {
    pub const ALL: [Self; 3] = [Self::Vt220Del, Self::Ascii127, Self::CtrlH];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vt220Del => "VT220 Del (Esc[3~)",
            Self::Ascii127 => "ASCII 127 (Ctrl+?)",
            Self::CtrlH => "Backspace (Ctrl+H)",
        }
    }
}

pub const TERMINAL_TYPES: &[&str] = &[
    "xterm-256color",
    "xterm",
    "linux",
    "vt100",
    "vt220",
    "screen",
    "screen-256color",
    "tmux",
    "tmux-256color",
    "rxvt-unicode-256color",
];

/// Per-session terminal settings. All fields are `Option` so that unset fields
/// fall back to the global defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_cols: Option<u16>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_rows: Option<u16>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback_lines: Option<usize>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_key: Option<DeleteKeyMode>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backspace_key: Option<BackspaceKeyMode>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_alt_as_meta: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_alt_as_meta: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_csi_u: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_kitty_keyboard: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_kitty_graphics: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_reporting: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_on_output: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_on_keypress: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answerback: Option<String>,
}

impl TerminalSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Merge this settings with a set of defaults. Per-session values take
    /// precedence; anything unset falls through to `defaults`.
    pub fn merge_over(&self, defaults: &Self) -> Self {
        Self {
            terminal_type: self
                .terminal_type
                .clone()
                .or_else(|| defaults.terminal_type.clone()),
            initial_cols: self.initial_cols.or(defaults.initial_cols),
            initial_rows: self.initial_rows.or(defaults.initial_rows),
            scrollback_lines: self.scrollback_lines.or(defaults.scrollback_lines),
            delete_key: self.delete_key.or(defaults.delete_key),
            backspace_key: self.backspace_key.or(defaults.backspace_key),
            left_alt_as_meta: self.left_alt_as_meta.or(defaults.left_alt_as_meta),
            right_alt_as_meta: self.right_alt_as_meta.or(defaults.right_alt_as_meta),
            enable_csi_u: self.enable_csi_u.or(defaults.enable_csi_u),
            enable_kitty_keyboard: self
                .enable_kitty_keyboard
                .or(defaults.enable_kitty_keyboard),
            enable_kitty_graphics: self
                .enable_kitty_graphics
                .or(defaults.enable_kitty_graphics),
            mouse_reporting: self.mouse_reporting.or(defaults.mouse_reporting),
            scroll_on_output: self.scroll_on_output.or(defaults.scroll_on_output),
            scroll_on_keypress: self.scroll_on_keypress.or(defaults.scroll_on_keypress),
            answerback: self
                .answerback
                .clone()
                .or_else(|| defaults.answerback.clone()),
        }
    }

    /// Resolve all `Option` fields to concrete values, using hardcoded defaults
    /// for anything still unset.
    pub fn resolve(&self) -> ResolvedTerminalSettings {
        ResolvedTerminalSettings {
            terminal_type: self
                .terminal_type
                .clone()
                .unwrap_or_else(|| "xterm-256color".into()),
            initial_cols: self.initial_cols.unwrap_or(120),
            initial_rows: self.initial_rows.unwrap_or(36),
            scrollback_lines: self.scrollback_lines.unwrap_or(6_000),
            delete_key: self.delete_key.unwrap_or_default(),
            backspace_key: self.backspace_key.unwrap_or_default(),
            left_alt_as_meta: self.left_alt_as_meta.unwrap_or(true),
            right_alt_as_meta: self.right_alt_as_meta.unwrap_or(true),
            enable_csi_u: self.enable_csi_u.unwrap_or(false),
            enable_kitty_keyboard: self.enable_kitty_keyboard.unwrap_or(false),
            enable_kitty_graphics: self.enable_kitty_graphics.unwrap_or(false),
            mouse_reporting: self.mouse_reporting.unwrap_or(true),
            scroll_on_output: self.scroll_on_output.unwrap_or(true),
            scroll_on_keypress: self.scroll_on_keypress.unwrap_or(false),
            answerback: self.answerback.clone().unwrap_or_else(|| "rsHell".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTerminalSettings {
    pub terminal_type: String,
    pub initial_cols: u16,
    pub initial_rows: u16,
    pub scrollback_lines: usize,
    pub delete_key: DeleteKeyMode,
    pub backspace_key: BackspaceKeyMode,
    pub left_alt_as_meta: bool,
    pub right_alt_as_meta: bool,
    pub enable_csi_u: bool,
    pub enable_kitty_keyboard: bool,
    pub enable_kitty_graphics: bool,
    pub mouse_reporting: bool,
    pub scroll_on_output: bool,
    pub scroll_on_keypress: bool,
    pub answerback: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub theme: AppTheme,

    #[serde(default)]
    pub terminal: TerminalSettings,
}

#[derive(Debug, Clone)]
pub struct SettingsRepository {
    path: PathBuf,
}

impl Default for SettingsRepository {
    fn default() -> Self {
        Self::new(default_settings_path())
    }
}

impl SettingsRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<GlobalConfig> {
        if !self.path.exists() {
            let config = GlobalConfig::default();
            self.save(&config)?;
            return Ok(config);
        }

        let data = std::fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let config: GlobalConfig = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse {}", self.path.display()))?;
        Ok(config)
    }

    pub fn save(&self, config: &GlobalConfig) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create settings directory {}", parent.display())
            })?;
        }

        let data = serde_json::to_string_pretty(config).context("failed to encode JSON")?;
        std::fs::write(&self.path, &data)
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }
}

fn default_settings_path() -> PathBuf {
    let base = dirs::config_local_dir()
        .or_else(dirs::config_dir)
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    base.join("rshell").join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resolve_produces_expected_values() {
        let settings = TerminalSettings::default();
        let resolved = settings.resolve();
        assert_eq!(resolved.terminal_type, "xterm-256color");
        assert_eq!(resolved.initial_cols, 120);
        assert_eq!(resolved.initial_rows, 36);
        assert_eq!(resolved.scrollback_lines, 6_000);
        assert!(resolved.left_alt_as_meta);
        assert!(resolved.mouse_reporting);
        assert!(!resolved.enable_csi_u);
    }

    #[test]
    fn merge_over_prefers_session_values() {
        let global = TerminalSettings {
            scrollback_lines: Some(3_000),
            terminal_type: Some("linux".into()),
            ..Default::default()
        };
        let session = TerminalSettings {
            scrollback_lines: Some(10_000),
            ..Default::default()
        };
        let merged = session.merge_over(&global);
        assert_eq!(merged.scrollback_lines, Some(10_000));
        assert_eq!(merged.terminal_type, Some("linux".into()));
    }

    #[test]
    fn merge_over_fills_unset_from_defaults() {
        let global = TerminalSettings {
            initial_cols: Some(80),
            initial_rows: Some(24),
            ..Default::default()
        };
        let session = TerminalSettings::default();
        let merged = session.merge_over(&global);
        assert_eq!(merged.initial_cols, Some(80));
        assert_eq!(merged.initial_rows, Some(24));
    }

    #[test]
    fn empty_settings_detected() {
        assert!(TerminalSettings::default().is_empty());
        let s = TerminalSettings {
            scrollback_lines: Some(1000),
            ..Default::default()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn repository_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "rshell-settings-test-{}.json",
            uuid::Uuid::new_v4()
        ));
        let repo = SettingsRepository::new(&path);

        let mut config = GlobalConfig::default();
        config.terminal.scrollback_lines = Some(5_000);
        config.terminal.terminal_type = Some("linux".into());

        repo.save(&config).unwrap();
        let loaded = repo.load().unwrap();

        assert_eq!(
            loaded.terminal.scrollback_lines,
            config.terminal.scrollback_lines
        );
        assert_eq!(loaded.terminal.terminal_type, config.terminal.terminal_type);

        let _ = std::fs::remove_file(path);
    }
}
