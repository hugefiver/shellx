use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use wezterm_term::color::{ColorPalette, Palette256, RgbColor, SrgbaTuple};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    Default,
    OneDark,
    SolarizedDark,
    SolarizedLight,
    Dracula,
    Monokai,
    Nord,
    GruvboxDark,
    TokyoNight,
    CampbellPowershell,
}

impl ColorScheme {
    pub const ALL: [Self; 10] = [
        Self::Default,
        Self::OneDark,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::Dracula,
        Self::Monokai,
        Self::Nord,
        Self::GruvboxDark,
        Self::TokyoNight,
        Self::CampbellPowershell,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default (XTerm)",
            Self::OneDark => "One Dark",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::Dracula => "Dracula",
            Self::Monokai => "Monokai",
            Self::Nord => "Nord",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::TokyoNight => "Tokyo Night",
            Self::CampbellPowershell => "Campbell PowerShell",
        }
    }

    /// Build a full `ColorPalette` for this scheme. Only the first 16 ANSI
    /// colors, foreground, background, cursor and selection colours are
    /// overridden – colours 16-255 (216-cube + 24 greys) are inherited from
    /// the default xterm palette so that 256-color apps still work.
    pub fn build_palette(self) -> ColorPalette {
        if self == Self::Default {
            return ColorPalette::default();
        }

        let def = ColorPalette::default();
        let mut colors = def.colors.0;

        // Scheme data: (ansi[16], fg, bg, cursor_bg, selection_bg)
        let (ansi, fg, bg, cursor_bg, sel_bg) = self.scheme_colors();

        for (i, &(r, g, b)) in ansi.iter().enumerate() {
            colors[i] = RgbColor::new_8bpc(r, g, b).into();
        }

        let fg: SrgbaTuple = RgbColor::new_8bpc(fg.0, fg.1, fg.2).into();
        let bg: SrgbaTuple = RgbColor::new_8bpc(bg.0, bg.1, bg.2).into();
        let cursor: SrgbaTuple = RgbColor::new_8bpc(cursor_bg.0, cursor_bg.1, cursor_bg.2).into();
        let sel: SrgbaTuple = RgbColor::new_8bpc(sel_bg.0, sel_bg.1, sel_bg.2).into();

        ColorPalette {
            colors: Palette256(colors),
            foreground: fg,
            background: bg,
            cursor_fg: bg, // cursor text = background for contrast
            cursor_bg: cursor,
            cursor_border: cursor,
            selection_fg: SrgbaTuple(0., 0., 0., 0.), // transparent = auto
            selection_bg: SrgbaTuple(sel.0, sel.1, sel.2, 0.5),
            scrollbar_thumb: def.scrollbar_thumb,
            split: def.split,
        }
    }

    /// Returns (ansi_16_colors, foreground, background, cursor, selection)
    /// as (r, g, b) tuples in 0-255 range.
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    fn scheme_colors(self) -> ([(u8, u8, u8); 16], (u8, u8, u8), (u8, u8, u8), (u8, u8, u8), (u8, u8, u8)) {
        match self {
            Self::Default => unreachable!(),

            Self::OneDark => (
                [
                    (0x28, 0x2c, 0x34), // black
                    (0xe0, 0x6c, 0x75), // red
                    (0x98, 0xc3, 0x79), // green
                    (0xe5, 0xc0, 0x7b), // yellow
                    (0x61, 0xaf, 0xef), // blue
                    (0xc6, 0x78, 0xdd), // magenta
                    (0x56, 0xb6, 0xc2), // cyan
                    (0xab, 0xb2, 0xbf), // white
                    (0x54, 0x58, 0x62), // bright black
                    (0xe0, 0x6c, 0x75), // bright red
                    (0x98, 0xc3, 0x79), // bright green
                    (0xe5, 0xc0, 0x7b), // bright yellow
                    (0x61, 0xaf, 0xef), // bright blue
                    (0xc6, 0x78, 0xdd), // bright magenta
                    (0x56, 0xb6, 0xc2), // bright cyan
                    (0xff, 0xff, 0xff), // bright white
                ],
                (0xab, 0xb2, 0xbf), // fg
                (0x28, 0x2c, 0x34), // bg
                (0x52, 0x8b, 0xff), // cursor
                (0x3e, 0x4a, 0x5b), // selection
            ),

            Self::SolarizedDark => (
                [
                    (0x07, 0x36, 0x42), // base02
                    (0xdc, 0x32, 0x2f), // red
                    (0x85, 0x99, 0x00), // green
                    (0xb5, 0x89, 0x00), // yellow
                    (0x26, 0x8b, 0xd2), // blue
                    (0xd3, 0x36, 0x82), // magenta
                    (0x2a, 0xa1, 0x98), // cyan
                    (0xee, 0xe8, 0xd5), // base2
                    (0x00, 0x2b, 0x36), // base03
                    (0xcb, 0x4b, 0x16), // orange
                    (0x58, 0x6e, 0x75), // base01
                    (0x65, 0x7b, 0x83), // base00
                    (0x83, 0x94, 0x96), // base0
                    (0x6c, 0x71, 0xc4), // violet
                    (0x93, 0xa1, 0xa1), // base1
                    (0xfd, 0xf6, 0xe3), // base3
                ],
                (0x83, 0x94, 0x96), // fg (base0)
                (0x00, 0x2b, 0x36), // bg (base03)
                (0x83, 0x94, 0x96), // cursor
                (0x07, 0x36, 0x42), // selection (base02)
            ),

            Self::SolarizedLight => (
                [
                    (0xee, 0xe8, 0xd5), // base2
                    (0xdc, 0x32, 0x2f), // red
                    (0x85, 0x99, 0x00), // green
                    (0xb5, 0x89, 0x00), // yellow
                    (0x26, 0x8b, 0xd2), // blue
                    (0xd3, 0x36, 0x82), // magenta
                    (0x2a, 0xa1, 0x98), // cyan
                    (0x07, 0x36, 0x42), // base02
                    (0xfd, 0xf6, 0xe3), // base3
                    (0xcb, 0x4b, 0x16), // orange
                    (0x93, 0xa1, 0xa1), // base1
                    (0x83, 0x94, 0x96), // base0
                    (0x65, 0x7b, 0x83), // base00
                    (0x6c, 0x71, 0xc4), // violet
                    (0x58, 0x6e, 0x75), // base01
                    (0x00, 0x2b, 0x36), // base03
                ],
                (0x65, 0x7b, 0x83), // fg (base00)
                (0xfd, 0xf6, 0xe3), // bg (base3)
                (0x65, 0x7b, 0x83), // cursor
                (0xee, 0xe8, 0xd5), // selection (base2)
            ),

            Self::Dracula => (
                [
                    (0x21, 0x22, 0x2c), // black
                    (0xff, 0x55, 0x55), // red
                    (0x50, 0xfa, 0x7b), // green
                    (0xf1, 0xfa, 0x8c), // yellow
                    (0xbd, 0x93, 0xf9), // blue/purple
                    (0xff, 0x79, 0xc6), // magenta/pink
                    (0x8b, 0xe9, 0xfd), // cyan
                    (0xf8, 0xf8, 0xf2), // white
                    (0x62, 0x72, 0xa4), // bright black (comment)
                    (0xff, 0x6e, 0x6e), // bright red
                    (0x69, 0xff, 0x94), // bright green
                    (0xff, 0xff, 0xa5), // bright yellow
                    (0xd6, 0xac, 0xff), // bright blue
                    (0xff, 0x92, 0xdf), // bright magenta
                    (0xa4, 0xff, 0xff), // bright cyan
                    (0xff, 0xff, 0xff), // bright white
                ],
                (0xf8, 0xf8, 0xf2), // fg
                (0x28, 0x2a, 0x36), // bg
                (0xf8, 0xf8, 0xf2), // cursor
                (0x44, 0x47, 0x5a), // selection
            ),

            Self::Monokai => (
                [
                    (0x27, 0x28, 0x22), // black
                    (0xf9, 0x26, 0x72), // red
                    (0xa6, 0xe2, 0x2e), // green
                    (0xf4, 0xbf, 0x75), // yellow
                    (0x66, 0xd9, 0xef), // blue
                    (0xae, 0x81, 0xff), // magenta
                    (0xa1, 0xef, 0xe4), // cyan
                    (0xf8, 0xf8, 0xf2), // white
                    (0x75, 0x71, 0x5e), // bright black
                    (0xf9, 0x26, 0x72), // bright red
                    (0xa6, 0xe2, 0x2e), // bright green
                    (0xf4, 0xbf, 0x75), // bright yellow
                    (0x66, 0xd9, 0xef), // bright blue
                    (0xae, 0x81, 0xff), // bright magenta
                    (0xa1, 0xef, 0xe4), // bright cyan
                    (0xf9, 0xf8, 0xf5), // bright white
                ],
                (0xf8, 0xf8, 0xf2), // fg
                (0x27, 0x28, 0x22), // bg
                (0xf8, 0xf8, 0xf0), // cursor
                (0x49, 0x48, 0x3e), // selection
            ),

            Self::Nord => (
                [
                    (0x3b, 0x42, 0x52), // nord1
                    (0xbf, 0x61, 0x6a), // nord11
                    (0xa3, 0xbe, 0x8c), // nord14
                    (0xeb, 0xcb, 0x8b), // nord13
                    (0x81, 0xa1, 0xc1), // nord9
                    (0xb4, 0x8e, 0xad), // nord15
                    (0x88, 0xc0, 0xd0), // nord8
                    (0xe5, 0xe9, 0xf0), // nord5
                    (0x4c, 0x56, 0x6a), // nord3
                    (0xbf, 0x61, 0x6a), // nord11
                    (0xa3, 0xbe, 0x8c), // nord14
                    (0xeb, 0xcb, 0x8b), // nord13
                    (0x81, 0xa1, 0xc1), // nord9
                    (0xb4, 0x8e, 0xad), // nord15
                    (0x8f, 0xbc, 0xbb), // nord7
                    (0xec, 0xef, 0xf4), // nord6
                ],
                (0xd8, 0xde, 0xe9), // fg (nord4)
                (0x2e, 0x34, 0x40), // bg (nord0)
                (0xd8, 0xde, 0xe9), // cursor
                (0x43, 0x4c, 0x5e), // selection (nord2)
            ),

            Self::GruvboxDark => (
                [
                    (0x28, 0x28, 0x28), // bg
                    (0xcc, 0x24, 0x1d), // red
                    (0x98, 0x97, 0x1a), // green
                    (0xd7, 0x99, 0x21), // yellow
                    (0x45, 0x85, 0x88), // blue
                    (0xb1, 0x62, 0x86), // purple
                    (0x68, 0x9d, 0x6a), // aqua
                    (0xa8, 0x99, 0x84), // fg4
                    (0x92, 0x83, 0x74), // grey
                    (0xfb, 0x49, 0x34), // bright red
                    (0xb8, 0xbb, 0x26), // bright green
                    (0xfa, 0xbd, 0x2f), // bright yellow
                    (0x83, 0xa5, 0x98), // bright blue
                    (0xd3, 0x86, 0x9b), // bright purple
                    (0x8e, 0xc0, 0x7c), // bright aqua
                    (0xeb, 0xdb, 0xb2), // fg
                ],
                (0xeb, 0xdb, 0xb2), // fg
                (0x28, 0x28, 0x28), // bg
                (0xeb, 0xdb, 0xb2), // cursor
                (0x50, 0x49, 0x45), // selection (bg2)
            ),

            Self::TokyoNight => (
                [
                    (0x15, 0x16, 0x1e), // black
                    (0xf7, 0x76, 0x8e), // red
                    (0x9e, 0xce, 0x6a), // green
                    (0xe0, 0xaf, 0x68), // yellow
                    (0x7a, 0xa2, 0xf7), // blue
                    (0xbb, 0x9a, 0xf7), // magenta
                    (0x7d, 0xcf, 0xff), // cyan
                    (0xa9, 0xb1, 0xd6), // white
                    (0x41, 0x48, 0x68), // bright black
                    (0xf7, 0x76, 0x8e), // bright red
                    (0x9e, 0xce, 0x6a), // bright green
                    (0xe0, 0xaf, 0x68), // bright yellow
                    (0x7a, 0xa2, 0xf7), // bright blue
                    (0xbb, 0x9a, 0xf7), // bright magenta
                    (0x7d, 0xcf, 0xff), // bright cyan
                    (0xc0, 0xca, 0xf5), // bright white
                ],
                (0xa9, 0xb1, 0xd6), // fg
                (0x1a, 0x1b, 0x26), // bg
                (0xc0, 0xca, 0xf5), // cursor
                (0x28, 0x3b, 0x70), // selection
            ),

            Self::CampbellPowershell => (
                [
                    (0x0c, 0x0c, 0x0c), // black
                    (0xc5, 0x0f, 0x1f), // red
                    (0x13, 0xa1, 0x0e), // green
                    (0xc1, 0x9c, 0x00), // yellow
                    (0x00, 0x37, 0xda), // blue
                    (0x88, 0x17, 0x98), // magenta
                    (0x3a, 0x96, 0xdd), // cyan
                    (0xcc, 0xcc, 0xcc), // white
                    (0x76, 0x76, 0x76), // bright black
                    (0xe7, 0x48, 0x56), // bright red
                    (0x16, 0xc6, 0x0c), // bright green
                    (0xf9, 0xf1, 0xa5), // bright yellow
                    (0x3b, 0x78, 0xff), // bright blue
                    (0xb4, 0x00, 0x9e), // bright magenta
                    (0x61, 0xd6, 0xd6), // bright cyan
                    (0xf2, 0xf2, 0xf2), // bright white
                ],
                (0xcc, 0xcc, 0xcc), // fg
                (0x01, 0x24, 0x56), // bg (PS blue)
                (0xcc, 0xcc, 0xcc), // cursor
                (0x13, 0x44, 0x7e), // selection
            ),
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<ColorScheme>,
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
            color_scheme: self.color_scheme.or(defaults.color_scheme),
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
            color_scheme: self.color_scheme.unwrap_or_default(),
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
    pub color_scheme: ColorScheme,
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
