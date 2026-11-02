//! The window, in SJK's look: a game per tab over the horizon, its versions
//! and latest changes, and a dock with the content sources and the one gold
//! action; then the content collections and the settings.

mod crash_window;
mod pk3_tab;

use crate::content::{COLLECTIONS, Collection};
use crate::games::{GAMES, Game};
use crate::github::Release;
use crate::install::Phase;
use crate::pk3scan::{Level, Location, Report};
use crate::scans::{ScanState, Scanner};
use crate::state::{Core, Standing};
use crate::system::{self, Tray, TrayAction};
use crate::chat::Status as ChatStatus;
use crate::theme::{self, EMBER, GOLD, GOLD_BRIGHT, HOLO, MUTED, SPACE, WARN, WindowButton, display, strong};
use eframe::egui::{self, Color32, Rect, RichText, Sense, Vec2};
use sjk_jof_content::job::{Job, Phase as ContentPhase, Settings};
use sjk_jof_content::{Asset, AssetState, Group, Plan, Selection, human_size};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const DIM: Color32 = MUTED;
const BAD: Color32 = EMBER;
/// How far a group's files sit in from its checkbox.
const FILE_INDENT: f32 = 40.0;
const INSPECT_WIDTH: f32 = 420.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Game(Game),
    Content(Collection),
    Pk3s,
    Settings,
}

pub struct App {
    core: Core,
    tray: Option<Tray>,
    tab: Tab,
    emblem: Option<egui::TextureHandle>,
    /// The release picked in each game's list, by tag; `None` is the newest.
    picked: [Option<String>; 2],
    /// Start the game once its install finishes ("Update and play").
    play_after: [bool; 2],
    message: Option<(String, bool)>,
    /// One job and one choice per collection, in `COLLECTIONS`' order.
    content: [Job; 2],
    selections: [Selection; 2],
    /// Each collection's folders and hosts, made once per refresh.
    settings: [Option<Settings>; 2],
    scanner: Scanner,
    /// The file whose insides are shown: collection, group id, file id.
    inspect: Option<(Collection, u32, u32)>,
    inspect_filter: String,
    /// Groups whose files are listed, by collection index and group id.
    open_groups: HashSet<(usize, u32)>,
    pending: Option<Pending>,
    tooltip: String,
    chat: crate::chat::Chat,
    /// The SJK chat window is open.
    chat_open: bool,
    chat_draft: String,
    /// Why the draft could not go, said under the field.
    chat_error: Option<String>,
    icon: Arc<egui::IconData>,
    pk3: pk3_tab::Pk3Tab,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Core, minimized: bool) -> Self {
        theme::install(&cc.egui_ctx);
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = cc.window_handle()
                && let RawWindowHandle::Win32(win32) = handle.as_raw()
            {
                core.hwnd.store(win32.hwnd.get(), Ordering::Relaxed);
            }
        }
        core.hidden.store(minimized, Ordering::Relaxed);
        let tray = make_tray(&core);
        {
            let core = core.clone();
            crate::instance::on_show(move || core.show());
        }
        let selections = COLLECTIONS.map(|collection| collection.selection(&core.config()));
        let mut app = Self {
            tray,
            tab: Tab::Game(GAMES[0]),
            emblem: theme::emblem(&cc.egui_ctx),
            picked: [None, None],
            play_after: [false, false],
            message: None,
            content: Default::default(),
            selections,
            settings: Default::default(),
            scanner: Scanner::new(cc.egui_ctx.clone()),
            inspect: None,
            inspect_filter: String::new(),
            open_groups: HashSet::new(),
            pending: None,
            tooltip: String::new(),
            chat: crate::chat::Chat::new(cc.egui_ctx.clone()),
            chat_open: false,
            chat_draft: String::new(),
            chat_error: None,
            icon: Arc::new(egui::IconData { rgba: system::icon_rgba(64), width: 64, height: 64 }),
            pk3: Default::default(),
            core,
        };
        for collection in COLLECTIONS {
            app.refresh_content(collection);
        }
        app
    }

    fn say(&mut self, text: impl Into<String>, bad: bool) {
        self.message = Some((text.into(), bad));
    }

    fn refresh_content(&mut self, collection: Collection) {
        if let Some(game_data) = self.core.game_data() {
            let settings = collection.settings(&self.core.config(), &game_data);
            self.settings[collection as usize] = Some(settings.clone());
            self.content[collection as usize].refresh(settings);
        }
    }

    fn save_selection(&self, collection: Collection) {
        let selection = &self.selections[collection as usize];
        self.core.edit(|config| collection.save(config, selection));
    }

    fn hide(&self, ctx: &egui::Context) {
        self.core.hidden.store(true, Ordering::Relaxed);
        if cfg!(windows) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }
}

fn make_tray(core: &Core) -> Option<Tray> {
    let handler = {
        let core = core.clone();
        move |action: TrayAction| match action {
            TrayAction::Show | TrayAction::Chat => {
                // The chat window lives with the main one, so both show.
                if action == TrayAction::Chat {
                    core.open_chat.store(true, Ordering::Relaxed);
                }
                core.show();
            }
            TrayAction::Play(game) => {
                let _ = core.play(game);
            }
            TrayAction::Check => core.check_all(),
            TrayAction::Quit => std::process::exit(0),
        }
    };
    // Linux trays need GTK's loop on a thread of its own.
    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(move || {
            if gtk::init().is_ok()
                && let Ok(_tray) = Tray::new(handler)
            {
                gtk::main();
            }
        });
        None
    }
    #[cfg(not(target_os = "linux"))]
    Tray::new(handler).ok()
}

/// Sizes follow the window's height, as SJK's menus follow a 1080-line screen.
fn scale(ctx: &egui::Context) -> f32 {
    (ctx.screen_rect().height() / 800.0).clamp(0.78, 1.3)
}

