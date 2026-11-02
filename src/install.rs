//! Installing a release: download its archive beside `GameData`, check it
//! against GitHub's SHA-256, then extract it into `GameData`.
//!
//! Both games ship drop-in archives meant to be extracted into `GameData`. JoF's
//! Windows ZIP wraps them in one folder (`JoF-beta-windows-x86_64/`); when every
//! entry sits in one folder and the game's program is inside it, that folder is
//! stripped. An entry that would land outside `GameData` (`..`, an absolute
//! path) is refused.

use crate::games::Game;
use crate::github::{Asset, Release};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Downloading {
        done: u64,
        total: u64,
    },
    Checking,
    Extracting {
        done: usize,
        total: usize,
    },
    Done(String),
    Failed(String),
}

impl Phase {
    pub fn running(&self) -> bool {
        matches!(self, Self::Downloading { .. } | Self::Checking | Self::Extracting { .. })
    }
}

/// One install on its own thread; the UI reads [`Install::phase`].
#[derive(Clone, Default)]
pub struct Install {
    phase: Arc<Mutex<Phase>>,
    cancel: Arc<AtomicBool>,
}

impl Install {
    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    pub fn running(&self) -> bool {
        self.phase().running()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        if !self.running() {
            self.set(Phase::Idle);
        }
    }

    fn set(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(|error| error.into_inner()) = phase;
    }

    /// Start installing; `done` gets the tag when it succeeds, on the worker thread.
    pub fn start(
        &self,
        game: Game,
        release: Release,
        game_data: PathBuf,
        repaint: impl Fn() + Send + 'static,
        done: impl FnOnce(Game, String) + Send + 'static,
    ) {
        if self.running() {
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.set(Phase::Downloading { done: 0, total: 0 });
        let this = self.clone();
        std::thread::spawn(move || {
            let result = this.work(game, &release, &game_data, &repaint);
            match result {
                Ok(()) => {
                    this.set(Phase::Done(format!(
                        "{} {} installed",
                        game.name(),
                        game.version_of(&release.tag_name)
                    )));
                    done(game, release.tag_name.clone());
                }
                Err(reason) => this.set(Phase::Failed(reason)),
            }
            repaint();
        });
    }

    fn work(&self, game: Game, release: &Release, game_data: &Path, repaint: &dyn Fn()) -> Result<(), String> {
        let asset = release
            .asset_for(game)
            .ok_or_else(|| format!("{} has no build for this computer", release.tag_name))?;
        let downloads = game_data.join("SJK").join("launcher-downloads");
        std::fs::create_dir_all(&downloads).map_err(|error| format!("cannot make {}: {error}", downloads.display()))?;
        let archive = downloads.join(&asset.name);
        self.download(asset, &archive, repaint)?;
        self.set(Phase::Checking);
        repaint();
        verify(asset, &archive)?;
        extract(game, &archive, game_data, &|done, total| {
            self.set(Phase::Extracting { done, total });
            repaint();
        })?;
        let _ = std::fs::remove_file(&archive);
        Ok(())
    }

    fn download(&self, asset: &Asset, to: &Path, repaint: &dyn Fn()) -> Result<(), String> {
        if to.metadata().is_ok_and(|meta| meta.len() == asset.size) && verify(asset, to).is_ok() {
            return Ok(());
        }
        let part = to.with_extension("part");
        let response = crate::github::agent()
            .get(&asset.browser_download_url)
            .call()
            .map_err(|error| format!("download: {error}"))?;
        let mut reader = response.into_body().into_reader();
        let mut file = File::create(&part).map_err(|error| format!("cannot write {}: {error}", part.display()))?;
        let mut buffer = vec![0_u8; 256 * 1024];
        let mut done = 0_u64;
        let mut shown = 0_u64;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err("stopped".into());
            }
            let read = reader.read(&mut buffer).map_err(|error| format!("download: {error}"))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(|error| format!("write: {error}"))?;
            done += read as u64;
            if done - shown > 512 * 1024 || done == asset.size {
                shown = done;
                self.set(Phase::Downloading { done, total: asset.size });
                repaint();
            }
        }
        file.flush().map_err(|error| format!("write: {error}"))?;
        drop(file);
        if done != asset.size {
            let _ = std::fs::remove_file(&part);
            return Err(format!("download ended at {done} of {} bytes", asset.size));
        }
        std::fs::rename(&part, to).map_err(|error| format!("cannot keep the download: {error}"))
    }
}

