use std::time::Duration;

use eframe::egui;

use crate::quiz::{QuizConfig, QuizState};

const FLASH_DURATION: Duration = Duration::from_millis(600);
const DONE_DISPLAY_DURATION: Duration = Duration::from_millis(1800);
const QUESTION_TIMEOUT_SECS: u64 = 30;

pub struct QuizApp {
    state: QuizState,
    done_at: Option<std::time::Instant>,
    question_deadline: std::time::Instant,
}

impl QuizApp {
    fn new(state: QuizState) -> Self {
        Self {
            state,
            done_at: None,
            question_deadline: std::time::Instant::now()
                + Duration::from_secs(QUESTION_TIMEOUT_SECS),
        }
    }
}

impl eframe::App for QuizApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep repainting while animating or counting down.
        if self.state.flash.is_some() || self.done_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        // Expire flash → advance question and reset countdown.
        if let Some((_, _, t)) = self.state.flash {
            if t.elapsed() >= FLASH_DURATION {
                self.state.flash = None;
                if self.done_at.is_none() {
                    self.state.next_question();
                    self.question_deadline =
                        std::time::Instant::now() + Duration::from_secs(QUESTION_TIMEOUT_SECS);
                }
            }
        }

        // Question timed out → exit like Quit (triggers the regular notify popup).
        if self.done_at.is_none()
            && self.state.flash.is_none()
            && std::time::Instant::now() >= self.question_deadline
        {
            std::process::exit(1);
        }

        // Exit after "earned" display.
        if let Some(t) = self.done_at {
            if t.elapsed() >= DONE_DISPLAY_DURATION {
                // Print result for the parent process before exiting.
                let secs = self.state.config.extra_minutes as i64 * 60;
                println!("EARNED_SECS:{secs}");
                std::process::exit(0);
            }
        }

        // Snapshot values so the closure below can borrow self.state mutably later.
        let score = self.state.score;
        let needed = self.state.config.correct_needed;
        let extra_mins = self.state.config.extra_minutes;
        let word = self.state.entries[self.state.current].word.clone();
        let choices = self.state.choices.clone();
        let correct_idx = self.state.correct_idx;
        let flash = self.state.flash;
        let in_flash = flash.is_some();
        let is_done = self.done_at.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
            // ── Quit button (top-right) ──────────────────────────────────────
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui.button("✕  Quit").clicked() {
                    std::process::exit(1);
                }
            });

            ui.add_space(16.0);

            ui.vertical_centered(|ui| {
                // ── Header ───────────────────────────────────────────────────
                ui.label(
                    egui::RichText::new("Vocabulary Quiz – Earn Extra Time")
                        .size(22.0)
                        .strong(),
                );
                ui.add_space(6.0);

                // ── "You earned …" message ───────────────────────────────────
                if is_done {
                    ui.add_space(60.0);
                    ui.label(
                        egui::RichText::new(format!("🎉  You earned {extra_mins} extra minutes!"))
                            .size(32.0)
                            .color(egui::Color32::from_rgb(80, 220, 80))
                            .strong(),
                    );
                    return;
                }

                // ── Progress ─────────────────────────────────────────────────
                ui.label(
                    egui::RichText::new(format!("✓  {score} / {needed} correct answers needed"))
                        .size(16.0)
                        .color(egui::Color32::from_rgb(180, 220, 180)),
                );
                ui.add_space(24.0);

                // ── Question word ────────────────────────────────────────────
                ui.label(
                    egui::RichText::new(&word)
                        .size(52.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                ui.add_space(8.0);

                // ── Per-question countdown ───────────────────────────────────
                let secs_left = self
                    .question_deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_secs();
                let countdown_color = if secs_left <= 5 {
                    egui::Color32::from_rgb(220, 60, 60)
                } else if secs_left <= 10 {
                    egui::Color32::from_rgb(230, 140, 30)
                } else {
                    egui::Color32::from_rgb(160, 160, 160)
                };
                ui.label(
                    egui::RichText::new(format!("{secs_left}s"))
                        .size(18.0)
                        .color(countdown_color),
                );
                ui.add_space(16.0);

                // ── 2 × 2 answer grid ────────────────────────────────────────
                let btn_size = egui::vec2(320.0, 64.0);

                ui.horizontal(|ui| {
                    let grid_width = 320.0 * 2.0 + 16.0;
                    let available = ui.available_width();
                    if available > grid_width {
                        ui.add_space((available - grid_width) / 2.0);
                    }
                    egui::Grid::new("answers")
                        .num_columns(2)
                        .spacing([16.0, 16.0])
                        .show(ui, |ui| {
                            for (i, choice) in choices.iter().enumerate() {
                                let fill = if in_flash {
                                    if let Some((was_correct, chosen, _)) = flash {
                                        if i == correct_idx {
                                            egui::Color32::from_rgb(40, 180, 40) // always green
                                        } else if !was_correct && i == chosen {
                                            egui::Color32::from_rgb(180, 40, 40) // chosen wrong = red
                                        } else {
                                            egui::Color32::from_gray(50)
                                        }
                                    } else {
                                        egui::Color32::from_gray(50)
                                    }
                                } else {
                                    egui::Color32::from_gray(70)
                                };

                                let btn = egui::Button::new(egui::RichText::new(choice).size(24.0))
                                    .min_size(btn_size)
                                    .fill(fill);

                                if ui.add_enabled(!in_flash, btn).clicked() {
                                    let was_correct = self.state.answer(i);
                                    if was_correct && self.state.score >= needed {
                                        self.done_at = Some(std::time::Instant::now());
                                    }
                                }

                                if i % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            });
        });
    }
}

/// Run the quiz window. Exits the process when done (exit 0 = earned, exit 1 = quit).
pub fn run_quiz_window(config: QuizConfig) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 560.0])
            .with_always_on_top()
            .with_decorations(false)
            .with_title("Vocabulary Quiz"),
        ..Default::default()
    };

    eframe::run_native(
        "Vocabulary Quiz",
        options,
        Box::new(move |_cc| {
            let state = QuizState::new(config)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            Ok(Box::new(QuizApp::new(state)))
        }),
    )
    .unwrap_or_else(|e| {
        eprintln!("vocab_trainer: display error: {e}");
        std::process::exit(2);
    });
}