/// What the dock's gold button does for a game now.
enum Action {
    Play,
    Install { release: Release, then_play: bool },
    Wait,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.core.hidden.load(Ordering::Relaxed) && !ctx.input(|input| input.viewport().minimized.unwrap_or(false)) {
            // Started with --minimized, or hidden from the tray before this frame.
            self.hide(ctx);
        }
        // Linux's tray lives on its own thread, so `self.tray` is empty there.
        let has_tray = self.tray.is_some() || cfg!(target_os = "linux");
        if ctx.input(|input| input.viewport().close_requested()) && self.core.config().close_to_tray && has_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        let summary = self.core.summary();
        if summary != self.tooltip {
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&summary);
            }
            self.tooltip = summary;
        }
        self.play_when_installed();
        // Debug builds: SJK_LAUNCHER_TAB=chat draws the chat page in the main window
        // (the one window a screenshot reaches), at the chat window's size.
        #[cfg(debug_assertions)]
        if std::env::var("SJK_LAUNCHER_TAB").as_deref() == Ok("chat") {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(460.0, 660.0)));
            self.chat.set_active(true, false, self.core.game_data().map(|game_data| game_data.join("SJK")));
            self.chat_page(ctx);
            return debug_shot(ctx, &mut self.tab);
        }
        if self.core.open_chat.swap(false, Ordering::Relaxed) {
            self.open_chat(ctx);
        }

        let on_game = matches!(self.tab, Tab::Game(_));
        let screen = ctx.screen_rect();
        theme::backdrop(
            &ctx.layer_painter(egui::LayerId::background()),
            screen,
            on_game,
            if on_game { self.emblem.as_ref() } else { None },
        );
        let s = scale(ctx);
        let clear = egui::Frame::NONE;

        egui::TopBottomPanel::top("header")
            .frame(clear.inner_margin(egui::Margin { left: (64.0 * s) as i8, right: (64.0 * s) as i8, top: (26.0 * s) as i8, bottom: 10 }))
            .show_separator_line(false)
            .show(ctx, |ui| self.header(ui, s));
        if let Tab::Game(game) = self.tab {
            egui::TopBottomPanel::bottom("dock")
                .frame(clear.inner_margin(egui::Margin { left: (64.0 * s) as i8, right: (64.0 * s) as i8, top: 12, bottom: (40.0 * s) as i8 }))
                .show_separator_line(false)
                .show(ctx, |ui| self.dock(ui, game, s));
        }
        self.inspect_panel(ctx);
        self.pk3_panel(ctx);
        let margin = if on_game { 64.0 * s } else { 48.0 * s };
        egui::CentralPanel::default()
            .frame(clear.inner_margin(egui::Margin { left: margin as i8, right: margin as i8, top: 8, bottom: 16 }))
            .show(ctx, |ui| match self.tab {
                Tab::Game(game) => self.game_page(ui, game, s),
                Tab::Content(collection) => self.content_tab(ui, collection),
                Tab::Pk3s => self.pk3_page(ui),
                Tab::Settings => self.settings(ui),
            });
        self.advance_pending(ctx);
        self.pk3_confirm_window(ctx);
        self.crash_window(ctx);
        window_edges(ctx);
        self.chat_window(ctx);

        let busy = GAMES.iter().any(|&game| self.core.slot(game).install.running()) || self.content.iter().any(Job::busy) || self.pending.is_some() || self.pk3.busy();
        ctx.request_repaint_after(std::time::Duration::from_millis(if busy { 100 } else { 1000 }));
        #[cfg(debug_assertions)]
        {
            debug_shot(ctx, &mut self.tab);
            // SJK_LAUNCHER_SEARCH=<text> searches the PK3s tab; SJK_LAUNCHER_PK3=<name> opens one there.
            self.pk3_debug();
            // SJK_LAUNCHER_INSPECT=<file.pk3> opens that file's insides once the list is here.
            if let (Ok(name), None, Tab::Content(collection)) = (std::env::var("SJK_LAUNCHER_INSPECT"), self.inspect, self.tab)
                && let Some(snapshot) = self.content[collection as usize].progress().snapshot
                && let Some((group_index, group)) =
                    snapshot.manifest.groups.iter().enumerate().find(|(_, group)| group.assets.iter().any(|asset| asset.name == name))
                && let Some(asset_index) = group.assets.iter().position(|asset| asset.name == name)
            {
                let asset = &group.assets[asset_index];
                self.scan(collection, snapshot.state(group_index, asset_index), asset);
                self.inspect = Some((collection, group.id, asset.id));
                self.open_groups.insert((collection as usize, group.id));
            }
        }
    }
}

