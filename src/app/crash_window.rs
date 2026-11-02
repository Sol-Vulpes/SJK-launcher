//! The window that comes up when a game started from the launcher goes down: what
//! happened, its last lines, where the crash report went, and the full log.

use super::{App, BAD, DIM};
use crate::game_log::{Exit, Report, STARTING};
use crate::theme::GOLD_BRIGHT;
use eframe::egui::{self, RichText};

impl App {
    pub(super) fn crash_window(&mut self, ctx: &egui::Context) {
        // Debug builds: SJK_LAUNCHER_CRASH=<a log file> shows the window as if Sol JK
        // had gone down with that log.
        #[cfg(debug_assertions)]
        if let Ok(log) = std::env::var("SJK_LAUNCHER_CRASH")
            && ctx.input(|input| input.time) < 0.5
        {
            let what = crate::game_log::describe(0xC000_0005_u32 as i32);
            let report = crate::game_log::Report::Waiting("the hub did not answer".to_owned());
            *self.core.exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(crate::game_log::exit(crate::games::Game::Sjk, what, std::time::Duration::from_secs(4), log.into(), report));
        }
        let Some(exit) = self.core.exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else { return };
        let mut close = false;
        let mut again = false;
        let title = if exit.ran < STARTING {
            format!("{} could not start", exit.game.name())
        } else {
            format!("{} closed unexpectedly", exit.game.name())
        };
        egui::Window::new(RichText::new(title).color(BAD))
            .id(egui::Id::new("crash_window"))
            .collapsible(false)
            .resizable(false)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("It stopped with {}, after {}.", exit.what, ran(&exit)));
                ui.label(RichText::new(report_line(&exit)).color(DIM));
                ui.add_space(6.0);
                if exit.tail.trim().is_empty() {
                    ui.label(RichText::new("The game printed nothing before it stopped.").color(DIM));
                } else {
                    ui.label(RichText::new("Its last lines:").strong());
                    egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(&exit.tail).monospace().size(12.5)).wrap());
                    });
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy the log").on_hover_text("The whole log, to paste in Discord or a bug report").clicked() {
                        let text = std::fs::read(&exit.log).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
                        ctx.copy_text(text);
                        self.say("The log is copied", false);
                    }
                    if ui.button("Open the log").clicked() {
                        crate::system::open(&exit.log);
                    }
                    if ui.button("Show in folder").clicked() {
                        crate::pk3s::reveal(&exit.log);
                    }
                    if ui.button(RichText::new("Play again").color(GOLD_BRIGHT)).clicked() {
                        again = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close || again {
            *self.core.exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
        if again && let Err(error) = self.core.play(exit.game) {
            self.say(error, true);
        }
    }
}

fn ran(exit: &Exit) -> String {
    match exit.ran.as_secs() {
        seconds @ 0..60 => format!("{seconds} s"),
        seconds @ 60..3600 => format!("{} min", seconds / 60),
        seconds => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

fn report_line(exit: &Exit) -> String {
    match &exit.report {
        Report::None => "If it keeps happening, copy the log and send it to whoever helps you with the game.".to_owned(),
        Report::Off => "Crash reports are off in SJK's settings, so nothing was sent. Copy the log to pass it on.".to_owned(),
        Report::Sent(numbers) if numbers.is_empty() => "SJK's team already had this crash report.".to_owned(),
        Report::Sent(_) => "A crash report went to SJK's team, with these lines.".to_owned(),
        Report::Waiting(why) => format!("A crash report is kept for SJK's team, sent as soon as it can be ({why})."),
    }
}
