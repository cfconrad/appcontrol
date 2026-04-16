mod ui;
mod wayland;
mod x11;

pub fn run_popup(message: String, bg_image_path: Option<String>, warn_only: bool) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        wayland::run_wayland_locked(message, bg_image_path, warn_only);
    } else {
        x11::run_x11(message, bg_image_path, warn_only);
    }
}
