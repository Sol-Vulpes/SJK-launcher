//! The Doctor tab (`crate::doctor`): the checks with their fixes, the graphics API, a
//! settings reset and Send my logs; and the window offered when a game freezes.

use super::{App, BAD, DIM, Tab};
use crate::config::Graphics;
use crate::doctor::{self, Check, Fix, Level, Package};
use crate::games::Game;
use crate::theme::{self, GOLD, GOLD_BRIGHT, WARN};
use eframe::egui::{self, RichText};
use std::sync::{Arc, Mutex};

/// A job running off the window's thread, and what it came to.
type Outcome = Arc<Mutex<Option<Result<String, String>>>>;

#[derive(Default)]
pub struct DoctorTab {
    /// The last checks; `None` until the tab first shows (or after a fix).
    checks: Option<Vec<Check>>,
    note: String,
    /// The logs as they would go, while the player looks at them.
    preview: Option<Package>,
    sending: bool,
    sent: Outcome,
    /// A fix running (the all-users one waits on Windows' prompt).
    fixing: bool,
    fixed: Outcome,
    confirm_reset: bool,
}

fn take(outcome: &Outcome) -> Option<Result<String, String>> {
    outcome.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
}

impl App {
    /// The tab opens: its checks are made again.
    pub(super) fn doctor_reopened(&mut self) {
        self.doctor.checks = None;
    }

