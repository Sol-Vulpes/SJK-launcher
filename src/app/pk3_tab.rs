//! The PK3s tab: every PK3 in `GameData`, searched by name and by the files inside
//! them, with what each holds, which PK3 replaces which, the safety scan, and
//! turning PK3s off or deleting them (one, or all those ticked).

use super::{App, BAD, DIM, Tab, cell, level_badge, level_mark};
use crate::content::{COLLECTIONS, Collection};
use crate::pk3s::{self, Index, Owner, Pk3};
use crate::pk3scan::{Level, Location};
use crate::scans::ScanState;
use crate::theme::{self, GOLD, GOLD_BRIGHT, HOLO, WindowButton, display, strong};
use eframe::egui::{self, Color32, RichText, Vec2};
use sjk_jof_content::human_size;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PANEL_WIDTH: f32 = 440.0;
const ROW_HEIGHT: f32 = 50.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    All,
    On,
    Off,
    /// The player's own: neither the games' files nor a content collection's.
    Yours,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Size,
    Date,
}

/// Files inside one PK3 that match the search: how many, and the first few.
#[derive(Clone, Default)]
struct Inside {
    count: usize,
    first: Vec<String>,
}

/// The search's answer for one index, kept until either changes.
struct Search {
    query: String,
    index: Arc<Index>,
    /// Per PK3: does its name match, and what inside matches.
    names: Vec<bool>,
    inside: Vec<Inside>,
}

#[derive(Default)]
pub(super) struct Pk3Tab {
    asked: bool,
    indexer: pk3s::Indexer,
    query: String,
    folder: Option<String>,
    show: Option<Show>,
    sort: Option<Sort>,
    ticked: HashSet<PathBuf>,
    /// The PK3 shown in the side panel.
    open: Option<PathBuf>,
    file_filter: String,
    /// PK3s waiting for the player to confirm their deletion.
    confirm: Option<Vec<PathBuf>>,
    search: Option<Search>,
}

impl Pk3Tab {
    /// Read the folder again next time the tab shows (GameData changed).
    pub(super) fn forget(&mut self) {
        self.asked = false;
        self.ticked.clear();
        self.open = None;
    }

    pub(super) fn busy(&self) -> bool {
        self.indexer.reading().is_some()
    }
}

/// Which content collection put a file where it is, and the manifest entry.
#[derive(Clone, Copy)]
struct Managed {
    collection: Collection,
    group: usize,
    asset: usize,
}

fn same_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
}

/// `dd/mm/yyyy` of a file time (UTC).
fn day(time: std::time::SystemTime) -> String {
    let days = time.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs() / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}/{m:02}/{y}")
}

impl App {
    fn pk3_index(&mut self) -> Option<Arc<Index>> {
        if !self.pk3.asked
            && let Some(game_data) = self.core.game_data()
        {
            self.pk3.asked = true;
            let ctx = self.core.ctx.clone();
            self.pk3.indexer.refresh(game_data, move || ctx.request_repaint());
        }
        self.pk3.indexer.index()
    }

    fn pk3_reread(&mut self) {
        if let Some(game_data) = self.core.game_data() {
            let ctx = self.core.ctx.clone();
            self.pk3.indexer.refresh(game_data, move || ctx.request_repaint());
        }
    }

    /// The files the content collections put on disk, by path.
    fn managed(&self) -> HashMap<String, Managed> {
        let mut managed = HashMap::new();
        let game_data = self.core.game_data();
        for collection in COLLECTIONS {
            let index = collection as usize;
            let (Some(snapshot), Some(settings)) = (self.content[index].progress().snapshot, &self.settings[index]) else { continue };
            for (group_index, group) in snapshot.manifest.groups.iter().enumerate() {
                for (asset_index, asset) in group.assets.iter().enumerate() {
                    let path = match snapshot.state(group_index, asset_index) {
                        sjk_jof_content::AssetState::Installed => settings.root.join("base").join(&asset.name),
                        sjk_jof_content::AssetState::InGameFolder => match &game_data {
                            Some(game_data) => game_data.join("base").join(&asset.name),
                            None => continue,
                        },
                        _ => continue,
                    };
                    managed.insert(same_path(&path), Managed { collection, group: group_index, asset: asset_index });
                }
            }
        }
        managed
    }

