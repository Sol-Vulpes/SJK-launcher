//! Every PK3 in `GameData`, read for the PK3s tab: what each holds (its central
//! directory, never the whole file), which of its files another PK3 replaces, and
//! turning one off or deleting it.
//!
//! - **Off** renames `name.pk3` to `name.pk3.off`: the games load `*.pk3` only, so
//!   the file stays where it was and comes back with one click.
//! - **Who wins:** inside one folder the game reads the PK3s in name order and the
//!   last one's copy of a file is used (OpenJK sorts them and searches the last
//!   first), so `zz_skin.pk3` replaces the same file in `assets1.pk3`. A mod folder
//!   such as `japlus` goes over `base` only while that mod runs, so folders are
//!   compared each on its own.
//! - The game's own files (Jedi Academy's `assets0-3.pk3`, SJK's folder and
//!   EternalJK's) are listed and searchable but can be neither turned off nor
//!   deleted from here.

use crate::pk3scan::{self, LocalFile};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The suffix that turns a PK3 off.
pub const OFF: &str = ".off";
/// How many folders deep under `GameData` PK3s are looked for (`SJK/jof/base` is three).
const DEPTH: usize = 3;
/// Folders under `GameData` that hold no game files.
const SKIPPED: &[&str] = &["SJK/launcher-downloads"];

/// Who a PK3 belongs to, when it is not the player's to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Jedi Academy's own `assets0.pk3` to `assets3.pk3`.
    JediAcademy,
    /// SJK's own folder (its assets, what it generates); not its JoF content.
    Sjk,
    /// EternalJK's own folder.
    EternalJk,
    /// Anything else: the player's to keep or remove.
    Player,
}

impl Owner {
    pub fn protected(self) -> bool {
        self != Self::Player
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::JediAcademy => "Jedi Academy",
            Self::Sjk => "SJK's own",
            Self::EternalJk => "EternalJK's own",
            Self::Player => "",
        }
    }
}

/// What kinds of things a PK3 carries, from its file names.
#[derive(Clone, Debug, Default)]
pub struct Kinds {
    /// Map names (`maps/<name>.bsp`).
    pub maps: Vec<String>,
    /// Player model folders (`models/players/<name>/`).
    pub models: Vec<String>,
    pub sabers: usize,
    pub sounds: usize,
    pub textures: usize,
    pub shaders: usize,
    pub effects: usize,
    /// Menus, configs and game code (`.menu`, `.cfg`, `.qvm`, native libraries).
    pub code: usize,
}

impl Kinds {
    fn of(files: &[(String, u64)]) -> Self {
        let mut kinds = Self::default();
        for (name, _) in files {
            let lower = name.to_ascii_lowercase();
            let extension = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or_default();
            if let Some(map) = lower.strip_prefix("maps/").and_then(|rest| rest.strip_suffix(".bsp")) {
                if !map.contains('/') {
                    kinds.maps.push(name[5..name.len() - 4].to_owned());
                }
            } else if let Some(rest) = lower.strip_prefix("models/players/") {
                if let Some((model, _)) = rest.split_once('/')
                    && !kinds.models.iter().any(|known| known.eq_ignore_ascii_case(model))
                {
                    kinds.models.push(name[15..15 + model.len()].to_owned());
                }
            } else if extension == "sab" {
                kinds.sabers += 1;
            } else if lower.starts_with("sound/") || lower.starts_with("music/") {
                kinds.sounds += 1;
            } else if extension == "shader" {
                kinds.shaders += 1;
            } else if extension == "efx" {
                kinds.effects += 1;
            } else if lower.starts_with("textures/") || lower.starts_with("gfx/") {
                kinds.textures += 1;
            } else if matches!(extension, "menu" | "cfg" | "qvm" | "dll" | "so" | "dylib") {
                kinds.code += 1;
            }
        }
        kinds.maps.sort_by_key(|map| map.to_ascii_lowercase());
        kinds.models.sort_by_key(|model| model.to_ascii_lowercase());
        kinds
    }