/// Without the system's frame: a thin holo edge, and the window resizes from
/// its edges and corners.
fn window_edges(ctx: &egui::Context) {
    use egui::{CursorIcon, ResizeDirection};
    let screen = ctx.screen_rect();
    let maximised = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    if maximised {
        return;
    }
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("edge")))
        .rect_stroke(screen, 0.0, egui::Stroke::new(1.0, theme::holo(70)), egui::StrokeKind::Inside);
    let Some(at) = ctx.input(|input| input.pointer.hover_pos()) else { return };
    let grip = 6.0;
    let (left, right) = (at.x - screen.left() < grip, screen.right() - at.x < grip);
    let (top, bottom) = (at.y - screen.top() < grip, screen.bottom() - at.y < grip);
    let (direction, cursor) = match (left, right, top, bottom) {
        (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        (true, ..) => (ResizeDirection::West, CursorIcon::ResizeWest),
        (_, true, ..) => (ResizeDirection::East, CursorIcon::ResizeEast),
        (_, _, true, _) => (ResizeDirection::North, CursorIcon::ResizeNorth),
        (.., true) => (ResizeDirection::South, CursorIcon::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|input| input.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

/// Debug builds only: `SJK_LAUNCHER_SHOT=<file.bmp>` saves the window after a few
/// seconds, `SJK_LAUNCHER_TAB=jof|content|sith|settings` picks the tab first.
#[cfg(debug_assertions)]
fn debug_shot(ctx: &egui::Context, tab: &mut Tab) {
    let Ok(path) = std::env::var("SJK_LAUNCHER_SHOT") else { return };
    let wait: f64 = std::env::var("SJK_LAUNCHER_WAIT").ok().and_then(|w| w.parse().ok()).unwrap_or(6.0);
    let time = ctx.input(|input| input.time);
    if time < 1.0 {
        *tab = match std::env::var("SJK_LAUNCHER_TAB").as_deref() {
            Ok("jof") => Tab::Game(Game::Jof),
            Ok("content") => Tab::Content(Collection::Jof),
            Ok("sith") => Tab::Content(Collection::Sith),
            Ok("pk3s") => Tab::Pk3s,
            Ok("settings") => Tab::Settings,
            _ => Tab::Game(GAMES[0]),
        };
    }
    if time > wait && ctx.memory(|memory| memory.data.get_temp::<bool>(egui::Id::new("shot")).is_none()) {
        ctx.memory_mut(|memory| memory.data.insert_temp(egui::Id::new("shot"), true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    for event in ctx.input(|input| input.raw.events.clone()) {
        if let egui::Event::Screenshot { image, .. } = event {
            let [width, height] = image.size;
            let mut bmp = Vec::with_capacity(54 + width * height * 4);
            let size = (54 + width * height * 4) as u32;
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&size.to_le_bytes());
            bmp.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
            bmp.extend_from_slice(&(width as i32).to_le_bytes());
            bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
            bmp.extend_from_slice(&[1, 0, 32, 0]);
            bmp.extend_from_slice(&[0; 24]);
            for pixel in &image.pixels {
                bmp.extend_from_slice(&[pixel.b(), pixel.g(), pixel.r(), 255]);
            }
            let _ = std::fs::write(&path, bmp);
            std::process::exit(0);
        }
    }
    ctx.request_repaint();
}

impl App {
    /// The games and Content as tabs, Settings and the window controls on the
    /// right; a message under them. It is also the title bar: drag it to move the
    /// window, double-click it to maximise.
    fn header(&mut self, ui: &mut egui::Ui, s: f32) {
        let ctx = ui.ctx().clone();
        let maximised = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let screen = ctx.screen_rect();
        let bar = Rect::from_min_max(screen.min, egui::pos2(screen.right(), ui.max_rect().top() + 44.0 * s));
        // Made first, so the tabs and buttons laid over it take the clicks.
        let bar = ui.interact(bar, egui::Id::new("title-bar"), Sense::click_and_drag());
        if bar.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bar.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximised));
        }
        ui.horizontal(|ui| {
            // As tall as the window buttons, so the tabs share their middle.
            ui.set_min_height(34.0 * s);
            ui.spacing_mut().item_spacing.x = 36.0 * s;
            for game in GAMES {
                if theme::tab(ui, game.name(), 22.0 * s, self.tab == Tab::Game(game)).clicked() {
                    self.tab = Tab::Game(game);
                }
            }
            let on_content = matches!(self.tab, Tab::Content(_));
            if theme::tab(ui, "Content", 22.0 * s, on_content).clicked() && !on_content {
                self.tab = Tab::Content(Collection::Jof);
            }
            if theme::tab(ui, "PK3s", 22.0 * s, self.tab == Tab::Pk3s).clicked() {
                self.tab = Tab::Pk3s;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let size = 34.0 * s;
                if theme::window_button(ui, WindowButton::Close, size).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let resize = if maximised { WindowButton::Restore } else { WindowButton::Maximise };
                if theme::window_button(ui, resize, size).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximised));
                }
                if theme::window_button(ui, WindowButton::Minimise, size).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                ui.add_space(14.0 * s);
                let (line, _) = ui.allocate_exact_size(Vec2::new(1.0, 22.0 * s), Sense::hover());
                ui.painter().rect_filled(line, 0.0, theme::holo(80));
                ui.add_space(14.0 * s);
                ui.spacing_mut().item_spacing.x = 36.0 * s;
                if theme::tab(ui, "Settings", 17.0 * s, self.tab == Tab::Settings).clicked() {
                    self.tab = Tab::Settings;
                }
                if theme::tab(ui, "Chat", 17.0 * s, self.chat_open).on_hover_text("SJK chat, in its own window").clicked() {
                    self.open_chat(&ctx);
                }
                if self.core.game_data().is_none() {
                    ui.label(RichText::new("Choose Jedi Academy's GameData in Settings").color(BAD));
                }
            });
        });
        if let Some((text, bad)) = self.message.clone() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(text).color(if bad { BAD } else { DIM }));
                if ui.small_button("✕").clicked() {
                    self.message = None;
                }
            });
        }
    }

    /// "Update and play": start the game when its install has finished.
    fn play_when_installed(&mut self) {
        for game in GAMES {
            if !self.play_after[game as usize] {
                continue;
            }
            match self.core.slot(game).install.phase() {
                Phase::Done(_) => {
                    self.play_after[game as usize] = false;
                    self.core.slot(game).install.clear();
                    if let Err(error) = self.core.play(game) {
                        self.say(error, true);
                    }
                }
                Phase::Failed(_) | Phase::Idle => self.play_after[game as usize] = false,
                _ => {}
            }
        }
    }

    fn picked_release(&self, game: Game) -> Option<Release> {
        let releases = self.core.releases(game).and_then(Result::ok).unwrap_or_default();
        self.picked[game as usize]
            .as_ref()
            .and_then(|tag| releases.iter().find(|release| &release.tag_name == tag))
            .or(releases.first())
            .cloned()
    }

    /// The game over the horizon: what it is, the versions, what changed.
    fn game_page(&mut self, ui: &mut egui::Ui, game: Game, s: f32) {
        let core = self.core.clone();
        let width = (ui.available_width() * 0.52).max(420.0);
        ui.add_space(70.0 * s);
        ui.label(RichText::new(game.blurb()).size(15.0 * s).color(HOLO));
        // As large as fits left of the emblem (Rajdhani's letters run about half their size wide).
        let fits = ui.available_width() * 0.56 / (game.name().chars().count() as f32 * 0.5);
        ui.label(display(game.name(), (118.0 * s).min(fits)).color(GOLD_BRIGHT));
        ui.add_space(10.0 * s);

        let standing = core.standing(game);
        let latest = core.latest(game);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let version = |ui: &mut egui::Ui, label: &str, value: String, colour: Color32| {
                ui.label(RichText::new(label).size(17.0 * s).color(DIM));
                ui.label(display(value, 22.0 * s).color(colour));
                ui.add_space(20.0 * s);
            };
            match &standing {
                Standing::NotInstalled => _ = ui.label(RichText::new("Not installed").size(17.0 * s).color(DIM)),
                Standing::Unknown => version(ui, "You play", "a version from before the launcher".into(), theme::TEXT),
                Standing::UpToDate(tag) | Standing::Installed(tag) | Standing::Behind { installed: tag, .. } => {
                    version(ui, "You play", game.version_of(tag), theme::TEXT)
                }
            }
            match (&standing, &latest) {
                (Standing::Behind { latest, .. }, _) => version(ui, "New", game.version_of(latest), GOLD_BRIGHT),
                (Standing::UpToDate(_), _) => _ = ui.label(RichText::new("up to date").size(17.0 * s).color(GOLD)),
                (_, Some(latest)) => version(ui, "Latest", game.version_of(&latest.tag_name), theme::TEXT),
                _ => {}
            }
            match core.releases(game) {
                _ if core.slot(game).checking.load(Ordering::Relaxed) => _ = ui.label(RichText::new("checking…").color(DIM)),
                Some(Err(error)) => _ = ui.label(RichText::new(error).color(BAD)),
                _ => {}
            }
        });

        let releases = core.releases(game).and_then(Result::ok).unwrap_or_default();
        ui.horizontal(|ui| {
            let picked = self.picked_release(game);
            let label = match (&self.picked[game as usize], &picked) {
                (Some(_), Some(release)) => format!("Version {}", game.version_of(&release.tag_name)),
                _ => "Other versions".to_owned(),
            };
            ui.menu_button(RichText::new(label).color(GOLD_BRIGHT), |ui| {
                ui.set_min_width(260.0);
                for (index, release) in releases.iter().enumerate() {
                    let mut text = format!("{}   {}", game.version_of(&release.tag_name), release.date());
                    if index == 0 {
                        text.push_str("   latest");
                    }
                    if release.prerelease {
                        text.push_str("   pre-release");
                    }
                    let chosen = picked.as_ref().is_some_and(|picked| picked.tag_name == release.tag_name);
                    if ui.selectable_label(chosen, text).clicked() {
                        self.picked[game as usize] = (index != 0).then(|| release.tag_name.clone());
                        ui.close();
                    }
                }
            });
            if let Some(release) = &picked
                && ui.link("Release notes").clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&release.html_url));
            }
            if ui.link("Check again").clicked() {
                core.check(game);
            }
        });

        let highlights = self.picked_release(game).map(|release| release.highlights(4)).unwrap_or_default();
        if !highlights.is_empty() {
            ui.add_space(18.0 * s);
            ui.label(strong("What's new", 15.0 * s).color(HOLO));
            ui.add_space(2.0);
            ui.scope(|ui| {
                ui.set_max_width(width);
                for line in highlights {
                    ui.add(egui::Label::new(RichText::new(line).size(16.0 * s)).wrap());
                }
            });
        }
    }

    /// What the gold button does now, its words, and the quieter choice beside it.
    fn action(&self, game: Game) -> (Action, String, Option<(String, Action)>) {
        let core = &self.core;
        if core.running(game) {
            return (Action::Wait, "Running".into(), None);
        }
        let phase = core.slot(game).install.phase();
        if phase.running() {
            let words = match phase {
                Phase::Downloading { done, total } if total > 0 => format!("Downloading {}%", done * 100 / total),
                Phase::Checking => "Checking…".into(),
                _ => "Installing…".into(),
            };
            return (Action::Wait, words, None);
        }
        let Some(release) = self.picked_release(game) else {
            let words = if core.game_data().is_some_and(|data| game.is_present(&data)) { "Play" } else { "Not installed" };
            let action = if words == "Play" { Action::Play } else { Action::Wait };
            return (action, words.into(), None);
        };
        let version = game.version_of(&release.tag_name);
        let installed = core.config().installed.get(&game).cloned();
        let switching = self.picked[game as usize].is_some() && installed.as_deref() != Some(release.tag_name.as_str());
        let play_as_is = |tag: &str| Some((format!("Play {} as is", game.version_of(tag)), Action::Play));
        match core.standing(game) {
            Standing::NotInstalled => (Action::Install { release, then_play: true }, "Install and play".into(), None),
            _ if switching => {
                let also = installed.as_deref().and_then(play_as_is);
                (Action::Install { release, then_play: true }, format!("Switch to {version}"), also)
            }
            Standing::Behind { installed, .. } => (Action::Install { release, then_play: true }, "Update and play".into(), play_as_is(&installed)),
            Standing::Unknown => (Action::Play, "Play".into(), Some((format!("Install {version}"), Action::Install { release, then_play: false }))),
            Standing::UpToDate(_) | Standing::Installed(_) => (Action::Play, "Play".into(), None),
        }
    }

    fn act(&mut self, game: Game, action: Action) {
        match action {
            Action::Play => {
                if let Err(error) = self.core.play(game) {
                    self.say(error, true);
                }
            }
            Action::Install { release, then_play } => {
                self.picked[game as usize] = None;
                self.play_after[game as usize] = then_play;
                self.core.install(game, release);
            }
            Action::Wait => {}
        }
    }

    /// The bottom of a game's page: the content sources, then the one action.
    fn dock(&mut self, ui: &mut egui::Ui, game: Game, s: f32) {
        let phase = self.core.slot(game).install.phase();
        match &phase {
            Phase::Failed(error) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(error).color(BAD));
                    if ui.small_button("✕").clicked() {
                        self.core.slot(game).install.clear();
                    }
                });
            }
            Phase::Done(text) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).color(GOLD));
                    if ui.small_button("✕").clicked() {
                        self.core.slot(game).install.clear();
                    }
                });
            }
            _ => {}
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 48.0 * s;
            for collection in COLLECTIONS {
                let (status, fraction) = self.content_status(collection);
                let response = ui
                    .scope(|ui| {
                        ui.set_width(250.0 * s);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.label(display(collection.name(), 19.0 * s));
                            ui.label(RichText::new(status).size(14.0 * s).color(DIM));
                            theme::progress_line(ui, 250.0 * s, fraction);
                        });
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    self.tab = Tab::Content(collection);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                ui.spacing_mut().item_spacing.x = 18.0 * s;
                let (action, words, also) = self.action(game);
                let enabled = !matches!(action, Action::Wait);
                if theme::main_button(ui, &words, Vec2::new(270.0 * s, 70.0 * s), enabled).clicked() {
                    self.act(game, action);
                }
                if phase.running() {
                    if ui.link("Stop").clicked() {
                        self.core.slot(game).install.cancel();
                        self.play_after[game as usize] = false;
                    }
                } else if let Some((words, action)) = also
                    && ui.link(RichText::new(words).size(15.0 * s).color(GOLD_BRIGHT)).clicked()
                {
                    self.act(game, action);
                }
            });
        });
    }

    /// One line on where a content collection stands, and how much of what is
    /// wanted is here.
    fn content_status(&self, collection: Collection) -> (String, f32) {
        let index = collection as usize;
        if self.pending.as_ref().is_some_and(|pending| pending.collection == collection) {
            let (done, total) = self.pending_progress();
            return (format!("Scanning {done} of {total} before download"), done as f32 / total.max(1) as f32);
        }
        let progress = self.content[index].progress();
        let busy = match &progress.phase {
            ContentPhase::Manifest => Some("Asking for the list…".to_owned()),
            ContentPhase::Checking { .. } => Some("Checking the files here…".to_owned()),
            ContentPhase::Removing { done, total } => Some(format!("Removing {done} of {total}")),
            ContentPhase::Downloading { file, files, .. } => Some(format!("Downloading {file} of {files}")),
            ContentPhase::Failed(_) if progress.snapshot.is_none() => Some("Cannot reach it".to_owned()),
            _ => None,
        };
        let Some(snapshot) = progress.snapshot else { return (busy.unwrap_or_else(|| "—".into()), 0.0) };
        let selection = &self.selections[index];
        let (mut wanted, mut here) = (0_u64, 0_u64);
        for (group_index, group) in snapshot.manifest.groups.iter().enumerate() {
            for (asset_index, asset) in group.assets.iter().enumerate() {
                let state = snapshot.state(group_index, asset_index);
                if state != AssetState::NotForSjk && (selection.wants(group, asset) || state.present()) {
                    wanted += asset.size;
                    if state.present() {
                        here += asset.size;
                    }
                }
            }
        }
        let fraction = if wanted == 0 { 1.0 } else { here as f32 / wanted as f32 };
        if let ContentPhase::Downloading { done, total, .. } = &progress.phase {
            return (busy.unwrap_or_default(), *done as f32 / (*total).max(1) as f32);
        }
        if let Some(busy) = busy {
            return (busy, fraction);
        }
        let plan = Plan::new(&snapshot, selection);
        let files = snapshot.manifest.assets().count();
        if files == 0 {
            return ("Nothing published yet".to_owned(), 0.0);
        }
        let text = if !plan.downloads.is_empty() {
            format!("{} to download", human_size(plan.download_bytes))
        } else if !plan.removals.is_empty() {
            format!("{} to remove", human_size(plan.removal_bytes))
        } else {
            "Up to date".to_owned()
        };
        (text, fraction)
    }

    fn game_data_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("GameData").color(DIM));
            match self.core.game_data() {
                Some(path) => ui.monospace(path.display().to_string()),
                None => ui.label(RichText::new("not found: choose Jedi Academy's GameData folder").color(BAD)),
            };
        });
        ui.horizontal(|ui| {
            if ui.button("Change…").clicked()
                && let Some(folder) = rfd::FileDialog::new().set_title("Jedi Academy's GameData folder").pick_folder()
            {
                let folder = crate::gamedata::normalise(&folder);
                if crate::gamedata::is_game_data(&folder) {
                    self.core.edit(|config| config.game_data = Some(folder));
                    self.pk3.forget();
                    for collection in COLLECTIONS {
                        self.refresh_content(collection);
                    }
                    self.say("GameData set", false);
                } else {
                    self.say(format!("{} has no base/assets0.pk3; pick Jedi Academy's GameData", folder.display()), true);
                }
            }
            if let Some(path) = self.core.game_data()
                && ui.button("Open folder").clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", path.display())));
            }
        });
    }
    fn content_tab(&mut self, ui: &mut egui::Ui, collection: Collection) {
        let index = collection as usize;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            for other in COLLECTIONS {
                if theme::tab(ui, other.name(), 19.0, other == collection).clicked() {
                    self.tab = Tab::Content(other);
                }
            }
        });
        ui.add_space(10.0);
        ui.label(RichText::new(collection.about()).color(DIM));
        if self.core.game_data().is_none() {
            ui.label(RichText::new("Choose the GameData folder first.").color(BAD));
            return;
        }
        let progress = self.content[index].progress();
        let busy = self.content[index].busy();
        let plan = progress.snapshot.as_ref().map(|snapshot| Plan::new(snapshot, &self.selections[index]));
        let pending_here = self.pending.as_ref().is_some_and(|pending| pending.collection == collection);

        let summary = progress.snapshot.as_ref().and_then(|snapshot| self.scan_summary(snapshot));
        let quiet = match &progress.phase {
            ContentPhase::Idle => true,
            ContentPhase::Finished(outcome) => outcome.failures.is_empty(),
            _ => false,
        };

        ui.add_space(12.0);
        theme::rule(ui);
        // Buttons from the right first, so the status left of them gives way.
        fn status(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
            ui.add(egui::Label::new(text).truncate())
        }
        ui.horizontal(|ui| {
            ui.set_min_height(64.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if busy || pending_here {
                    if ui.button("Stop").clicked() {
                        self.content[index].cancel();
                        self.pending = None;
                    }
                } else if let Some(plan) = &plan {
                    let label = if !plan.downloads.is_empty() {
                        format!("Download {}", human_size(plan.download_bytes))
                    } else if !plan.removals.is_empty() {
                        format!("Remove {}", human_size(plan.removal_bytes))
                    } else {
                        "Up to date".to_owned()
                    };
                    let enabled = !plan.is_empty() && self.pending.is_none();
                    if theme::main_button(ui, &label, Vec2::new(0.0, 42.0), enabled).clicked() {
                        self.begin_download(collection, plan.downloads.clone());
                    }
                }
                if ui.add_enabled(!busy, egui::Button::new("Check again")).clicked() {
                    self.refresh_content(collection);
                }
                if let Some(snapshot) = &progress.snapshot
                    && ui
                        .add_enabled(!busy, egui::Button::new("Scan files here"))
                        .on_hover_text("Look inside every PK3 already on disk for harmful content")
                        .clicked()
                {
                    for (group_index, group) in snapshot.manifest.groups.iter().enumerate() {
                        for (asset_index, asset) in group.assets.iter().enumerate() {
                            let state = snapshot.state(group_index, asset_index);
                            if state.present() {
                                self.scan(collection, state, asset);
                            }
                        }
                    }
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    match &progress.phase {
                        _ if pending_here => {
                            let (done, total) = self.pending_progress();
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                                    .desired_width(ui.available_width().min(360.0))
                                    .text(format!("Checking {done} of {total} files for harmful content")),
                            );
                        }
                        // Once anything is scanned, the scan is the news; failures still win.
                        _ if quiet && summary.is_some() => _ = status(ui, summary.clone().unwrap_or_default().size(15.0)),
                        ContentPhase::Idle => {}
                        ContentPhase::Manifest => _ = status(ui, "Asking for the list…"),
                        ContentPhase::Checking { done, total } => {
                            ui.add(
                                egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                                    .desired_width(ui.available_width().min(300.0))
                                    .text("Checking files already here"),
                            );
                        }
                        ContentPhase::Removing { done, total } => _ = status(ui, format!("Removing {done} of {total}")),
                        ContentPhase::Downloading { file, files, name, done, total, .. } => {
                            ui.add(
                                egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                                    .desired_width(ui.available_width().min(420.0))
                                    .text(format!("{file} of {files}: {name}")),
                            );
                        }
                        ContentPhase::Finished(outcome) => {
                            let mut text = if outcome.synced {
                                format!("Done: {} downloaded ({}), {} removed", outcome.downloaded, human_size(outcome.downloaded_bytes), outcome.removed)
                            } else {
                                format!("List from {}", outcome.source)
                            };
                            if !outcome.failures.is_empty() {
                                text.push_str(&format!(", {} failed: {}", outcome.failures.len(), outcome.failures.join("; ")));
                            }
                            status(ui, RichText::new(text).color(if outcome.failures.is_empty() { DIM } else { BAD }));
                        }
                        ContentPhase::Failed(reason) => _ = status(ui, RichText::new(reason).color(BAD)),
                        ContentPhase::Cancelled => _ = status(ui, "Stopped; it carries on from where it was next time."),
                    }
                });
            });
        });
        theme::rule(ui);
        let Some(snapshot) = progress.snapshot else { return };

        let mut changed = collection == Collection::Sith && self.extras_row(ui, &snapshot);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(6.0);
            if snapshot.manifest.groups.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Nothing here yet.").color(DIM));
            }
            for (group_index, group) in snapshot.manifest.groups.iter().enumerate() {
                let key = (index, group.id);
                let open = self.open_groups.contains(&key);
                let total = group.assets.len();
                let here = (0..total).filter(|&asset| snapshot.state(group_index, asset).present()).count();
                let wanted = self.selections[index].wanted_in(group);
                let fetchable = group.assets.iter().any(|asset| asset.target == sjk_jof_content::Target::Base);
                let worst = group
                    .assets
                    .iter()
                    .filter_map(|asset| match self.scanner.state(&asset.md5) {
                        Some(ScanState::Done(report)) => report.level(),
                        _ => None,
                    })
                    .max();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.set_min_height(34.0);
                    let mut chosen = self.selections[index].chosen(group);
                    if ui.add_enabled(fetchable, egui::Checkbox::without_text(&mut chosen)).changed() {
                        self.selections[index].toggle(&snapshot.manifest, group.id);
                        changed = true;
                    }
                    ui.label(display(&group.name, 21.0));
                    ui.label(RichText::new(group.kind.label()).color(DIM));
                    if let Some(level @ (Level::Danger | Level::Warning)) = worst {
                        let (text, colour) = level_badge(level);
                        ui.label(RichText::new(text).color(colour)).on_hover_text("Open the files to see which");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let toggle = if open { "Hide files".to_owned() } else { format!("Show {total} files") };
                        if ui.link(toggle).clicked() {
                            if open {
                                self.open_groups.remove(&key);
                            } else {
                                self.open_groups.insert(key);
                            }
                        }
                        let size = human_size(group.assets.iter().map(|asset| asset.size).sum());
                        ui.label(RichText::new(format!("{here} of {total} here · {wanted} chosen · {size}")).color(DIM));
                    });
                });
                if !group.description.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(FILE_INDENT);
                        ui.label(RichText::new(&group.description).color(DIM));
                    });
                }
                if open {
                    ui.add_space(4.0);
                    for (asset_index, asset) in group.assets.iter().enumerate() {
                        let state = snapshot.state(group_index, asset_index);
                        changed |= self.file_row(ui, collection, group, asset, state);
                    }
                }
                ui.add_space(10.0);
                theme::rule(ui);
            }
        });
        if changed {
            self.save_selection(collection);
        }
    }

    /// The Sith cloud's extras pack on a switch of its own, above the groups, while
    /// the cloud offers it. Returns whether the choice changed.
    fn extras_row(&mut self, ui: &mut egui::Ui, snapshot: &sjk_jof_content::Snapshot) -> bool {
        let Some((group_index, asset_index)) = crate::content::extras(&snapshot.manifest) else { return false };
        let group = &snapshot.manifest.groups[group_index];
        let asset = &group.assets[asset_index];
        let state = snapshot.state(group_index, asset_index);
        let selection = &mut self.selections[Collection::Sith as usize];
        let mut wants = selection.wants(group, asset);
        let mut changed = false;
        ui.add_space(8.0);
        egui::Frame::NONE.fill(theme::RAISED).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(40.0);
                if ui.checkbox(&mut wants, "").changed() {
                    selection.toggle_file(group, asset);
                    changed = true;
                }
                ui.vertical(|ui| {
                    ui.label(display("SJK extras pack", 19.0));
                    ui.label(RichText::new("Unofficial emotes, skins and sounds, in one file.").color(DIM));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(human_size(asset.size)).color(DIM));
                    let (text, colour) = match (state, wants) {
                        (AssetState::Installed, true) => ("Here", DIM),
                        (AssetState::Installed | AssetState::Outdated, false) => ("To remove", GOLD_BRIGHT),
                        (AssetState::InGameFolder, _) => ("In game folder", DIM),
                        (AssetState::Outdated, true) => ("To update", GOLD_BRIGHT),
                        (AssetState::Missing, true) => ("To download", GOLD_BRIGHT),
                        _ => ("Off", DIM),
                    };
                    ui.label(RichText::new(text).color(colour));
                });
            });
        });
        changed
    }

    /// One file under its group: choice, name, scan, state, size, Inspect.
    /// Returns whether the choice changed. The file being inspected is lit.
    fn file_row(&mut self, ui: &mut egui::Ui, collection: Collection, group: &Group, asset: &Asset, state: AssetState) -> bool {
        let index = collection as usize;
        let inspected = self.inspect == Some((collection, group.id, asset.id));
        let lit = ui.painter().add(egui::Shape::Noop);
        let mut changed = false;
        let row = ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.add_space(FILE_INDENT);
            let mut wants = self.selections[index].wants(group, asset);
            if ui.add_enabled(state != AssetState::NotForSjk, egui::Checkbox::without_text(&mut wants)).changed() {
                self.selections[index].toggle_file(group, asset);
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let link = cell(ui, 56.0, |ui| ui.link(RichText::new("Inspect").size(14.0).color(GOLD_BRIGHT)));
                if link.clicked() {
                    self.scan(collection, state, asset);
                    self.inspect = if inspected { None } else { Some((collection, group.id, asset.id)) };
                    self.inspect_filter.clear();
                }
                cell(ui, 68.0, |ui| ui.label(RichText::new(human_size(asset.size)).color(DIM)));
                let (text, colour) = match state {
                    AssetState::Installed => ("Here", DIM),
                    AssetState::InGameFolder => ("In game folder", DIM),
                    AssetState::Outdated => ("To update", GOLD_BRIGHT),
                    AssetState::Missing if wants => ("To download", GOLD_BRIGHT),
                    AssetState::Missing => ("Not chosen", DIM),
                    AssetState::NotForSjk => ("JoF's client only", DIM),
                };
                cell(ui, 118.0, |ui| ui.label(RichText::new(text).color(colour)));
                cell(ui, 104.0, |ui| match self.scanner.state(&asset.md5) {
                    Some(ScanState::Queued) => ui.label(RichText::new("scanning…").color(DIM)),
                    Some(ScanState::Done(report)) => {
                        let (text, colour) = report.level().map_or(CLEAN, level_badge);
                        ui.label(RichText::new(text).color(colour))
                    }
                    Some(ScanState::Failed(_)) => ui.label(RichText::new("not read").color(BAD)),
                    None => ui.label(""),
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = ui.add(egui::Label::new(&asset.name).truncate());
                    if !asset.description.is_empty() {
                        name.on_hover_text(&asset.description);
                    }
                });
            });
        });
        if inspected {
            let rect = row.response.rect.expand2(Vec2::new(8.0, 0.0));
            ui.painter().set(lit, egui::Shape::rect_filled(rect, 4.0, theme::gold(34)));
        }
        changed
    }

    /// Where a manifest file can be read: on disk when it is here as the
    /// manifest has it, else on the collection's hosts.
    fn location(&self, collection: Collection, state: AssetState, asset: &Asset) -> Option<Location> {
        let settings = self.settings[collection as usize].as_ref()?;
        Some(match state {
            AssetState::Installed => Location::Local(settings.root.join("base").join(&asset.name)),
            AssetState::InGameFolder => Location::Local(self.core.game_data()?.join("base").join(&asset.name)),
            _ => Location::Remote {
                urls: settings.sources.iter().map(|source| source.file_url(asset)).collect(),
                size: asset.size,
            },
        })
    }

    fn scan(&self, collection: Collection, state: AssetState, asset: &Asset) {
        if let Some(location) = self.location(collection, state, asset) {
            self.scanner.request(asset.md5, location);
        }
    }

    /// "Scanned 40 of 52 files: 1 harmful, 3 with warnings", once any is scanned.
    fn scan_summary(&self, snapshot: &sjk_jof_content::Snapshot) -> Option<RichText> {
        let (mut scanned, mut danger, mut warning, mut failed, mut total) = (0, 0, 0, 0, 0);
        for (_, asset) in snapshot.manifest.assets() {
            if asset.target != sjk_jof_content::Target::Base {
                continue;
            }
            total += 1;
            match self.scanner.state(&asset.md5) {
                Some(ScanState::Done(report)) => {
                    scanned += 1;
                    match report.level() {
                        Some(Level::Danger) => danger += 1,
                        Some(Level::Warning) => warning += 1,
                        _ => {}
                    }
                }
                Some(ScanState::Failed(_)) => failed += 1,
                _ => {}
            }
        }
        if scanned + failed == 0 {
            return None;
        }
        let mut text = format!("Scanned {scanned} of {total} · {danger} harmful · {warning} with warnings");
        if failed > 0 {
            text.push_str(&format!(" · {failed} not read"));
        }
        let colour = if danger > 0 { BAD } else if warning > 0 { WARN } else { HOLO };
        Some(RichText::new(text).color(colour))
    }

    /// Scan what a download would fetch first; the download starts once every
    /// scan is back and nothing harmful is left, or after the player decides.
    fn begin_download(&mut self, collection: Collection, files: Vec<(usize, usize)>) {
        let Some(snapshot) = self.content[collection as usize].progress().snapshot else { return };
        for &(group, asset) in &files {
            let asset_ref = &snapshot.manifest.groups[group].assets[asset];
            // Scan what would arrive: the server's copy, not an outdated one here.
            self.scan(collection, AssetState::Missing, asset_ref);
        }
        self.pending = Some(Pending { collection, files, snapshot });
    }

    fn pending_progress(&self) -> (usize, usize) {
        let Some(pending) = &self.pending else { return (0, 0) };
        let done = pending
            .files
            .iter()
            .filter(|&&(group, asset)| {
                !matches!(self.scanner.state(&pending.snapshot.manifest.groups[group].assets[asset].md5), Some(ScanState::Queued) | None)
            })
            .count();
        (done, pending.files.len())
    }

    /// The pending download's harmful files the player has not trusted, with their reports.
    fn harmful(&self, pending: &Pending) -> Vec<((usize, usize), Arc<Report>)> {
        let trusted = self.core.config().trusted.clone();
        pending
            .files
            .iter()
            .filter_map(|&(group, asset)| {
                let asset_ref = &pending.snapshot.manifest.groups[group].assets[asset];
                match self.scanner.state(&asset_ref.md5) {
                    Some(ScanState::Done(report))
                        if report.level() == Some(Level::Danger) && !trusted.contains(&sjk_jof_content::md5_hex(&asset_ref.md5)) =>
                    {
                        Some(((group, asset), report))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn start_sync(&mut self, collection: Collection) {
        if let Some(settings) = self.settings[collection as usize].clone() {
            self.content[collection as usize].sync(settings, self.selections[collection as usize].clone());
        }
    }

    /// Move a pending download on: wait for its scans, then download or ask.
    fn advance_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else { return };
        let (done, total) = self.pending_progress();
        if done < total {
            return;
        }
        let collection = pending.collection;
        let harmful = self.harmful(pending);
        if harmful.is_empty() {
            self.pending = None;
            self.start_sync(collection);
            return;
        }
        let mut decision = None;
        egui::Window::new("Harmful content found")
            .collapsible(false)
            .resizable(true)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} of the files to download contain things that could harm your computer. They were not downloaded.",
                    harmful.len()
                ));
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for ((group, asset), report) in &harmful {
                        let asset_ref = &pending.snapshot.manifest.groups[*group].assets[*asset];
                        ui.label(RichText::new(&asset_ref.name).strong().color(BAD));
                        for finding in report.findings.iter().filter(|finding| finding.level == Level::Danger) {
                            ui.label(format!("   {}  {}", finding.path, finding.text));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Skip them, download the rest").strong()).clicked() {
                        decision = Some(Decision::Skip);
                    }
                    if ui.button("Download anyway").on_hover_text("Only if you trust where these files come from").clicked() {
                        decision = Some(Decision::Trust);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(Decision::Cancel);
                    }
                });
            });
        let Some(decision) = decision else { return };
        let Some(pending) = self.pending.take() else { return };
        if decision == Decision::Cancel {
            return;
        }
        if decision == Decision::Skip {
            let selection = &mut self.selections[collection as usize];
            for ((group, asset), _) in &harmful {
                let group = &pending.snapshot.manifest.groups[*group];
                let asset = &group.assets[*asset];
                if selection.wants(group, asset) {
                    selection.toggle_file(group, asset);
                }
            }
            self.save_selection(collection);
        } else {
            self.core.edit(|config| {
                for ((group, asset), _) in &harmful {
                    let md5 = &pending.snapshot.manifest.groups[*group].assets[*asset].md5;
                    config.trusted.insert(sjk_jof_content::md5_hex(md5));
                }
            });
        }
        self.start_sync(collection);
    }

    /// Docked at the right of a collection's page: what is inside one PK3 and
    /// what the scan found, with what the levels mean at its foot.
    fn inspect_panel(&mut self, ctx: &egui::Context) {
        let Some((collection, group_id, asset_id)) = self.inspect else { return };
        if self.tab != Tab::Content(collection) {
            return;
        }
        let Some(snapshot) = self.content[collection as usize].progress().snapshot else { return };
        let found = snapshot.manifest.groups.iter().enumerate().find_map(|(group_index, group)| {
            let asset_index = group.assets.iter().position(|asset| asset.id == asset_id)?;
            (group.id == group_id).then_some((group_index, asset_index))
        });
        let Some((group_index, asset_index)) = found else {
            self.inspect = None;
            return;
        };
        let asset = snapshot.manifest.groups[group_index].assets[asset_index].clone();
        let state = snapshot.state(group_index, asset_index);
        let report = match self.scanner.state(&asset.md5) {
            Some(ScanState::Done(report)) => Some(report),
            _ => None,
        };
        let mut close = false;
        egui::SidePanel::right("inspect")
            .exact_width(INSPECT_WIDTH)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::RAISED).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    legend(ui);
                    ui.add_space(12.0);
                    theme::rule(ui);
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                close = theme::window_button(ui, WindowButton::Close, 32.0).on_hover_text("Close").clicked();
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.add(egui::Label::new(display(&asset.name, 25.0)).truncate());
                                });
                            });
                        });
                        let from = if state.present() { "read from disk" } else { "read from the server, not downloaded" };
                        let meta = match &report {
                            Some(report) => format!(
                                "{} · {} files, {} unpacked · {from}",
                                human_size(report.archive_size),
                                report.entries.iter().filter(|entry| !entry.name.ends_with('/')).count(),
                                human_size(report.expanded_size)
                            ),
                            None => format!("{} · {from}", human_size(asset.size)),
                        };
                        ui.label(RichText::new(meta).size(13.0).color(DIM))
                            .on_hover_text(format!("MD5 {}", sjk_jof_content::md5_hex(&asset.md5)));
                        ui.add_space(10.0);
                        match (self.scanner.state(&asset.md5), report) {
                            (_, Some(report)) => self.report_view(ui, &report),
                            (Some(ScanState::Failed(error)), _) => {
                                ui.label(RichText::new(format!("Could not read it: {error}")).color(BAD));
                                if ui.button("Try again").clicked() {
                                    self.scan(collection, state, &asset);
                                }
                            }
                            _ => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Reading the archive…");
                                });
                            }
                        }
                    });
                });
            });
        if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.inspect = None;
        }
    }

    fn report_view(&mut self, ui: &mut egui::Ui, report: &Report) {
        findings_view(ui, report);
        ui.add_space(14.0);
        ui.label(RichText::new("Files inside").size(13.0).color(DIM));
        ui.add(
            egui::TextEdit::singleline(&mut self.inspect_filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY)
                .margin(Vec2::new(10.0, 8.0)),
        );
        ui.add_space(6.0);
        // Entries the scan spoke about keep its colour in the list.
        let flagged: std::collections::HashMap<&str, Level> = report.findings.iter().map(|finding| (finding.path.as_str(), finding.level)).collect();
        let filter = self.inspect_filter.to_ascii_lowercase();
        // Folders are entries of their own in a ZIP; only the files matter here.
        let mut rows: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| !entry.name.ends_with('/'))
            .filter(|entry| filter.is_empty() || entry.name.to_ascii_lowercase().contains(&filter))
            .collect();
        rows.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        let height = 22.0;
        egui::ScrollArea::vertical().id_salt("entries").auto_shrink(false).show_rows(ui, height, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for entry in &rows[range] {
                let colour = flagged.get(entry.name.as_str()).map_or(theme::TEXT, |&level| level_mark(level).1);
                ui.horizontal(|ui| {
                    ui.set_height(height);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(human_size(entry.size)).size(13.0).color(DIM));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(&entry.name).monospace().size(13.0).color(colour)).truncate());
                        });
                    });
                });
            }
        });
    }

    /// Open the SJK chat window, or bring it forward.
    fn open_chat(&mut self, ctx: &egui::Context) {
        if self.chat_open {
            ctx.send_viewport_cmd_to(chat_viewport(), egui::ViewportCommand::Focus);
        }
        self.chat_open = true;
    }

    /// SJK chat in a window of its own, beside the launcher's. It reads the hub only
    /// while open, and not while a game started from here runs (the game reads it).
    fn chat_window(&mut self, ctx: &egui::Context) {
        let playing = GAMES.iter().any(|&game| self.core.running(game));
        let folder = self.core.game_data().map(|game_data| game_data.join("SJK"));
        self.chat.set_active(self.chat_open, playing, folder);
        if !self.chat_open {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("SJK chat")
            .with_inner_size([460.0, 660.0])
            .with_min_inner_size([360.0, 420.0])
            .with_icon(Arc::clone(&self.icon))
            .with_decorations(false);
        ctx.show_viewport_immediate(chat_viewport(), builder, |ctx, _| {
            if ctx.input(|input| input.viewport().close_requested()) {
                self.chat_open = false;
            }
            self.chat_page(ctx);
            window_edges(ctx);
        });
    }

    fn chat_page(&mut self, ctx: &egui::Context) {
        let view = self.chat.view();
        let (status, lines, online, own, sending, refused) =
            (view.status.clone(), view.lines.clone(), view.online, view.own_key.clone(), view.sending, view.refused.clone());
        drop(view);
        let live = status == ChatStatus::Live;

        egui::TopBottomPanel::top("chat-bar")
            .frame(egui::Frame::NONE.fill(SPACE).inner_margin(egui::Margin { left: 22, right: 12, top: 12, bottom: 10 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = Rect::from_min_max(ctx.screen_rect().min, egui::pos2(ctx.screen_rect().right(), ui.max_rect().top() + 40.0));
                let bar = ui.interact(bar, egui::Id::new("chat-title-bar"), Sense::click_and_drag());
                if bar.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    ui.label(display("SJK chat", 24.0).color(theme::TEXT));
                    if live {
                        ui.label(RichText::new(format!("{online} online")).color(DIM));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if theme::window_button(ui, WindowButton::Close, 32.0).clicked() {
                            self.chat_open = false;
                        }
                        if theme::window_button(ui, WindowButton::Minimise, 32.0).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("chat-field")
            .frame(egui::Frame::NONE.fill(SPACE).inner_margin(egui::Margin { left: 22, right: 22, top: 0, bottom: 18 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                theme::rule(ui);
                ui.add_space(12.0);
                let said = self.chat_error.clone().or(refused);
                if let Some(why) = said {
                    ui.label(RichText::new(why).color(BAD));
                    ui.add_space(4.0);
                }
                let mut send = false;
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = live && !sending && !self.chat_draft.trim().is_empty();
                        send |= theme::main_button(ui, if sending { "Sending" } else { "Send" }, Vec2::new(86.0, 38.0), ready).clicked();
                        let count = self.chat_draft.chars().count();
                        let colour = if count > sjk_identity::chat::TEXT_MAX { BAD } else { DIM };
                        ui.label(RichText::new(format!("{count}/{}", sjk_identity::chat::TEXT_MAX)).size(13.0).color(colour));
                        let field = ui.add_enabled(
                            live,
                            egui::TextEdit::singleline(&mut self.chat_draft)
                                .hint_text("Message SJK chat")
                                .desired_width(f32::INFINITY)
                                .margin(Vec2::new(10.0, 9.0)),
                        );
                        if field.changed() {
                            self.chat_error = None;
                        }
                        if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                            send = true;
                            field.request_focus();
                        }
                    });
                });
                if send && !sending {
                    match self.chat.send(&self.chat_draft) {
                        Ok(()) => self.chat_draft.clear(),
                        Err(why) => self.chat_error = Some(why),
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(SPACE).inner_margin(egui::Margin { left: 22, right: 22, top: 4, bottom: 8 }))
            .show(ctx, |ui| {
                let note = match &status {
                    ChatStatus::Idle | ChatStatus::Connecting => Some(("Connecting to SJK chat…".to_owned(), DIM)),
                    ChatStatus::Paused => Some(("Paused while you play: SJK chat is in the game".to_owned(), DIM)),
                    ChatStatus::Unavailable(why) => Some((why.clone(), BAD)),
                    ChatStatus::Failing(why) => Some((format!("{why}, trying again"), BAD)),
                    ChatStatus::Live => None,
                };
                if let Some((text, colour)) = note {
                    ui.label(RichText::new(text).color(colour));
                    ui.add_space(8.0);
                }
                if live && lines.is_empty() {
                    ui.label(RichText::new("Nothing said yet.").color(DIM));
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64);
                egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    for line in &lines {
                        chat_line(ui, line, line.key_id == own, now);
                    }
                });
            });
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Game folder");
        self.game_data_row(ui);
        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        let mut config = self.core.config().clone();
        let before = config.clone();

        ui.heading("Launcher");
        if ui.checkbox(&mut config.auto_start, "Start with my computer, in the tray").changed()
            && let Err(error) = system::set_autostart(config.auto_start)
        {
            self.say(format!("Start with the computer: {error}"), true);
            config.auto_start = !config.auto_start;
        }
        ui.checkbox(&mut config.close_to_tray, "Closing the window keeps the launcher in the tray");
        ui.checkbox(&mut config.auto_update, "Install new versions by themselves (when the game is closed)");
        ui.horizontal(|ui| {
            ui.label("Check for new versions every");
            ui.add(egui::DragValue::new(&mut config.check_hours).range(1..=48).suffix(" h"));
        });

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Versions");
        for game in GAMES {
            let mut on = config.prereleases(game);
            if ui.checkbox(&mut on, format!("Offer {} pre-releases", game.name())).changed() {
                config.prereleases.insert(game, on);
            }
        }

        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);
        ui.heading("Sith cloud");
        ui.horizontal(|ui| {
            ui.label("Server");
            ui.add(
                egui::TextEdit::singleline(&mut config.sith_url)
                    .hint_text("the one the SJK hub names")
                    .desired_width(320.0),
            );
            if ui.small_button("Default").clicked() {
                config.sith_url.clear();
            }
        });

        ui.add_space(8.0);
        if let Some(folder) = crate::config::folder() {
            ui.label(RichText::new(format!("Settings: {}", folder.join("launcher.json").display())).color(DIM));
        }

        if format!("{config:?}") != format!("{before:?}") {
            let prereleases_changed = config.prereleases != before.prereleases;
            self.core.edit(|saved| *saved = config);
            if prereleases_changed {
                self.core.check_all();
            }
        }
    }
}