    /// The search over names and insides, worked out again only when the query or
    /// the index changes.
    fn pk3_search(&mut self, index: &Arc<Index>) {
        let query = self.pk3.query.trim().to_ascii_lowercase();
        if self.pk3.search.as_ref().is_some_and(|search| search.query == query && Arc::ptr_eq(&search.index, index)) {
            return;
        }
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut names = Vec::with_capacity(index.pk3s.len());
        let mut inside = Vec::with_capacity(index.pk3s.len());
        for pk3 in &index.pk3s {
            let name = format!("{}/{}", pk3.folder, pk3.name).to_ascii_lowercase();
            names.push(words.iter().all(|word| name.contains(word)));
            let mut found = Inside::default();
            if !words.is_empty() {
                for (file, _) in &pk3.contents.files {
                    let lower = file.to_ascii_lowercase();
                    if words.iter().all(|word| lower.contains(word)) {
                        found.count += 1;
                        if found.first.len() < 3 {
                            found.first.push(file.clone());
                        }
                    }
                }
            }
            inside.push(found);
        }
        self.pk3.search = Some(Search { query, index: Arc::clone(index), names, inside });
    }

    /// The PK3s to list, in the order chosen.
    fn pk3_rows(&self, index: &Index, managed: &HashMap<String, Managed>) -> Vec<usize> {
        let Some(search) = &self.pk3.search else { return Vec::new() };
        let searching = !search.query.is_empty();
        let show = self.pk3.show.unwrap_or(Show::All);
        let mut rows: Vec<usize> = (0..index.pk3s.len())
            .filter(|&at| {
                let pk3 = &index.pk3s[at];
                let folder_ok = self.pk3.folder.as_ref().is_none_or(|folder| folder == &pk3.folder);
                let show_ok = match show {
                    Show::All => true,
                    Show::On => pk3.on,
                    Show::Off => !pk3.on,
                    Show::Yours => pk3.owner == Owner::Player && !managed.contains_key(&same_path(&pk3.on_path())),
                };
                let search_ok = !searching || search.names[at] || search.inside[at].count > 0;
                folder_ok && show_ok && search_ok
            })
            .collect();
        match self.pk3.sort.unwrap_or(Sort::Name) {
            Sort::Name => {}
            Sort::Size => rows.sort_by(|&a, &b| index.pk3s[b].size.cmp(&index.pk3s[a].size)),
            Sort::Date => rows.sort_by(|&a, &b| index.pk3s[b].modified.cmp(&index.pk3s[a].modified)),
        }
        // A name match comes before a match inside only.
        if searching {
            rows.sort_by_key(|&at| !search.names[at]);
        }
        rows
    }

    pub(super) fn pk3_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        if self.core.game_data().is_none() {
            ui.label(RichText::new("Choose the GameData folder in Settings first.").color(BAD));
            return;
        }
        let index = self.pk3_index();
        let managed = self.managed();
        if let Some(index) = &index {
            self.pk3_search(index);
        }