/// The archive's SHA-256 against GitHub's digest; an asset without one is checked by size only.
pub(crate) fn verify(asset: &Asset, path: &Path) -> Result<(), String> {
    let Some(expected) = asset.digest.as_deref().and_then(|digest| digest.strip_prefix("sha256:")) else {
        return Ok(());
    };
    let mut file = File::open(path).map_err(|error| format!("cannot read the download: {error}"))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| format!("cannot read the download: {error}"))?;
    let actual: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        let _ = std::fs::remove_file(path);
        Err(format!("{} does not match GitHub's checksum; deleted, try again", asset.name))
    }
}

/// A relative path made only of plain names, or `None`.
fn safe(path: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

/// The folders around the game's program (`JoF-master-windows-x86_64/GameData`),
/// when every entry is in them or is one of them; `None` when the program is at
/// the archive's root.
fn wrapper(game: Game, entries: &[PathBuf]) -> Option<PathBuf> {
    let program = std::ffi::OsStr::new(game.executable()?);
    let prefix: PathBuf = entries.iter().find_map(|entry| {
        let parts: Vec<_> = entry.components().collect();
        let at = parts.iter().position(|part| part.as_os_str() == program)?;
        Some(parts[..at].iter().collect())
    })?;
    if prefix.as_os_str().is_empty() {
        return None;
    }
    let inside = |entry: &PathBuf| entry.starts_with(&prefix) || prefix.starts_with(entry);
    entries.iter().all(inside).then_some(prefix)
}

fn extract(game: Game, archive: &Path, game_data: &Path, report: &dyn Fn(usize, usize)) -> Result<(), String> {
    let name = archive.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if name.ends_with(".zip") {
        extract_zip(game, archive, game_data, report)
    } else if name.ends_with(".tar.gz") {
        extract_tar(game, archive, game_data, report)
    } else {
        Err(format!("cannot open {name}"))
    }
}

fn destination(game_data: &Path, entry: &Path, strip: Option<&Path>) -> Option<PathBuf> {
    let relative = match strip {
        Some(top) => entry.strip_prefix(top).ok()?,
        None => entry,
    };
    safe(relative).map(|relative| game_data.join(relative))
}

fn extract_zip(game: Game, archive: &Path, game_data: &Path, report: &dyn Fn(usize, usize)) -> Result<(), String> {
    let open = |error: zip::result::ZipError| format!("{}: {error}", archive.display());
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|error| error.to_string())?).map_err(open)?;
    let names: Vec<PathBuf> = (0..zip.len())
        .filter_map(|index| zip.by_index(index).ok().and_then(|entry| entry.enclosed_name()))
        .collect();
    let strip = wrapper(game, &names);
    let total = zip.len();
    for index in 0..total {
        let mut entry = zip.by_index(index).map_err(open)?;
        let Some(to) = entry
            .enclosed_name()
            .and_then(|name| destination(game_data, &name, strip.as_deref()))
        else {
            continue;
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&to).map_err(|error| format!("{}: {error}", to.display()))?;
        } else {
            write_file(&to, &mut entry)?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode & 0o777));
            }
        }
        if index % 16 == 0 || index + 1 == total {
            report(index + 1, total);
        }
    }
    Ok(())
}

