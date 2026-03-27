use gtk::{gdk, CssProvider};
use std::sync::OnceLock;
use tracing_subscriber::EnvFilter;

static LOGGING_INIT: OnceLock<()> = OnceLock::new();

pub fn init_logging() {
    LOGGING_INIT.get_or_init(|| {
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("shellx=info,wezterm_ssh=info"));

        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .compact()
            .try_init();
    });
}

pub fn register_resources() {}

pub fn apply_global_css() {
    let display = gdk::Display::default().expect("GTK display is not available");
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../resources/style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