/// A download waiting for its files' scans.
struct Pending {
    collection: Collection,
    files: Vec<(usize, usize)>,
    snapshot: Arc<sjk_jof_content::Snapshot>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Skip,
    Trust,
    Cancel,
}

const CLEAN: (&str, Color32) = ("✔ clean", HOLO);

/// The scan's verdict and what it found, for the inspector and the PK3s tab.
fn findings_view(ui: &mut egui::Ui, report: &Report) {
    let (verdict, colour) = match report.level() {
        Some(Level::Danger) => ("Harmful: it can run code or damage your files", BAD),
        Some(Level::Warning) => ("Look before using: it changes settings or carries unusual files", WARN),
        _ => ("Clean: only game content", HOLO),
    };
    ui.label(strong(verdict, 16.0).color(colour));
    if !report.findings.is_empty() {
        ui.add_space(4.0);
        egui::ScrollArea::vertical().id_salt("findings").max_height(150.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for finding in &report.findings {
                let (mark, colour) = level_mark(finding.level);
                let mut job = egui::text::LayoutJob::default();
                let style = ui.style().clone();
                let mut add = |text: RichText| text.append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Center);
                add(RichText::new(format!("{mark} ")).color(colour).size(14.0));
                if !finding.path.is_empty() {
                    add(RichText::new(format!("{} ", finding.path)).monospace().size(13.0).color(theme::TEXT));
                }
                add(RichText::new(&finding.text).size(14.0).color(DIM));
                ui.add(egui::Label::new(job).wrap());
            }
        });
    }
}