        // The tally, and what can be done to all.
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            ui.label(display("PK3s", 26.0).color(theme::TEXT));
            if let Some(index) = &index {
                let size: u64 = index.pk3s.iter().map(|pk3| pk3.size).sum();
                let off = index.pk3s.iter().filter(|pk3| !pk3.on).count();
                let mut tally = format!("{} files · {}", index.pk3s.len(), human_size(size));
                if off > 0 {
                    tally.push_str(&format!(" · {off} off"));
                }
                ui.label(RichText::new(tally).color(DIM));
            }
            if let Some((done, total)) = self.pk3.indexer.reading() {
                ui.spinner();
                ui.label(RichText::new(if total == 0 { "Looking…".to_owned() } else { format!("Reading {done} of {total}") }).color(DIM));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(game_data) = self.core.game_data()
                    && ui.button("Open GameData").clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", game_data.display())));
                }
                if let Some(index) = &index
                    && ui.button("Scan all").on_hover_text("Look inside every PK3 for harmful content (reads only their lists and configs)").clicked()
                {
                    for pk3 in &index.pk3s {
                        self.scanner.request(pk3.key, Location::Local(pk3.path.clone()));
                    }
                }
                if ui.add_enabled(!self.pk3.busy(), egui::Button::new("Refresh")).clicked() {
                    self.pk3_reread();
                }
            });
        });
        ui.add_space(4.0);

        // The search and the filters.
        let folders: Vec<String> = index.as_ref().map_or_else(Vec::new, |index| {
            let mut folders: Vec<String> = Vec::new();
            for pk3 in &index.pk3s {
                if !folders.contains(&pk3.folder) {
                    folders.push(pk3.folder.clone());
                }
            }
            folders
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let sort = self.pk3.sort.get_or_insert(Sort::Name);
                egui::ComboBox::from_id_salt("pk3-sort")
                    .selected_text(match sort {
                        Sort::Name => "By name",
                        Sort::Size => "Largest first",
                        Sort::Date => "Newest first",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(sort, Sort::Name, "By name");
                        ui.selectable_value(sort, Sort::Size, "Largest first");
                        ui.selectable_value(sort, Sort::Date, "Newest first");
                    });
                let show = self.pk3.show.get_or_insert(Show::All);
                egui::ComboBox::from_id_salt("pk3-show")
                    .selected_text(match show {
                        Show::All => "All",
                        Show::On => "On",
                        Show::Off => "Off",
                        Show::Yours => "Added by you",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(show, Show::All, "All");
                        ui.selectable_value(show, Show::On, "On");
                        ui.selectable_value(show, Show::Off, "Off");
                        ui.selectable_value(show, Show::Yours, "Added by you")
                            .on_hover_text("Not the games' own files and not from JoF content or the Sith cloud");
                    });
                egui::ComboBox::from_id_salt("pk3-folder")
                    .selected_text(self.pk3.folder.clone().unwrap_or_else(|| "Every folder".into()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.pk3.folder, None, "Every folder");
                        for folder in &folders {
                            ui.selectable_value(&mut self.pk3.folder, Some(folder.clone()), folder);
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut self.pk3.query)
                        .hint_text("Search PK3s and the files inside them: a map, a model, a sound…")
                        .desired_width(f32::INFINITY)
                        .margin(Vec2::new(10.0, 8.0)),
                );
            });
        });
        ui.add_space(6.0);

        let Some(index) = index else { return };
        let rows = self.pk3_rows(&index, &managed);
        self.pk3_bulk_bar(ui, &index);
        theme::rule(ui);
        if rows.is_empty() {
            ui.add_space(12.0);
            let text = if self.pk3.query.trim().is_empty() { "No PK3s here." } else { "Nothing matches." };
            ui.label(RichText::new(text).color(DIM));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for &at in &rows[range] {
                self.pk3_row(ui, &index, at, &managed);
            }
        });
    }

    /// Shown while PK3s are ticked: what can be done to them all.
    fn pk3_bulk_bar(&mut self, ui: &mut egui::Ui, index: &Index) {
        self.pk3.ticked.retain(|path| index.find(path).is_some());
        if self.pk3.ticked.is_empty() {
            return;
        }
        let ticked: Vec<&Pk3> = index.pk3s.iter().filter(|pk3| self.pk3.ticked.contains(&pk3.path)).collect();
        let size: u64 = ticked.iter().map(|pk3| pk3.size).sum();
        let (on, off) = (ticked.iter().filter(|pk3| pk3.on).count(), ticked.iter().filter(|pk3| !pk3.on).count());
        let mut act = None;
        egui::Frame::NONE.fill(theme::gold(26)).inner_margin(egui::Margin::symmetric(14, 8)).corner_radius(4).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(strong(format!("{} ticked · {}", ticked.len(), human_size(size)), 15.0).color(GOLD_BRIGHT));
                ui.add_space(12.0);
                if on > 0 && ui.button(format!("Turn off {on}")).clicked() {
                    act = Some(Some(false));
                }
                if off > 0 && ui.button(format!("Turn on {off}")).clicked() {
                    act = Some(Some(true));
                }
                if ui.button(RichText::new("Delete…").color(BAD)).clicked() {
                    act = Some(None);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.link("Untick all").clicked() {
                        self.pk3.ticked.clear();
                    }
                });
            });
        });
        ui.add_space(6.0);
        let paths: Vec<PathBuf> = ticked.iter().map(|pk3| pk3.path.clone()).collect();
        match act {
            Some(Some(on)) => self.pk3_set_on(index, &paths, on),
            Some(None) => self.pk3.confirm = Some(paths),
            None => {}
        }
    }

    fn pk3_row(&mut self, ui: &mut egui::Ui, index: &Index, at: usize, managed: &HashMap<String, Managed>) {
        let pk3 = &index.pk3s[at];
        let open = self.pk3.open.as_ref() == Some(&pk3.path);
        let lit = ui.painter().add(egui::Shape::Noop);
        let source = managed.get(&same_path(&pk3.on_path())).map(|managed| managed.collection);
        let row = ui.horizontal(|ui| {
            ui.set_height(ROW_HEIGHT);
            let mut ticked = self.pk3.ticked.contains(&pk3.path);
            let tick = ui
                .add_enabled(!pk3.owner.protected(), egui::Checkbox::without_text(&mut ticked))
                .on_disabled_hover_text(format!("{}: it stays", pk3.owner.label()));
            if tick.changed() {
                if ticked {
                    self.pk3.ticked.insert(pk3.path.clone());
                } else {
                    self.pk3.ticked.remove(&pk3.path);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                // With the panel open the list narrows: the folder and date make way.
                let wide = ui.available_width() > 980.0;
                if wide {
                    cell(ui, 84.0, |ui| ui.label(RichText::new(pk3.modified.map(day).unwrap_or_default()).size(13.5).color(DIM)));
                }
                cell(ui, 74.0, |ui| ui.label(RichText::new(human_size(pk3.size)).color(DIM)));
                if wide {
                    cell(ui, 112.0, |ui| ui.add(egui::Label::new(RichText::new(&pk3.folder).size(13.5).color(DIM)).truncate()));
                }
                cell(ui, 96.0, |ui| match self.scanner.state(&pk3.key) {
                    Some(ScanState::Queued) => ui.label(RichText::new("scanning…").size(13.5).color(DIM)),
                    Some(ScanState::Done(report)) => {
                        let (text, colour) = report.level().map_or(super::CLEAN, level_badge);
                        ui.label(RichText::new(text).size(13.5).color(colour))
                    }
                    Some(ScanState::Failed(_)) => ui.label(RichText::new("not read").size(13.5).color(BAD)),
                    None => ui.label(""),
                });
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.add_space(6.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let name = RichText::new(&pk3.name).size(15.5);
                        let name = if pk3.on { name.color(theme::TEXT) } else { name.color(DIM).strikethrough() };
                        let response = ui.add(egui::Label::new(name).truncate().sense(egui::Sense::click()));
                        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.pk3_toggle_open(pk3);
                        }
                        if !pk3.on {
                            chip(ui, "off", DIM);
                        }
                        if pk3.owner.protected() {
                            chip(ui, pk3.owner.label(), HOLO);
                        }
                        if let Some(collection) = source {
                            chip(ui, collection.name(), GOLD);
                        }
                        if !index.beats[at].is_empty() {
                            let files: usize = index.beats[at].iter().map(|overlap| overlap.files).sum();
                            chip(ui, &format!("replaces {files}"), GOLD_BRIGHT).on_hover_text("Files other PK3s in its folder also have; the game uses this one's");
                        }
                    });
                    let search = self.pk3.search.as_ref();
                    let inside = search.map(|search| &search.inside[at]).filter(|inside| inside.count > 0);
                    let line = match (inside, &pk3.contents.error) {
                        (Some(inside), _) => {
                            let more = if inside.count > inside.first.len() { format!(" (+{})", inside.count - inside.first.len()) } else { String::new() };
                            RichText::new(format!("{}{more}", inside.first.join("  ·  "))).monospace().size(12.5).color(GOLD_BRIGHT)
                        }
                        (None, Some(error)) => RichText::new(format!("cannot be read: {error}")).size(13.0).color(BAD),
                        (None, None) => RichText::new(pk3.contents.kinds.summary()).size(13.0).color(DIM),
                    };
                    ui.add(egui::Label::new(line).truncate());
                });
            });
        });
        let rect = row.response.rect.expand2(Vec2::new(8.0, 0.0));
        let hovered = ui.rect_contains_pointer(rect);
        if open || hovered {
            let fill = if open { theme::gold(34) } else { theme::holo(10) };
            ui.painter().set(lit, egui::Shape::rect_filled(rect, 4.0, fill));
        }
        let rule = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 1.0), rect.right_bottom());
        ui.painter().rect_filled(rule, 0.0, theme::holo(28));
    }

    fn pk3_toggle_open(&mut self, pk3: &Pk3) {
        if self.pk3.open.as_ref() == Some(&pk3.path) {
            self.pk3.open = None;
            return;
        }
        self.pk3.open = Some(pk3.path.clone());
        // Show what the search found inside, if it found anything there.
        let inside = self.pk3.search.as_ref().is_some_and(|search| {
            search.index.find(&pk3.path).is_some_and(|at| search.inside[at].count > 0)
        });
        self.pk3.file_filter = if inside { self.pk3.query.trim().to_owned() } else { String::new() };
        self.scanner.request(pk3.key, Location::Local(pk3.path.clone()));
    }

    /// Turn PK3s on or off. A file a content collection keeps is let go there too,
    /// or its next download would bring it back.
    fn pk3_set_on(&mut self, index: &Index, paths: &[PathBuf], on: bool) {
        let managed = self.managed();
        let mut failures = Vec::new();
        let mut done = 0;
        for path in paths {
            let Some(at) = index.find(path) else { continue };
            let pk3 = &index.pk3s[at];
            match pk3s::set_on(pk3, on) {
                Ok(now) => {
                    done += 1;
                    if !on {
                        self.let_go(&managed, pk3);
                    }
                    if self.pk3.ticked.remove(path) {
                        self.pk3.ticked.insert(now.clone());
                    }
                    if self.pk3.open.as_ref() == Some(path) {
                        self.pk3.open = Some(now);
                    }
                }
                Err(error) => failures.push(error),
            }
        }
        self.after_pk3_change(done, if on { "turned on" } else { "turned off" }, failures);
    }

    fn pk3_delete(&mut self, index: &Index, paths: &[PathBuf]) {
        let managed = self.managed();
        let mut failures = Vec::new();
        let mut done = 0;
        for path in paths {
            let Some(at) = index.find(path) else { continue };
            let pk3 = &index.pk3s[at];
            match pk3s::delete(pk3) {
                Ok(()) => {
                    done += 1;
                    self.let_go(&managed, pk3);
                    self.pk3.ticked.remove(path);
                    if self.pk3.open.as_ref() == Some(path) {
                        self.pk3.open = None;
                    }
                }
                Err(error) => failures.push(error),
            }
        }
        self.after_pk3_change(done, "deleted", failures);
    }

    /// Unchoose a content collection's file the player removed or turned off.
    fn let_go(&mut self, managed: &HashMap<String, Managed>, pk3: &Pk3) {
        let Some(&Managed { collection, group, asset }) = managed.get(&same_path(&pk3.on_path())) else { return };
        let Some(snapshot) = self.content[collection as usize].progress().snapshot else { return };
        let group = &snapshot.manifest.groups[group];
        let asset = &group.assets[asset];
        let selection = &mut self.selections[collection as usize];
        if selection.wants(group, asset) {
            selection.toggle_file(group, asset);
            self.save_selection(collection);
        }
    }

    fn after_pk3_change(&mut self, done: usize, what: &str, failures: Vec<String>) {
        if failures.is_empty() {
            self.say(format!("{done} PK3{} {what}", if done == 1 { "" } else { "s" }), false);
        } else {
            self.say(format!("{done} {what}; {} failed: {}", failures.len(), failures.join("; ")), true);
        }
        self.pk3_reread();
        for collection in COLLECTIONS {
            self.refresh_content(collection);
        }
    }

    /// Asks before deleting, and offers turning off instead.
    pub(super) fn pk3_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(paths) = self.pk3.confirm.clone() else { return };
        let Some(index) = self.pk3.indexer.index() else { return };
        let pk3s: Vec<&Pk3> = paths.iter().filter_map(|path| index.find(path)).map(|at| &index.pk3s[at]).collect();
        if pk3s.is_empty() {
            self.pk3.confirm = None;
            return;
        }
        let managed = self.managed();
        let kept: Vec<&str> = pk3s
            .iter()
            .filter_map(|pk3| managed.get(&same_path(&pk3.on_path())).map(|managed| managed.collection.name()))
            .collect();
        let size: u64 = pk3s.iter().map(|pk3| pk3.size).sum();
        let mut choice = None;
        let title = if pk3s.len() == 1 { format!("Delete {}?", pk3s[0].name) } else { format!("Delete {} PK3s?", pk3s.len()) };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{} will be gone from your computer. This cannot be undone.", human_size(size)));
                if pk3s.len() > 1 {
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for pk3 in &pk3s {
                            ui.label(RichText::new(format!("{}/{}", pk3.folder, pk3.name)).monospace().size(13.0).color(DIM));
                        }
                    });
                }
                if !kept.is_empty() {
                    ui.label(RichText::new(format!("{} of them came from {}: they are unchosen there so they don't come back.", kept.len(), kept[0])).color(DIM));
                }
                ui.label(RichText::new("Turning them off keeps them, and they come back with one click.").color(DIM));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Delete").strong().color(BAD)).clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Turn off instead").clicked() {
                        choice = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        self.pk3.confirm = None;
                    }
                });
            });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.pk3.confirm = None;
        }
        if let Some(delete) = choice {
            self.pk3.confirm = None;
            if delete {
                self.pk3_delete(&index, &paths);
            } else {
                self.pk3_set_on(&index, &paths, false);
            }
        }
    }

    /// Docked at the right: one PK3, what it holds, who it replaces, the scan,
    /// and its files.
    pub(super) fn pk3_panel(&mut self, ctx: &egui::Context) {
        if self.tab != Tab::Pk3s || self.pk3.confirm.is_some() {
            return;
        }
        let Some(path) = self.pk3.open.clone() else { return };
        let Some(index) = self.pk3.indexer.index() else { return };
        let Some(at) = index.find(&path) else {
            // Renamed or deleted elsewhere; keep it open while the index catches up.
            if !self.pk3.busy() {
                self.pk3.open = None;
            }
            return;
        };
        let pk3 = index.pk3s[at].clone();
        let managed = self.managed().get(&same_path(&pk3.on_path())).copied();
        let mut close = false;
        let mut action = None;
        egui::SidePanel::right("pk3")
            .exact_width(PANEL_WIDTH)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::RAISED).inner_margin(egui::Margin::symmetric(26, 20)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = theme::window_button(ui, WindowButton::Close, 32.0).on_hover_text("Close").clicked();
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(display(&pk3.name, 24.0)).truncate());
                        });
                    });
                });
                let mut meta = format!("{} · {}", pk3.folder, human_size(pk3.size));
                if let Some(modified) = pk3.modified {
                    meta.push_str(&format!(" · {}", day(modified)));
                }
                meta.push_str(&format!(" · {} files", pk3.contents.files.len()));
                ui.label(RichText::new(meta).size(13.0).color(DIM)).on_hover_text(pk3.path.display().to_string());
                let whose = match (pk3.owner, managed) {
                    (Owner::Player, Some(managed)) => format!("From {}", managed.collection.name()),
                    (Owner::Player, None) => if pk3.on { "On: the game loads it".to_owned() } else { "Off: the game skips it".to_owned() },
                    (owner, _) => format!("{}: always on", owner.label()),
                };
                ui.label(RichText::new(whose).size(14.0).color(HOLO));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !pk3.owner.protected() {
                        if ui.button(if pk3.on { "Turn off" } else { "Turn on" }).clicked() {
                            action = Some(Some(!pk3.on));
                        }
                        if ui.button(RichText::new("Delete…").color(BAD)).clicked() {
                            action = Some(None);
                        }
                    }
                    if ui.button("Show in folder").clicked() {
                        pk3s::reveal(&pk3.path);
                    }
                });
                ui.add_space(8.0);
                theme::rule(ui);
                ui.add_space(6.0);

                egui::ScrollArea::vertical().id_salt("pk3-about").max_height(ui.available_height() * 0.5).show(ui, |ui| {
                    self.scan_view(ui, &pk3);
                    ui.add_space(8.0);
                    let kinds = &pk3.contents.kinds;
                    if let Some(error) = &pk3.contents.error {
                        ui.label(RichText::new(format!("Cannot be read: {error}")).color(BAD));
                    }
                    let list = |ui: &mut egui::Ui, title: &str, names: &[String]| {
                        if names.is_empty() {
                            return;
                        }
                        ui.label(strong(format!("{title} ({})", names.len()), 14.0).color(HOLO));
                        ui.add(egui::Label::new(RichText::new(names.join(", ")).size(14.0)).wrap());
                        ui.add_space(4.0);
                    };
                    list(ui, "Maps", &kinds.maps);
                    list(ui, "Player models", &kinds.models);
                    let mut others = Vec::new();
                    for (count, one, many) in [
                        (kinds.sabers, "saber", "sabers"),
                        (kinds.effects, "effect", "effects"),
                        (kinds.sounds, "sound", "sounds"),
                        (kinds.textures, "texture", "textures"),
                        (kinds.shaders, "shader file", "shader files"),
                        (kinds.code, "menu, config or code file", "menus, configs or code"),
                    ] {
                        match count {
                            0 => {}
                            1 => others.push(format!("1 {one}")),
                            _ => others.push(format!("{count} {many}")),
                        }
                    }
                    if !others.is_empty() {
                        ui.label(RichText::new(others.join(" · ")).size(14.0).color(DIM));
                        ui.add_space(4.0);
                    }
                    self.overlaps_view(ui, &index, at);
                });
                ui.add_space(6.0);
                theme::rule(ui);
                ui.add_space(6.0);
                self.pk3_files(ui, &index, at);
            });
        if close || (ctx.input(|input| input.key_pressed(egui::Key::Escape)) && self.pk3.confirm.is_none()) {
            self.pk3.open = None;
        }
        match action {
            Some(Some(on)) => self.pk3_set_on(&index, std::slice::from_ref(&pk3.path), on),
            Some(None) => self.pk3.confirm = Some(vec![pk3.path.clone()]),
            None => {}
        }
    }

    /// Which PK3s of its folder this one replaces files of, and which replace its own.
    fn overlaps_view(&mut self, ui: &mut egui::Ui, index: &Index, at: usize) {
        let mut open = None;
        for (title, list, colour) in [
            ("Replaces files of", &index.beats[at], GOLD_BRIGHT),
            ("Its files replaced by", &index.beaten[at], DIM),
        ] {
            if list.is_empty() {
                continue;
            }
            ui.add_space(4.0);
            ui.label(strong(title, 14.0).color(colour))
                .on_hover_text("Within one folder the game uses the copy in the PK3 that comes last by name");
            for overlap in list.iter().take(6) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if ui.link(RichText::new(&index.pk3s[overlap.other].name).size(14.0)).clicked() {
                        open = Some(overlap.other);
                    }
                    let files = if overlap.files == 1 { "1 file".to_owned() } else { format!("{} files", overlap.files) };
                    ui.label(RichText::new(files).size(13.0).color(DIM));
                });
            }
            if list.len() > 6 {
                ui.label(RichText::new(format!("and {} more", list.len() - 6)).size(13.0).color(DIM));
            }
        }
        if let Some(other) = open {
            let other = index.pk3s[other].clone();
            self.pk3_toggle_open(&other);
        }
    }

    fn scan_view(&mut self, ui: &mut egui::Ui, pk3: &Pk3) {
        match self.scanner.state(&pk3.key) {
            Some(ScanState::Done(report)) => super::findings_view(ui, &report),
            Some(ScanState::Failed(error)) => {
                ui.label(RichText::new(format!("Could not scan it: {error}")).color(BAD));
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Scanning…").color(DIM));
                });
            }
        }
    }

    /// The files inside, filtered; a file another PK3 replaces is struck through.
    fn pk3_files(&mut self, ui: &mut egui::Ui, index: &Index, at: usize) {
        let pk3 = &index.pk3s[at];
        ui.label(RichText::new("Files inside").size(13.0).color(DIM));
        ui.add(egui::TextEdit::singleline(&mut self.pk3.file_filter).hint_text("Filter").desired_width(f32::INFINITY).margin(Vec2::new(10.0, 8.0)));
        ui.add_space(4.0);
        let flagged: HashMap<String, Level> = match self.scanner.state(&pk3.key) {
            Some(ScanState::Done(report)) => report.findings.iter().map(|finding| (finding.path.replace('\\', "/"), finding.level)).collect(),
            _ => HashMap::new(),
        };
        let words: Vec<String> = self.pk3.file_filter.to_ascii_lowercase().split_whitespace().map(str::to_owned).collect();
        let mut rows: Vec<&(String, u64)> = pk3
            .contents
            .files
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                words.iter().all(|word| lower.contains(word))
            })
            .collect();
        rows.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        let height = 22.0;
        egui::ScrollArea::vertical().id_salt("pk3-files").auto_shrink(false).show_rows(ui, height, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (name, size) in &rows[range] {
                let hidden = index.hidden[at].get(&name.to_ascii_lowercase()).copied();
                let colour = flagged.get(name).map_or(if hidden.is_some() { DIM } else { theme::TEXT }, |&level| level_mark(level).1);
                ui.horizontal(|ui| {
                    ui.set_height(height);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(human_size(*size)).size(13.0).color(DIM));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let text = RichText::new(name.as_str()).monospace().size(13.0).color(colour);
                            let text = if hidden.is_some() { text.strikethrough() } else { text };
                            let label = ui.add(egui::Label::new(text).truncate());
                            if let Some(winner) = hidden {
                                label.on_hover_text(format!("The game uses {}'s copy", index.pk3s[winner].name));
                            }
                        });
                    });
                });
            }
        });
    }
}

/// A small coloured word beside a name.
fn chip(ui: &mut egui::Ui, text: &str, colour: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(11.5), colour);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_stroke(rect, 3.0, egui::Stroke::new(1.0, colour.gamma_multiply(0.6)), egui::StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, colour);
    response
}

#[cfg(debug_assertions)]
impl App {
    /// Debug builds: `SJK_LAUNCHER_SEARCH=<text>` searches the tab and
    /// `SJK_LAUNCHER_PK3=<name.pk3>` opens that PK3, once the index is read.
    pub(super) fn pk3_debug(&mut self) {
        if self.tab != Tab::Pk3s {
            return;
        }
        if let Ok(query) = std::env::var("SJK_LAUNCHER_SEARCH")
            && self.pk3.query.is_empty()
        {
            self.pk3.query = query;
        }
        if let (Ok(name), None) = (std::env::var("SJK_LAUNCHER_PK3"), &self.pk3.open)
            && let Some(index) = self.pk3.indexer.index()
            && let Some(pk3) = index.pk3s.iter().find(|pk3| pk3.name.eq_ignore_ascii_case(&name))
        {
            let pk3 = pk3.clone();
            self.pk3_toggle_open(&pk3);
        }
    }
}
