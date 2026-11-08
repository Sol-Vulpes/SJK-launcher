//! The Tools tab (`crate::tools`): SJK's folders and files a click away, a backup of
//! the player's setup, and Steam's file check.

use super::{App, DIM};
use crate::system;
use crate::theme::{self, GOLD_BRIGHT};
use crate::tools::{self, Place};
use eframe::egui::{self, RichText};

impl App {
    pub(super) fn tools_page(&mut self, ui: &mut egui::Ui) {
        let game_data = self.core.game_data();
        ui.add_space(8.0);
        ui.heading("Folders");
        ui.label(RichText::new("Where Sol JK keeps its things. A greyed one does not exist yet (nothing saved there so far).").color(DIM));
        ui.add_space(4.0);
        places(ui, "tools-folders", &tools::folders(game_data.as_deref()), system::open);

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Files");
        ui.label(RichText::new("Opened in a text editor.").color(DIM));
        ui.add_space(4.0);
        places(ui, "tools-files", &tools::files(game_data.as_deref()), tools::edit);

        let Some(game_data) = game_data else {
            ui.add_space(8.0);
            ui.label(RichText::new("Choose Jedi Academy's GameData in Settings for the rest.").color(DIM));
            return;
        };

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Back up");
        ui.label(
            RichText::new("Your SJK identity, settings, favourites, notes and map edits in one zip, to keep safe or move to another PC (unzip it into GameData there). Demos, screenshots and ghosts stay out: copy those folders yourself.")
                .color(DIM),
        );
        if ui.button(RichText::new("Back up my SJK setup…").color(GOLD_BRIGHT)).clicked()
            && let Some(to) = rfd::FileDialog::new().set_title("Save the backup").set_file_name(tools::backup_name()).add_filter("Zip", &["zip"]).save_file()
        {
            match tools::backup(&game_data, &to) {
                Ok(count) => {
                    self.say(format!("{count} files backed up to {}", to.display()), false);
                    crate::pk3s::reveal(&to);
                }
                Err(error) => self.say(error, true),
            }
        }

        if tools::is_steam(&game_data) {
            ui.add_space(8.0);
            theme::rule(ui);
            ui.add_space(8.0);
            ui.heading("Steam");
            ui.label(RichText::new("Steam checks Jedi Academy's own files and fetches any broken or missing one (a crash on start or a missing map can come from those). SJK's files are left alone.").color(DIM));
            if ui.button("Verify the game's files").clicked() {
                tools::steam_verify();
                self.say("Steam is checking Jedi Academy's files", false);
            }
        }

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Copy");
        ui.horizontal(|ui| {
            if ui.button("GameData's path").clicked() {
                ui.ctx().copy_text(game_data.display().to_string());
                self.say("GameData's path is copied", false);
            }
            if let Some(folder) = crate::config::folder()
                && ui.button("The launcher's folder path").clicked()
            {
                ui.ctx().copy_text(folder.display().to_string());
                self.say("The launcher's folder path is copied", false);
            }
        });
        ui.add_space(16.0);
    }

}

/// One button per place, open with `open`; greyed while it is not on disk.
fn places(ui: &mut egui::Ui, id: &str, places: &[Place], open: fn(&std::path::Path)) {
    egui::Grid::new(id).num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        for place in places {
            let there = place.path.exists();
            let button = ui
                .add_enabled(there, egui::Button::new(place.label).min_size(egui::vec2(140.0, 0.0)))
                .on_hover_text(place.path.display().to_string())
                .on_disabled_hover_text(format!("Not there yet: {}", place.path.display()));
            if button.clicked() {
                open(&place.path);
            }
            ui.label(RichText::new(place.hint).color(DIM));
            ui.end_row();
        }
    });
}
