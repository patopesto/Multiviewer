mod app;
mod canvas;
mod menu_bar;
mod remote;
mod shortcuts;
mod side_panel;
mod source_settings;
mod viewer;

pub use app::App;
pub(crate) use remote::RemoteHost;

/// Shared egui setup for the native window and the remote session.
pub fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);

    // Enable image/SVG loading for asset icons
    egui_extras::install_image_loaders(ctx);

    // Add "Hack" font to render glyphs (→, ←, ↓, ↑, etc..)
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("Hack".to_owned());
    ctx.set_fonts(fonts);
}