    /// "2 maps · 3 player models · sounds", the most telling first.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        if !self.maps.is_empty() {
            parts.push(if self.maps.len() <= 2 { self.maps.join(", ") } else { count(self.maps.len(), "map", "maps") });
        }
        if !self.models.is_empty() {
            parts.push(count(self.models.len(), "player model", "player models"));
        }
        if self.sabers > 0 {
            parts.push(count(self.sabers, "saber", "sabers"));
        }
        if self.effects > 0 {
            parts.push("effects".into());
        }
        if self.sounds > 0 {
            parts.push("sounds".into());
        }
        if self.textures > 0 || self.shaders > 0 {
            parts.push("textures".into());
        }
        if self.code > 0 {
            parts.push("menus or code".into());
        }
        parts.join(" · ")
    }
}

/// A PK3's files, read once per file version and kept for the run.
#[derive(Debug, Default)]
pub struct Contents {
    /// Files only (folders are entries of their own in a ZIP), with their sizes.
    pub files: Vec<(String, u64)>,
    pub kinds: Kinds,
    /// Why the archive could not be read.
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Pk3 {
    /// Where it is now (ending `.pk3.off` while off).
    pub path: PathBuf,
    /// Its folder under `GameData`, with `/` between parts: `base`, `SJK/jof/base`.
    pub folder: String,
    /// The PK3's name, without `.off`.
    pub name: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub on: bool,
    pub owner: Owner,
    /// Stands for this version of the file (path, size, time), for scans and the cache.
    pub key: [u8; 16],
    pub contents: Arc<Contents>,
}

impl Pk3 {
    /// The path it has while on.
    pub fn on_path(&self) -> PathBuf {
        self.path.with_file_name(&self.name)
    }

    pub fn off_path(&self) -> PathBuf {
        self.path.with_file_name(format!("{}{OFF}", self.name))
    }
}

/// Files two PK3s of one folder both have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlap {
    /// The other PK3, as an index into [`Index::pk3s`].
    pub other: usize,
    pub files: usize,
}

#[derive(Debug, Default)]
pub struct Index {
    /// Folder by folder (`base` first), then by name as the game orders them.
    pub pk3s: Vec<Pk3>,
    /// Per PK3: whose files its own replace, and who replaces its own.
    pub beats: Vec<Vec<Overlap>>,
    pub beaten: Vec<Vec<Overlap>>,
    /// Per PK3: its files another PK3 replaces (lower case) and by whom.
    pub hidden: Vec<HashMap<String, usize>>,
}

impl Index {
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.pk3s.iter().position(|pk3| pk3.path == path)
    }
}

/// The game reads a folder's PK3s in this order, the last winning (`FS_PathCmp`).
fn game_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}

fn owner(folder: &str, name: &str) -> Owner {
    let folder_lower = folder.to_ascii_lowercase();
    let name_lower = name.to_ascii_lowercase();
    if folder_lower == "base" && matches!(name_lower.as_str(), "assets0.pk3" | "assets1.pk3" | "assets2.pk3" | "assets3.pk3") {
        Owner::JediAcademy
    } else if folder_lower == "sjk" || (folder_lower.starts_with("sjk/") && !folder_lower.starts_with("sjk/jof")) {
        Owner::Sjk
    } else if folder_lower == "eternaljk" || folder_lower.starts_with("eternaljk/") {
        Owner::EternalJk
    } else {
        Owner::Player
    }
}