fn extract_tar(game: Game, archive: &Path, game_data: &Path, report: &dyn Fn(usize, usize)) -> Result<(), String> {
    let open = || -> Result<tar::Archive<flate2::read::GzDecoder<File>>, String> {
        let file = File::open(archive).map_err(|error| error.to_string())?;
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(file)))
    };
    let problem = |error: std::io::Error| format!("{}: {error}", archive.display());
    let names: Vec<PathBuf> = open()?
        .entries()
        .map_err(problem)?
        .filter_map(|entry| entry.ok().and_then(|entry| entry.path().ok().map(|path| path.into_owned())))
        .collect();
    let strip = wrapper(game, &names);
    let total = names.len();
    for (index, entry) in open()?.entries().map_err(problem)?.enumerate() {
        let mut entry = entry.map_err(problem)?;
        let path = entry.path().map_err(problem)?.into_owned();
        let Some(to) = destination(game_data, &path, strip.as_deref()) else { continue };
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        entry.unpack(&to).map_err(|error| format!("{}: {error}", to.display()))?;
        if index % 16 == 0 || index + 1 == total {
            report(index + 1, total);
        }
    }
    Ok(())
}

/// Written beside the target then renamed over it, so a running game's file is
/// never left half-written; a locked file (the game is open) fails cleanly.
fn write_file(to: &Path, from: &mut impl Read) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let mut temporary = to.as_os_str().to_owned();
    temporary.push(".sjk-launcher-new");
    let temporary = PathBuf::from(temporary);
    let mut file = File::create(&temporary).map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::io::copy(from, &mut file).map_err(|error| format!("{}: {error}", to.display()))?;
    drop(file);
    std::fs::rename(&temporary, to).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("cannot replace {} (is the game running?): {error}", to.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_entries_are_refused() {
        assert_eq!(safe(Path::new("base/a.pk3")), Some(PathBuf::from("base/a.pk3")));
        assert_eq!(safe(Path::new("../evil")), None);
        assert_eq!(safe(Path::new("/etc/passwd")), None);
    }

    #[test]
    fn a_wrapping_folder_is_stripped_only_around_the_program() {
        let program = Game::Jof.executable().unwrap();
        let wrapped = vec![
            PathBuf::from("JoF-beta-windows-x86_64"),
            PathBuf::from(format!("JoF-beta-windows-x86_64/{program}")),
            PathBuf::from("JoF-beta-windows-x86_64/base/JoF_Animations.pk3"),
        ];
        assert_eq!(wrapper(Game::Jof, &wrapped), Some(PathBuf::from("JoF-beta-windows-x86_64")));
        let flat = vec![PathBuf::from(program), PathBuf::from("base/JoF_Animations.pk3")];
        assert_eq!(wrapper(Game::Jof, &flat), None);
        let only_base = vec![PathBuf::from("base/a.pk3"), PathBuf::from("base/b.pk3")];
        assert_eq!(wrapper(Game::Jof, &only_base), None);
        // JoF's Windows ZIP, as published for 1.6.2.
        let deep = vec![
            PathBuf::from("JoF-master-windows-x86_64"),
            PathBuf::from("JoF-master-windows-x86_64/GameData"),
            PathBuf::from(format!("JoF-master-windows-x86_64/GameData/{program}")),
            PathBuf::from("JoF-master-windows-x86_64/GameData/base/JoF_Animations.pk3"),
        ];
        assert_eq!(wrapper(Game::Jof, &deep), Some(PathBuf::from("JoF-master-windows-x86_64/GameData")));
        let stray = [deep.clone(), vec![PathBuf::from("readme.txt")]].concat();
        assert_eq!(wrapper(Game::Jof, &stray), None);
    }

    /// `cargo test -- --ignored`: both games' newest releases download, pass
    /// their checksum and extract with the program at GameData's root.
    #[test]
    #[ignore = "downloads both games"]
    fn both_games_install_into_an_empty_folder() {
        let root = std::env::temp_dir().join("sjk-launcher-install-test");
        let _ = std::fs::remove_dir_all(&root);
        for game in crate::games::GAMES {
            let release = crate::github::releases(&crate::github::agent(), game, false).unwrap().remove(0);
            let install = Install::default();
            install.start(game, release, root.clone(), || {}, |_, _| {});
            while install.running() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(matches!(install.phase(), Phase::Done(_)), "{:?}", install.phase());
            assert!(game.is_present(&root), "{} missing", game.name());
        }
        let top: Vec<_> = std::fs::read_dir(&root).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        println!("{top:?}");
    }
}