fn level_mark(level: Level) -> (&'static str, Color32) {
    match level {
        Level::Danger => ("■", BAD),
        Level::Warning => ("!", WARN),
        Level::Info => ("i", DIM),
    }
}

fn level_badge(level: Level) -> (&'static str, Color32) {
    match level {
        Level::Danger => ("■ harmful", BAD),
        Level::Warning => ("! warnings", WARN),
        Level::Info => ("i notes", DIM),
    }
}

/// What the scan's colours mean, at the inspector's foot (laid out bottom-up).
fn legend(ui: &mut egui::Ui) {
    let lines = [
        ("✔ Clean", HOLO, "only game content"),
        ("! Warnings", WARN, "scripts, nested archives, risky console commands"),
        ("■ Harmful", BAD, "native code, escapes from the game folder, ZIP bombs: held back"),
    ];
    for (name, colour, meaning) in lines {
        let mut job = egui::text::LayoutJob::default();
        let style = ui.style().clone();
        RichText::new(format!("{name}  ")).size(13.0).color(colour).append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Center);
        RichText::new(meaning).size(13.0).color(DIM).append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Center);
        ui.add(egui::Label::new(job).wrap());
    }
}

fn chat_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("sjk-chat")
}

/// One message as SJK's chat page lays it out: the name in its colours, the verified
/// tick, the text in the SJK chat's gold flowing after it; Staff and the time on the
/// right of its first row.
fn chat_line(ui: &mut egui::Ui, line: &crate::chat::Line, own: bool, now: i64) {
    use egui::text::{LayoutJob, TextFormat};
    let body = egui::FontId::proportional(15.0);
    let strong = egui::FontId::new(15.0, egui::FontFamily::Name("body-strong".into()));
    let mut job = LayoutJob::default();
    for (text, colour) in coloured(&line.name) {
        job.append(&text, 0.0, TextFormat::simple(strong.clone(), colour));
    }
    if line.verified {
        job.append(" ✔", 0.0, TextFormat::simple(body.clone(), GOLD_BRIGHT));
    }
    job.append(": ", 0.0, TextFormat::simple(body.clone(), DIM));
    job.append(&line.text, 0.0, TextFormat::simple(body, crate::chat::GOLD));
    let lit = ui.painter().add(egui::Shape::Noop);
    let row = ui.horizontal_top(|ui| {
        let side = 96.0;
        let width = (ui.available_width() - side).max(120.0);
        ui.allocate_ui_with_layout(Vec2::new(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(width);
            ui.add(egui::Label::new(job).wrap());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.label(RichText::new(crate::chat::ago(line.at, now)).size(12.5).color(DIM));
            if line.staff {
                ui.label(RichText::new("Staff").size(12.5).color(HOLO));
            }
        });
    });
    // The player's own lines carry a thin gold mark at their left.
    if own {
        let rect = row.response.rect;
        let mark = Rect::from_min_max(egui::pos2(rect.left() - 10.0, rect.top() + 2.0), egui::pos2(rect.left() - 8.0, rect.bottom() - 2.0));
        ui.painter().set(lit, egui::Shape::rect_filled(mark, 1.0, GOLD));
    }
}