fn key(path: &Path, size: u64, modified: Option<SystemTime>) -> [u8; 16] {
    let nanos = modified.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |since| since.as_nanos());
    let digest = Sha256::digest(format!("{}|{size}|{nanos}", path.display()).as_bytes());
    let mut key = [0; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// The games load PK3s straight inside a mod folder (`base`, `japlus`); SJK also has
/// its own below `SJK` (`SJK/assets`, its JoF content in `SJK/jof/base`).
fn loaded_from(folder: &str) -> bool {
    !folder.is_empty() && (!folder.contains('/') || folder.to_ascii_lowercase().starts_with("sjk/"))
}

/// The PK3 files (on or off) under `game_data`, with their folders.
fn find(game_data: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut folders = vec![(game_data.to_owned(), String::new(), 0)];
    while let Some((folder, relative, depth)) = folders.pop() {
        let Ok(read) = std::fs::read_dir(&folder) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                let inner = if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") };
                if depth < DEPTH && !name.starts_with('.') && !SKIPPED.iter().any(|skip| skip.eq_ignore_ascii_case(&inner)) {
                    folders.push((path, inner, depth + 1));
                }
            } else if loaded_from(&relative) {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".pk3") || lower.ends_with(".pk3.off") {
                    found.push((path, relative.clone()));
                }
            }
        }
    }
    found
}

fn read_contents(path: &Path) -> Contents {
    let entries = LocalFile::open(path).and_then(|mut file| pk3scan::entries(&mut file));
    match entries {
        Ok(entries) => {
            let files: Vec<(String, u64)> = entries
                .into_iter()
                .filter(|entry| !entry.name.ends_with('/'))
                .map(|entry| (entry.name.replace('\\', "/"), entry.size))
                .collect();
            Contents { kinds: Kinds::of(&files), files, error: None }
        }
        Err(error) => Contents { error: Some(error), ..Contents::default() },
    }
}

/// Read every PK3 under `game_data`, the unchanged ones from `cache`, and work out
/// who replaces whom. `progress` hears (done, total) as files are read.
pub fn index(game_data: &Path, cache: &Mutex<HashMap<[u8; 16], Arc<Contents>>>, progress: &dyn Fn(usize, usize)) -> Index {
    let found = find(game_data);
    let total = found.len();
    let mut pk3s = Vec::with_capacity(total);
    for (done, (path, folder)) in found.into_iter().enumerate() {
        progress(done, total);
        let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let on = !file_name.to_ascii_lowercase().ends_with(OFF);
        let name = if on { file_name } else { file_name[..file_name.len() - OFF.len()].to_owned() };
        let metadata = std::fs::metadata(&path).ok();
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified = metadata.and_then(|metadata| metadata.modified().ok());
        let key = key(&path, size, modified);
        let cached = cache.lock().unwrap_or_else(|error| error.into_inner()).get(&key).cloned();
        let contents = cached.unwrap_or_else(|| {
            let contents = Arc::new(read_contents(&path));
            cache.lock().unwrap_or_else(|error| error.into_inner()).insert(key, Arc::clone(&contents));
            contents
        });
        let owner = owner(&folder, &name);
        pk3s.push(Pk3 { path, folder, name, size, modified, on, owner, key, contents });
    }
    progress(total, total);
    pk3s.sort_by(|a, b| {
        let base = |pk3: &Pk3| !pk3.folder.eq_ignore_ascii_case("base");
        base(a).cmp(&base(b)).then_with(|| game_order(&a.folder, &b.folder)).then_with(|| game_order(&a.name, &b.name))
    });
    let mut index = Index { pk3s, ..Index::default() };
    overlaps(&mut index);
    index
}