    pub(super) fn doctor_page(&mut self, ui: &mut egui::Ui) {
        let game_data = self.core.game_data();
        let graphics = self.core.config().graphics;
        if let Some(result) = take(&self.doctor.fixed) {
            self.doctor.fixing = false;
            self.doctor.checks = None;
            match result {
                Ok(text) => self.say(text, false),
                Err(error) => self.say(error, true),
            }
        }
        if let Some(result) = take(&self.doctor.sent) {
            self.doctor.sending = false;
            if result.is_ok() {
                self.core.edit(|config| config.logs_sent = doctor::unix_now());
                self.doctor.note.clear();
                self.doctor.preview = None;
            }
            match result {
                Ok(text) => self.say(text, false),
                Err(error) => self.say(error, true),
            }
        }
        let checks = self.doctor.checks.get_or_insert_with(|| doctor::checks(game_data.as_deref(), graphics)).clone();

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Doctor");
            if ui.button("Check again").clicked() {
                self.doctor.checks = None;
            }
        });
        ui.label(RichText::new("What usually stops Sol JK from starting, checked on this PC, and what the launcher can fix.").color(DIM));
        ui.add_space(8.0);
        let mut fix = None;
        for check in &checks {
            let (mark, colour) = match check.level {
                Level::Good => ("✔", GOLD),
                Level::Warn => ("!", WARN),
                Level::Bad => ("✖", BAD),
            };
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(mark).color(colour).strong().size(17.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&check.title).color(colour).strong());
                    ui.add(egui::Label::new(RichText::new(&check.detail).color(DIM)).wrap());
                    if let Some(action) = &check.fix
                        && ui.add_enabled(!self.doctor.fixing, egui::Button::new(action.label())).clicked()
                    {
                        fix = Some(action.clone());
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(fix) = fix {
            self.apply(fix);
        }

        ui.add_space(4.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Graphics");
        ui.label(
            RichText::new("Sol JK draws with Vulkan where it can. If it stays white or black when it starts, or crashes on graphics errors, try DirectX 12. It applies at the next start.")
                .color(DIM),
        );
        let mut picked = graphics;
        ui.horizontal(|ui| {
            for option in Graphics::ALL {
                ui.radio_value(&mut picked, option, option.label());
            }
        });
        if picked != graphics {
            self.core.edit(|config| config.graphics = picked);
            self.doctor.checks = None;
        }

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("SJK's settings");
        ui.label(RichText::new("Start Sol JK with its default settings. Yours are kept aside as a copy, nothing is deleted; your identity and collection stay.").color(DIM));
        let running = self.core.running(Game::Sjk);
        ui.horizontal(|ui| match (&game_data, self.doctor.confirm_reset) {
            (Some(game_data), true) => {
                ui.label(RichText::new("Put SJK's settings aside?").color(WARN));
                if ui.add_enabled(!running, egui::Button::new("Yes, reset them")).clicked() {
                    match doctor::reset_settings(game_data) {
                        Ok(kept) => self.say(format!("SJK's settings are reset; yours are kept as SJK\\{kept}"), false),
                        Err(error) => self.say(error, true),
                    }
                    self.doctor.confirm_reset = false;
                    self.doctor.checks = None;
                }
                if ui.button("Cancel").clicked() {
                    self.doctor.confirm_reset = false;
                }
            }
            (Some(_), false) => {
                if ui.add_enabled(!running, egui::Button::new("Reset SJK's settings")).on_disabled_hover_text("Close Sol JK first").clicked() {
                    self.doctor.confirm_reset = true;
                }
            }
            (None, _) => {
                ui.label(RichText::new("Choose the game folder first.").color(DIM));
            }
        });

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        self.send_block(ui, game_data.as_deref(), &checks, graphics);
        ui.add_space(16.0);
    }

    fn send_block(&mut self, ui: &mut egui::Ui, game_data: Option<&std::path::Path>, checks: &[Check], graphics: Graphics) {
        ui.heading("Send my logs to SJK's team");
        ui.label(
            RichText::new(
                "For a game that won't start or freezes, which sends no crash report by itself. What goes: the launcher's and SJK's logs (chat left out, your Windows user name replaced), your system, graphics card and build, and the checks above, signed with your SJK identity. Nothing else.",
            )
            .color(DIM),
        );
        let Some(game_data) = game_data else {
            ui.label(RichText::new("Choose the game folder first.").color(DIM));
            return;
        };
        ui.add(
            egui::TextEdit::singleline(&mut self.doctor.note)
                .hint_text("What happens? (optional, e.g. \"white screen after the menu\")")
                .char_limit(sjk_identity::logs::OWN_NOTE_MAX)
                .desired_width(560.0),
        );
        let build = self.core.config().installed.get(&Game::Sjk).map(|tag| Game::Sjk.version_of(tag)).unwrap_or_default();
        let wait = doctor::wait_left(self.core.config().logs_sent);
        ui.horizontal(|ui| {
            let words = if self.doctor.sending {
                "Sending…".to_owned()
            } else if wait > 0 {
                format!("Sent: again in {} min", wait.div_ceil(60))
            } else {
                "Send my logs".to_owned()
            };
            if ui.add_enabled(!self.doctor.sending && wait == 0, egui::Button::new(RichText::new(words).color(GOLD_BRIGHT))).clicked() {
                let package = doctor::package(game_data, checks, graphics, &build);
                let note = self.doctor.note.clone();
                let game_data = game_data.to_owned();
                let sent = self.doctor.sent.clone();
                let ctx = ui.ctx().clone();
                self.doctor.sending = true;
                std::thread::spawn(move || {
                    let result = doctor::send(&game_data, &package, &note);
                    *sent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                    ctx.request_repaint();
                });
            }
            let showing = self.doctor.preview.is_some();
            if ui.button(if showing { "Hide what goes" } else { "Show what goes" }).clicked() {
                self.doctor.preview = (!showing).then(|| doctor::package(game_data, checks, graphics, &build));
            }
            if ui.button("Copy them").on_hover_text("The same text, to paste in Discord").clicked() {
                ui.ctx().copy_text(doctor::package(game_data, checks, graphics, &build).upload.logs);
                self.say("Your logs are copied", false);
            }
        });
        if let Some(package) = &self.doctor.preview {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("{} KB", package.upload.logs.len().div_ceil(1024))).color(DIM));
            egui::ScrollArea::vertical().id_salt("doctor-preview").max_height(320.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&package.upload.logs).monospace().size(12.0)).wrap());
            });
        }
    }

    fn apply(&mut self, fix: Fix) {
        match fix {
            Fix::ChooseGameData => self.tab = Tab::Settings,
            Fix::Install => self.tab = Tab::Game(Game::Sjk),
            Fix::ClearRunAsAdmin { exe, all_users } => {
                self.doctor.fixing = true;
                let fixed = self.doctor.fixed.clone();
                let ctx = self.core.ctx.clone();
                std::thread::spawn(move || {
                    let result = doctor::clear_run_as_admin(&exe, all_users).map(|()| "Sol JK no longer asks to run as administrator: press Play".to_owned());
                    *fixed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                    ctx.request_repaint();
                });
            }
        }
    }

    /// The window offered when a game stopped answering (`Core::watch_freezes`), or the
    /// player asked to end it.
    pub(super) fn freeze_window(&mut self, ctx: &egui::Context) {
        let freeze = *self.core.freeze.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(freeze) = freeze.filter(|freeze| !freeze.waiting) else { return };
        let game = freeze.game;
        egui::Window::new(RichText::new(format!("{} is not responding", game.name())).color(WARN))
            .id(egui::Id::new("freeze_window"))
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("It has not answered Windows for {} s.", freeze.since.elapsed().as_secs()));
                ui.label(
                    RichText::new("A game stuck like this sends no crash report by itself. Ending it from here sends one with its last lines, so SJK's team can see where it stopped.")
                        .color(DIM),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("End it and report").color(GOLD_BRIGHT)).clicked()
                        && let Err(error) = self.core.end(game)
                    {
                        self.say(error, true);
                    }
                    if ui.button("Keep waiting").clicked() {
                        self.core.keep_waiting();
                    }
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}
