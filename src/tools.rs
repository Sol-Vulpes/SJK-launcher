//! The Tools tab's work (`app::tools_tab`): where SJK keeps its things, opening them,
//! a backup of the small files that make a player's setup, and Steam's file check.

use std::path::{Path, PathBuf};

/// A folder the Tools tab opens.
pub struct Place {
    pub label: &'static str,
    pub hint: &'static str,
    pub path: PathBuf,
}

/// The folders worth opening, in the order shown; `game_data` is Jedi Academy's.
pub fn folders(game_data: Option<&Path>) -> Vec<Place> {
    let mut places = Vec::new();
    if let Some(game_data) = game_data {
        let sjk = game_data.join("SJK");
        let place = |label, hint, path: PathBuf| Place { label, hint, path };
        places.extend([
            place("GameData", "Jedi Academy's own folder", game_data.to_owned()),
            place("base", "The game's pk3s (maps, models, mods)", game_data.join("base")),
            place("SJK", "SJK's settings, identity and everything below", sjk.clone()),
            place("Screenshots", "F12 and screenshot pictures", sjk.join("screenshots")),
            place("Demos", "Recorded demos", sjk.join("demos")),
            place("Ghosts", "Ghost recordings", sjk.join("ghosts")),
            place("Chat logs", "Saved chat (cl_logChat)", sjk.join("chatlogs")),
            place("SJK's logs", "session.log and the one before", sjk.join("logs")),
            place("Crash reports", "Kept crash reports and last-crash.txt", sjk.join("crashes")),
        ]);
    }
    if let Some(folder) = crate::config::folder() {
        places.push(Place { label: "Launcher", hint: "The launcher's settings and the games' output logs", path: folder });
    }
    places
}

/// A file the Tools tab opens in a text editor.
pub fn files(game_data: Option<&Path>) -> Vec<Place> {
    let mut files = Vec::new();
    if let Some(game_data) = game_data {
        let sjk = game_data.join("SJK");
        let file = |label, hint, path: PathBuf| Place { label, hint, path };
        files.extend([
            file("config.cfg", "SJK's settings (close the game before editing)", sjk.join("config.cfg")),
            file("chat.log", "Saved chat", sjk.join("chatlogs").join("chat.log")),
            file("session.log", "SJK's log of its last run", sjk.join("logs").join("session.log")),
            file("last-crash.txt", "The last crash report", sjk.join("crashes").join("last-crash.txt")),
        ]);
    }
    if let Some(path) = crate::game_log::path(crate::games::Game::Sjk) {
        files.push(Place { label: "sjk.log", hint: "What Sol JK printed when the launcher last started it", path });
    }
    files
}

/// Open `path` in a plain text editor (a `.cfg` has no program of its own on most PCs).
pub fn edit(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("notepad").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-t").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Jedi Academy's Steam app.
const STEAM_APP: u32 = 6020;

/// Whether `game_data` is a Steam install, which Steam can check and repair.
pub fn is_steam(game_data: &Path) -> bool {
    game_data.components().any(|part| part.as_os_str().eq_ignore_ascii_case("steamapps"))
}

/// Ask Steam to check Jedi Academy's files and fetch the broken or missing ones.
pub fn steam_verify() {
    crate::system::open(Path::new(&format!("steam://validate/{STEAM_APP}")));
}

/// Files bigger than this are not a setting: left out of a backup.
const BACKUP_FILE_MAX: u64 = 8 << 20;

/// The default name of a backup made today.
pub fn backup_name() -> String {
    let (year, month, day) = civil(crate::doctor::unix_now() / 86_400);
    format!("SJK backup {day:02}-{month:02}-{year}.zip")
}

/// Zip SJK's own small files (identity, settings, lists, notes; the files at the top of
/// `SJK` and `map_edits`) into `to`; the big folders (demos, screenshots, assets) stay
/// out. The number of files kept.
pub fn backup(game_data: &Path, to: &Path) -> Result<usize, String> {
    use std::io::Write;
    let sjk = game_data.join("SJK");
    let mut picked = Vec::new();
    for folder in [sjk.clone(), sjk.join("map_edits")] {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let log = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
            if meta.is_file() && meta.len() <= BACKUP_FILE_MAX && !log {
                picked.push(path);
            }
        }
    }
    if !picked.iter().any(|path| path.ends_with("identity.key")) {
        return Err("SJK has no identity here yet: nothing worth a backup".to_owned());
    }
    let file = std::fs::File::create(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for path in &picked {
        let name = path.strip_prefix(game_data).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(path).map_err(|error| format!("{name}: {error}"))?;
        zip.start_file(name.clone(), options).map_err(|error| format!("{name}: {error}"))?;
        zip.write_all(&bytes).map_err(|error| format!("{name}: {error}"))?;
    }
    zip.finish().map_err(|error| error.to_string())?;
    Ok(picked.len())
}

/// The date of day `days` since 1970 (Howard Hinnant's civil_from_days).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_400), (2025, 11, 8));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn steam_installs_are_known() {
        assert!(is_steam(Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Jedi Academy\GameData")));
        assert!(!is_steam(Path::new(r"E:\JKA SOL\GameData")));
    }

    #[test]
    fn a_backup_keeps_the_small_files_and_leaves_logs_and_folders_out() {
        let dir = std::env::temp_dir().join(format!("sjk-launcher-backup-{}", std::process::id()));
        let sjk = dir.join("SJK");
        std::fs::create_dir_all(sjk.join("demos")).unwrap();
        std::fs::create_dir_all(sjk.join("map_edits")).unwrap();
        std::fs::write(sjk.join("identity.key"), "key").unwrap();
        std::fs::write(sjk.join("config.cfg"), "seta a 1").unwrap();
        std::fs::write(sjk.join("last-server.log"), "log").unwrap();
        std::fs::write(sjk.join("demos").join("a.dm_26"), "demo").unwrap();
        std::fs::write(sjk.join("map_edits").join("mp_ffa5.json"), "{}").unwrap();
        let to = dir.join("backup.zip");
        assert_eq!(backup(&dir, &to), Ok(3));
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&to).unwrap()).unwrap();
        let mut names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_owned()).collect();
        names.sort();
        assert_eq!(names, ["SJK/config.cfg", "SJK/identity.key", "SJK/map_edits/mp_ffa5.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
