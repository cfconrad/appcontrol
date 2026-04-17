use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Rounding, Stroke, Vec2};

const BASE_SIZE: f32 = 18.0;
const HEAD_SIZE: f32 = BASE_SIZE * 1.4;
const BOLD_COLOR: Color32 = Color32::WHITE;
const NORMAL_COLOR: Color32 = Color32::from_gray(210);

fn parse_inline(text: &str) -> Vec<(&str, bool)> {
    let mut segments = Vec::new();
    let mut remaining = text;
    let mut bold = false;
    while let Some(pos) = remaining.find("**") {
        if pos > 0 {
            segments.push((&remaining[..pos], bold));
        }
        bold = !bold;
        remaining = &remaining[pos + 2..];
    }
    if !remaining.is_empty() {
        segments.push((remaining, bold));
    }
    segments
}

fn inline_job(text: &str, wrap_width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for (seg, bold) in parse_inline(text) {
        job.append(
            seg,
            0.0,
            egui::text::TextFormat {
                font_id: egui::FontId::proportional(BASE_SIZE),
                color: if bold { BOLD_COLOR } else { NORMAL_COLOR },
                ..Default::default()
            },
        );
    }
    job
}

fn render_message(ui: &mut egui::Ui, message: &str) {
    let lines: Vec<&str> = message.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            ui.add_space(6.0);
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let text = rest.trim().replace("**", "");
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.label(RichText::new(text).size(HEAD_SIZE).color(BOLD_COLOR));
            });
            i += 1;
            continue;
        }
        if line.starts_with('|') {
            let mut table_lines: Vec<&str> = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                table_lines.push(lines[i].trim());
                i += 1;
            }
            render_table(ui, &table_lines);
            continue;
        }
        let avail = ui.available_width();
        let job = inline_job(line, avail);
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.label(job);
        });
        i += 1;
    }
}

fn render_table(ui: &mut egui::Ui, lines: &[&str]) {
    let data: Vec<Vec<&str>> = lines
        .iter()
        .filter(|l| !l.contains("---"))
        .map(|l| {
            l.split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .collect();
    if data.is_empty() {
        return;
    }
    egui::Grid::new("msg_table")
        .striped(true)
        .min_col_width(80.0)
        .show(ui, |ui| {
            for (row_idx, cells) in data.iter().enumerate() {
                for cell in cells {
                    let text = if row_idx == 0 {
                        RichText::new(*cell).strong().color(Color32::WHITE)
                    } else {
                        RichText::new(*cell).color(Color32::WHITE)
                    };
                    ui.label(text);
                }
                ui.end_row();
            }
        });
}

pub(crate) struct PopupState {
    pub message: String,
    pub warn_only: bool,
    pub bg_image_path: Option<String>,
    pub background_texture: Option<egui::TextureHandle>,
    pub cached_content_height: f32,
    pub deadline: Option<std::time::Instant>,
}

impl PopupState {
    pub fn new(
        message: String,
        bg_image_path: Option<String>,
        warn_only: bool,
        timeout_secs: Option<u64>,
    ) -> Self {
        Self {
            message,
            warn_only,
            bg_image_path,
            background_texture: None,
            cached_content_height: 200.0,
            deadline: timeout_secs
                .map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s)),
        }
    }

    pub fn load_texture(&mut self, ctx: &egui::Context) {
        if self.background_texture.is_some() {
            return;
        }
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
                    eprintln!("xpopup: could not load background image '{}': {}", path, e);
                }
            }
        }
    }
}

/// Render the popup UI. Returns `Some(exit_code)` when the user clicks a button.
pub(crate) fn popup_ui(ctx: &egui::Context, state: &mut PopupState) -> Option<i32> {
    let screen_rect = ctx.screen_rect();

    const FRAME_WIDTH: f32 = 600.0;
    const PAD_TOP: f32 = 60.0;
    const PAD_MID: f32 = 50.0;
    const BTN_H: f32 = 42.0;
    const PAD_BOT: f32 = 40.0;

    let frame_height = PAD_TOP + state.cached_content_height + PAD_MID + BTN_H + PAD_BOT;

    // Background
    let bg_painter = ctx.layer_painter(egui::LayerId::background());
    if let Some(texture) = &state.background_texture {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        bg_painter.image(texture.id(), screen_rect, uv, Color32::WHITE);
    } else {
        bg_painter.rect_filled(screen_rect, 0.0, Color32::from_rgb(50, 50, 50));
    }

    // Auto-close when the deadline has passed.
    if let Some(dl) = state.deadline {
        if std::time::Instant::now() >= dl {
            return Some(2);
        }
    }

    let mut exit_code: Option<i32> = None;

    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let center = screen_rect.center();
            let square_rect = Rect::from_center_size(center, Vec2::new(FRAME_WIDTH, frame_height));

            ui.painter().rect(
                square_rect,
                Rounding::same(10.0),
                Color32::from_rgb(35, 35, 35),
                Stroke::new(1.5, Color32::from_rgb(200, 35, 35)),
            );

            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(square_rect), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.add_space(PAD_TOP);

                    let content = egui::Frame::none()
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            render_message(ui, &state.message.clone());
                        });
                    state.cached_content_height = content.response.rect.height();

                    ui.add_space(PAD_MID);

                    // Countdown label
                    if let Some(dl) = state.deadline {
                        let secs_left = dl
                            .saturating_duration_since(std::time::Instant::now())
                            .as_secs();
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!("Closing in {secs_left}s"))
                                    .size(BASE_SIZE)
                                    .color(Color32::from_rgb(230, 140, 30)),
                            );
                            ui.add_space(8.0);
                        });
                    }

                    ui.horizontal(|ui| {
                        let button_width = 120.0;
                        let n_buttons = if state.warn_only { 1 } else { 2 };
                        let spacing = 30.0;
                        let total =
                            button_width * n_buttons as f32 + spacing * (n_buttons - 1) as f32;
                        ui.add_space((ui.available_width() - total) / 2.0);

                        let close_btn = egui::Button::new(
                            RichText::new("Close Application")
                                .color(Color32::WHITE)
                                .size(16.0),
                        )
                        .fill(Color32::from_rgb(160, 40, 40))
                        .min_size(Vec2::new(button_width, BTN_H));

                        if ui.add(close_btn).clicked() {
                            exit_code = Some(2);
                        }

                        if !state.warn_only {
                            ui.add_space(spacing);

                            let later_btn = egui::Button::new(
                                RichText::new("Let me finish")
                                    .color(Color32::WHITE)
                                    .size(16.0),
                            )
                            .fill(Color32::from_rgb(40, 80, 160))
                            .min_size(Vec2::new(button_width, BTN_H));

                            if ui.add(later_btn).clicked() {
                                exit_code = Some(0);
                            }
                        }
                    });

                    ui.add_space(PAD_BOT);
                });
            });
        });

    exit_code
}
