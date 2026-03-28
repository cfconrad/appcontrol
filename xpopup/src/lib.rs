use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Rounding, Stroke, Vec2};

pub struct XPopup {
    message: String,
    background_texture: Option<egui::TextureHandle>,
    bg_image_path: Option<String>,
    texture_loaded: bool,
}

impl XPopup {
    pub fn new(_cc: &eframe::CreationContext<'_>, message: String, bg_image_path: Option<String>) -> Self {
        Self {
            message,
            background_texture: None,
            bg_image_path,
            texture_loaded: false,
        }
    }

    fn load_texture(&mut self, ctx: &egui::Context) {
        if self.texture_loaded {
            return;
        }
        self.texture_loaded = true;

        if let Some(path) = self.bg_image_path.clone() {
            match image::open(&path) {
                Ok(img) => {
                    let img = img.into_rgba8();
                    let (width, height) = img.dimensions();
                    let pixels = img.into_raw();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &pixels,
                    );
                    self.background_texture = Some(ctx.load_texture(
                        "background",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                Err(e) => {
                    eprintln!("Warning: could not load background image '{}': {}", path, e);
                }
            }
        }
    }
}

impl eframe::App for XPopup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Prevent any attempt to close the window
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        // Keep the window always on top and fullscreen
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);

        self.load_texture(ctx);

        let screen_rect = ctx.screen_rect();

        // Paint background
        let bg_painter = ctx.layer_painter(egui::LayerId::background());

        if let Some(texture) = &self.background_texture {
            let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            bg_painter.image(texture.id(), screen_rect, uv, Color32::WHITE);
        } else {
            // Fallback dark background when no image is provided
            bg_painter.rect_filled(screen_rect, 0.0, Color32::from_rgb(20, 25, 40));
            // Subtle lighter top half for visual depth
            let top_half = Rect::from_min_max(
                screen_rect.min,
                Pos2::new(screen_rect.max.x, screen_rect.center().y),
            );
            bg_painter.rect_filled(top_half, 0.0, Color32::from_rgba_unmultiplied(50, 60, 90, 60));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let center = screen_rect.center();
                let square_size = Vec2::new(520.0, 300.0);
                let square_rect = Rect::from_center_size(center, square_size);

                // Draw the black center square
                let painter = ui.painter();
                painter.rect(
                    square_rect,
                    Rounding::same(10.0),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 220),
                    Stroke::new(1.5, Color32::from_gray(60)),
                );

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(square_rect), |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.add_space(60.0);

                        // Message in white
                        ui.label(
                            RichText::new(&self.message)
                                .color(Color32::WHITE)
                                .size(22.0),
                        );

                        ui.add_space(50.0);

                        // Buttons row
                        ui.horizontal(|ui| {
                            let button_width = 120.0;
                            let spacing = 30.0;
                            let total_buttons = button_width * 2.0 + spacing;
                            let available = ui.available_width();
                            ui.add_space((available - total_buttons) / 2.0);

                            let close_btn = egui::Button::new(
                                RichText::new("close")
                                    .color(Color32::WHITE)
                                    .size(16.0),
                            )
                            .fill(Color32::from_rgb(160, 40, 40))
                            .min_size(Vec2::new(button_width, 42.0));

                            if ui.add(close_btn).clicked() {
                                std::process::exit(2);
                            }

                            ui.add_space(spacing);

                            let later_btn = egui::Button::new(
                                RichText::new("later")
                                    .color(Color32::WHITE)
                                    .size(16.0),
                            )
                            .fill(Color32::from_rgb(40, 80, 160))
                            .min_size(Vec2::new(button_width, 42.0));

                            if ui.add(later_btn).clicked() {
                                std::process::exit(0);
                            }
                        });
                    });
                });
            });
    }
}

pub fn run_popup(message: String, bg_image_path: Option<String>) {
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
        Box::new(|cc| Ok(Box::new(XPopup::new(cc, message, bg_image_path)))),
    )
    .unwrap();
}
