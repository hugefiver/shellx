use crate::config::AppTheme;
use gtk::{gdk, CssProvider};
use std::cell::OnceCell;

thread_local! {
    static PROVIDER: OnceCell<CssProvider> = const { OnceCell::new() };
}

fn ensure_css_loaded() {
    PROVIDER.with(|cell| {
        cell.get_or_init(|| {
            let provider = CssProvider::new();
            provider.load_from_string(include_str!("../resources/style.css"));
            let display = gdk::Display::default().expect("GTK display is not available");
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            provider
        });
    });
}

pub fn apply_theme(theme: AppTheme) {
    ensure_css_loaded();
    let display = gdk::Display::default().expect("GTK display is not available");
    let settings = gtk::Settings::for_display(&display);

    match theme {
        AppTheme::Light => settings.set_gtk_application_prefer_dark_theme(false),
        AppTheme::Dark => settings.set_gtk_application_prefer_dark_theme(true),
        AppTheme::System => settings.reset_property("gtk-application-prefer-dark-theme"),
    }
}
