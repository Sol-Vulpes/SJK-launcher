//! SJK Launcher: installs and updates Sol JK and JoF EternalJK from their
//! GitHub releases, starts them, and fetches JoF content into SJK's JoF folder.
//! `--minimized` starts it hidden in the tray, as the autostart entry does.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod brand;
mod chat;
mod config;
mod content;
mod dev;
mod doctor;
mod state;
mod game_log;
mod gamedata;
mod games;
mod github;
mod install;
mod instance;
mod pk3s;
mod pk3scan;
mod scans;
mod self_update;
mod system;
mod theme;
mod tools;

use eframe::egui;
use std::sync::Arc;

fn main() -> eframe::Result {
    let minimized = std::env::args().any(|arg| arg == system::MINIMIZED);
    if !instance::claim(minimized) {
        // Already running: it shows its window (unless this was the autostart).
        return Ok(());
    }
    self_update::clean_up();
    let mut config = config::Config::load();
    if config.game_data.as_ref().is_none_or(|path| !gamedata::is_game_data(path)) {
        config.game_data = gamedata::detect().or(config.game_data);
        config.save();
    }
    // One separator throughout (Steam's library paths mix them).
    config.game_data = config.game_data.map(|path| path.components().collect());

    let icon = egui::IconData {
        rgba: system::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(brand::NAME)
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([960.0, 620.0])
            // The header is the title bar (app::App::header).
            .with_decorations(false)
            .with_icon(Arc::new(icon))
            .with_visible(!(minimized && cfg!(windows))),
        ..Default::default()
    };
    eframe::run_native(
        brand::NAME,
        options,
        Box::new(move |cc| {
            let core = state::Core::new(cc.egui_ctx.clone(), config);
            core.start_checker();
            Ok(Box::new(app::App::new(cc, core, minimized)))
        }),
    )
}