/// Who replaces whom, among the PK3s that are on, folder by folder.
fn overlaps(index: &mut Index) {
    let count = index.pk3s.len();
    index.beats = vec![Vec::new(); count];
    index.beaten = vec![Vec::new(); count];
    index.hidden = vec![HashMap::new(); count];
    // Each file's PK3s in game order (the index is sorted so), per folder.
    let mut holders: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (at, pk3) in index.pk3s.iter().enumerate().filter(|(_, pk3)| pk3.on) {
        let folder = pk3.folder.to_ascii_lowercase();
        for (name, _) in &pk3.contents.files {
            holders.entry((folder.clone(), name.to_ascii_lowercase())).or_default().push(at);
        }
    }
    let mut pairs: HashMap<(usize, usize), usize> = HashMap::new();
    for ((_, name), holders) in holders {
        let Some((&winner, losers)) = holders.split_last() else { continue };
        for &loser in losers {
            *pairs.entry((winner, loser)).or_default() += 1;
            index.hidden[loser].insert(name.clone(), winner);
        }
    }
    for ((winner, loser), files) in pairs {
        index.beats[winner].push(Overlap { other: loser, files });
        index.beaten[loser].push(Overlap { other: winner, files });
    }
    for list in index.beats.iter_mut().chain(index.beaten.iter_mut()) {
        list.sort_by(|a, b| b.files.cmp(&a.files).then(a.other.cmp(&b.other)));
    }
}

/// What the file system said, in the player's words.
fn explain(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        Some(32 | 33) if cfg!(windows) => "in use: close the game first".into(),
        _ if error.kind() == std::io::ErrorKind::PermissionDenied => "not allowed: the folder may need administrator rights".into(),
        _ => error.to_string(),
    }
}

/// Turn a PK3 on or off by renaming it; returns where it is now.
pub fn set_on(pk3: &Pk3, on: bool) -> Result<PathBuf, String> {
    if pk3.owner.protected() {
        return Err(format!("{} is {} and stays on", pk3.name, pk3.owner.label()));
    }
    if pk3.on == on {
        return Ok(pk3.path.clone());
    }
    let target = if on { pk3.on_path() } else { pk3.off_path() };
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    std::fs::rename(&pk3.path, &target).map_err(|error| format!("{}: {}", pk3.name, explain(&error)))?;
    Ok(target)
}

/// Delete a PK3 for good.
pub fn delete(pk3: &Pk3) -> Result<(), String> {
    if pk3.owner.protected() {
        return Err(format!("{} is {} and cannot be deleted here", pk3.name, pk3.owner.label()));
    }
    std::fs::remove_file(&pk3.path).map_err(|error| format!("{}: {}", pk3.name, explain(&error)))
}

/// Show the file in the system's file manager.
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(folder) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
    }
}

/// The index, rebuilt on a thread of its own whenever asked.
#[derive(Clone, Default)]
pub struct Indexer {
    state: Arc<Mutex<State>>,
    cache: Arc<Mutex<HashMap<[u8; 16], Arc<Contents>>>>,
}

#[derive(Default)]
struct State {
    index: Option<Arc<Index>>,
    /// (done, total) while reading.
    reading: Option<(usize, usize)>,
    /// A refresh asked for while one runs: it runs again after.
    again: bool,
}

impl Indexer {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn index(&self) -> Option<Arc<Index>> {
        self.lock().index.clone()
    }

    pub fn reading(&self) -> Option<(usize, usize)> {
        self.lock().reading
    }