/// A name's runs in Jedi Academy's colours (`^0` to `^9`), as SJK lifts them to read
/// on its dark ground; white before any code.
fn coloured(name: &str) -> Vec<(String, Color32)> {
    const PALETTE: [Color32; 10] = [
        Color32::from_rgb(0x8A, 0x8F, 0x99),
        Color32::from_rgb(0xFF, 0x5C, 0x5C),
        Color32::from_rgb(0x6B, 0xE3, 0x6B),
        Color32::from_rgb(0xFF, 0xE1, 0x5A),
        Color32::from_rgb(0x70, 0x8C, 0xFF),
        Color32::from_rgb(0x5A, 0xE6, 0xE6),
        Color32::from_rgb(0xE6, 0x6B, 0xE6),
        Color32::from_rgb(0xF2, 0xF4, 0xF8),
        Color32::from_rgb(0xFF, 0x9A, 0x3D),
        Color32::from_rgb(0xA6, 0xA6, 0xA6),
    ];
    let mut runs: Vec<(String, Color32)> = vec![(String::new(), PALETTE[7])];
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && let Some(digit) = chars.peek().and_then(|next| next.to_digit(10))
        {
            chars.next();
            runs.push((String::new(), PALETTE[digit as usize]));
        } else if let Some((text, _)) = runs.last_mut() {
            text.push(c);
        }
    }
    runs.retain(|(text, _)| !text.is_empty());
    runs
}

/// A fixed-width column in a right-to-left row.
fn cell<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = Vec2::new(width, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        add(ui)
    })
    .inner
}
