use eframe::egui;

use crate::ui::{PopupState, popup_ui};

/// Grab both keyboard and pointer on the currently focused X11 window.
/// Returns the grabbed window ID on success, 0 on failure.
fn x11_grab_all() -> u32 {
    use x11rb::protocol::xproto::{ConnectionExt, EventMask, GrabMode, GrabStatus, Time};

    let (conn, _) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Identify the focused window.
    let focus = match conn.get_input_focus().ok().and_then(|c| c.reply().ok()) {
        Some(r) if r.focus > 1 => r.focus,
        _ => return 0,
    };

    // Grab keyboard.
    let kb_ok = conn
        .grab_keyboard(
            true,
            focus,
            Time::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.status == GrabStatus::SUCCESS)
        .unwrap_or(false);

    if !kb_ok {
        return 0;
    }

    // Grab pointer.
    let ptr_ok = conn
        .grab_pointer(
            true,
            focus,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            0u32, // confine_to: none
            0u32, // cursor: none
            Time::CURRENT_TIME,
        )
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.status == GrabStatus::SUCCESS)
        .unwrap_or(false);

    if ptr_ok {
        focus
    } else {
        let _ = conn.ungrab_keyboard(Time::CURRENT_TIME);
        0
    }
}

struct XPopup {
    state: PopupState,
    grabbed_window: u32,
}

impl XPopup {
    fn new(
        _cc: &eframe::CreationContext<'_>,
        message: String,
        bg_image_path: Option<String>,
        warn_only: bool,
        timeout_secs: Option<u64>,
    ) -> Self {
        Self {
            state: PopupState::new(message, bg_image_path, warn_only, timeout_secs),
            grabbed_window: 0,
        }
    }
}

impl eframe::App for XPopup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.load_texture(ctx);

        // Grab keyboard + pointer once (retry every frame until it succeeds).
        if self.grabbed_window == 0 {
            self.grabbed_window = x11_grab_all();
        }

        if let Some(code) = popup_ui(ctx, &mut self.state) {
            std::process::exit(code);
        }

        // Re-render regularly so the countdown label ticks down.
        if self.state.deadline.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

pub(crate) fn run_x11(
    message: String,
    bg_image_path: Option<String>,
    warn_only: bool,
    timeout_secs: Option<u64>,
) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_always_on_top()
            .with_decorations(false)
            .with_title("xpopup"),
        ..Default::default()
    };
    eframe::run_native(
        "xpopup",
        options,
        Box::new(|cc| {
            Ok(Box::new(XPopup::new(
                cc,
                message,
                bg_image_path,
                warn_only,
                timeout_secs,
            )))
        }),
    )
    .unwrap();
}