    pub fn refresh(&self, game_data: PathBuf, repaint: impl Fn() + Send + Sync + 'static) {
        {
            let mut state = self.lock();
            if state.reading.is_some() {
                state.again = true;
                return;
            }
            state.reading = Some((0, 0));
        }
        let this = self.clone();
        std::thread::spawn(move || {
            loop {
                let index = index(&game_data, &this.cache, &|done, total| {
                    this.lock().reading = Some((done, total));
                    if done % 16 == 0 {
                        repaint();
                    }
                });
                let mut state = this.lock();
                state.index = Some(Arc::new(index));
                if !std::mem::take(&mut state.again) {
                    state.reading = None;
                    break;
                }
            }
            repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn pk3(path: &Path, files: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for name in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();
    }

    fn game_data(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sjk-launcher-pk3s-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        pk3(&root.join("base/assets1.pk3"), &["models/players/kyle/model.glm", "maps/ffa_bespin.bsp"]);
        pk3(&root.join("base/zz_kyle.pk3"), &["models/players/kyle/model.glm", "models/players/kyle/skin.skin"]);
        pk3(&root.join("base/arena.pk3"), &["maps/Arena.bsp", "levelshots/arena.jpg"]);
        pk3(&root.join("japlus/japlus_assets.pk3"), &["models/players/kyle/model.glm"]);
        pk3(&root.join("SJK/assets/sjk.pk3"), &["ui/sjk.menu"]);
        pk3(&root.join("SJK/jof/base/jof_map.pk3"), &["maps/jof.bsp"]);
        pk3(&root.join("SJK/launcher-downloads/half.pk3"), &["x"]);
        pk3(&root.join("base/backup/old.pk3"), &["x"]);
        root
    }

    fn by_name<'a>(index: &'a Index, name: &str) -> (usize, &'a Pk3) {
        index.pk3s.iter().enumerate().find(|(_, pk3)| pk3.name == name).unwrap()
    }

    #[test]
    fn finds_every_folder_and_knows_the_owners() {
        let root = game_data("find");
        let index = index(&root, &Mutex::default(), &|_, _| {});
        let names: Vec<_> = index.pk3s.iter().map(|pk3| format!("{}/{}", pk3.folder, pk3.name)).collect();
        assert_eq!(names, ["base/arena.pk3", "base/assets1.pk3", "base/zz_kyle.pk3", "japlus/japlus_assets.pk3", "SJK/assets/sjk.pk3", "SJK/jof/base/jof_map.pk3"]);
        assert_eq!(by_name(&index, "jof_map.pk3").1.owner, Owner::Player);
        assert_eq!(by_name(&index, "assets1.pk3").1.owner, Owner::JediAcademy);
        assert_eq!(by_name(&index, "sjk.pk3").1.owner, Owner::Sjk);
        assert_eq!(by_name(&index, "arena.pk3").1.owner, Owner::Player);
        assert_eq!(by_name(&index, "arena.pk3").1.contents.kinds.maps, ["Arena"]);
        assert_eq!(by_name(&index, "zz_kyle.pk3").1.contents.kinds.models, ["kyle"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_last_in_name_order_wins_within_a_folder() {
        let root = game_data("wins");
        let index = index(&root, &Mutex::default(), &|_, _| {});
        let (assets, _) = by_name(&index, "assets1.pk3");
        let (kyle, _) = by_name(&index, "zz_kyle.pk3");
        assert_eq!(index.beats[kyle], [Overlap { other: assets, files: 1 }]);
        assert_eq!(index.hidden[assets].get("models/players/kyle/model.glm"), Some(&kyle));
        // japlus has the same file, but another folder never counts.
        let (japlus, _) = by_name(&index, "japlus_assets.pk3");
        assert!(index.beaten[japlus].is_empty() && index.beats[japlus].is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn off_renames_and_on_brings_it_back() {
        let root = game_data("off");
        let cache = Mutex::default();
        let index = index(&root, &cache, &|_, _| {});
        let arena = by_name(&index, "arena.pk3").1.clone();
        let off = set_on(&arena, false).unwrap();
        assert!(off.ends_with("arena.pk3.off") && off.is_file());
        let again = super::index(&root, &cache, &|_, _| {});
        let arena = by_name(&again, "arena.pk3").1.clone();
        assert!(!arena.on);
        assert!(set_on(&arena, true).unwrap().ends_with("arena.pk3"));
        // The game's own files stay.
        let assets = by_name(&again, "assets1.pk3").1.clone();
        assert!(set_on(&assets, false).is_err() && delete(&assets).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_off_pk3_hides_nothing() {
        let root = game_data("offwins");
        std::fs::rename(root.join("base/zz_kyle.pk3"), root.join("base/zz_kyle.pk3.off")).unwrap();
        let index = index(&root, &Mutex::default(), &|_, _| {});
        let (assets, _) = by_name(&index, "assets1.pk3");
        assert!(index.hidden[assets].is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
