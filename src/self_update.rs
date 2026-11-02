//! The launcher updating itself from its own GitHub releases (`v*` tags on
//! `Sol-Vulpes/sjk-launcher`), each carrying a `<program>-<os>-<arch>` archive
//! with the program at its root (`build.ps1`, `build.sh`).
//!
//! The new program is checked against GitHub's SHA-256, written beside the running
//! one, then swapped in: the running file is renamed to `<name>.old` (Windows lets a
//! running program be renamed, not overwritten) and the new one takes its name. The
//! new launcher is started with `--updated`, which waits for this one to let go of
//! the one-launcher port, and this one quits. The next start deletes the `.old` file.
//!
//! A launcher built from source (a debug build, or one run from a cargo `target`
//! folder) never updates itself: that would overwrite a developer's own build.

use crate::github::{Asset, Release};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const REPOSITORY: &str = "Sol-Vulpes/sjk-launcher";
/// Given to the new launcher after an update, so it waits for the old one to quit.
pub const UPDATED: &str = "--updated";

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `0.2.10` as numbers, so it sorts after `0.2.9`; `None` for a pre-release or a stray tag.
fn version(tag: &str) -> Option<Vec<u64>> {
    tag.trim_start_matches('v').split('.').map(|part| part.parse().ok()).collect()
}

/// This launcher's archive in a release: `sjk-launcher-windows-x64.zip` and so on.
fn asset_name() -> Option<String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let extension = if os == "windows" { "zip" } else { "tar.gz" };
    Some(format!("{}-{os}-{arch}.{extension}", crate::brand::PROGRAM))
}

/// The program's name inside the archive.
fn program_name() -> String {
    if cfg!(windows) { format!("{}.exe", crate::brand::PROGRAM) } else { crate::brand::PROGRAM.to_owned() }
}

/// Whether this launcher may replace itself: not a build from source.
pub fn allowed() -> bool {
    if cfg!(debug_assertions) || std::env::var_os("SJK_LAUNCHER_NO_UPDATE").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    !exe.components().any(|part| part.as_os_str().to_string_lossy().starts_with("target"))
}

/// Delete what the last update left (`<name>.old`), retrying while the old
/// launcher is still on its way out.
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = with_suffix(&exe, ".old");
    if !old.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..20 {
            if std::fs::remove_file(&old).is_ok() || !old.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Updating,
    Failed(String),
}

/// The newest launcher release when it is newer than this one, and the update's progress.
#[derive(Clone, Default)]
pub struct Updater {
    available: Arc<Mutex<Option<Release>>>,
    phase: Arc<Mutex<Phase>>,
}

impl Updater {
    pub fn available(&self) -> Option<Release> {
        self.available.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    fn set(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(|error| error.into_inner()) = phase;
    }

    /// Ask GitHub for the newest launcher release (blocking; the checker's thread).
    pub fn check(&self) {
        let found = newest().ok().flatten();
        *self.available.lock().unwrap_or_else(|error| error.into_inner()) = found;
    }

    /// Download, check and swap in the new launcher, then start it and quit. On a
    /// failure the running launcher stays as it was. `minimized` keeps the new one in
    /// the tray. Blocking: call it on a worker thread.
    pub fn apply(&self, minimized: bool) {
        let Some(release) = self.available() else { return };
        if self.phase() == Phase::Updating {
            return;
        }
        self.set(Phase::Updating);
        match replace(&release) {
            Ok(exe) => {
                let mut command = std::process::Command::new(&exe);
                command.arg(UPDATED);
                if minimized {
                    command.arg(crate::system::MINIMIZED);
                }
                match command.spawn() {
                    Ok(_) => std::process::exit(0),
                    Err(error) => self.set(Phase::Failed(format!("updated, but cannot start it: {error}; start it again"))),
                }
            }
            Err(error) => self.set(Phase::Failed(error)),
        }
    }
}

/// The newest stable launcher release with an archive for this computer, when newer than this one.
fn newest() -> Result<Option<Release>, String> {
    let Some(wanted) = asset_name() else { return Ok(None) };
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=10");
    let releases: Vec<Release> = crate::github::agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("GitHub: {error}"))?
        .body_mut()
        .read_json()
        .map_err(|error| format!("GitHub's answer: {error}"))?;
    let here = version(current()).unwrap_or_default();
    Ok(releases
        .into_iter()
        .filter(|release| !release.draft && !release.prerelease && release.tag_name.starts_with('v'))
        .filter(|release| release.assets.iter().any(|asset| asset.name == wanted))
        .filter_map(|release| version(&release.tag_name).map(|number| (number, release)))
        .filter(|(number, _)| *number > here)
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, release)| release))
}

/// Put `release`'s program in place of the running one; its path.
fn replace(release: &Release) -> Result<PathBuf, String> {
    let wanted = asset_name().ok_or("no launcher build for this computer")?;
    let asset = release.assets.iter().find(|asset| asset.name == wanted).ok_or("no launcher build for this computer")?;
    let exe = std::env::current_exe().map_err(|error| format!("where am I: {error}"))?;
    let folder = crate::config::folder().ok_or("no settings folder")?.join("update");
    std::fs::create_dir_all(&folder).map_err(|error| format!("cannot make {}: {error}", folder.display()))?;
    let archive = folder.join(&asset.name);
    download(asset, &archive)?;
    crate::install::verify(asset, &archive)?;

    let new = with_suffix(&exe, ".new");
    let bytes = program_from(&archive)?;
    std::fs::write(&new, bytes).map_err(|error| format!("cannot write {}: {error}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_file(&archive);

    let old = with_suffix(&exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|error| {
        let _ = std::fs::remove_file(&new);
        format!("cannot move the running launcher aside: {error}")
    })?;
    if let Err(error) = std::fs::rename(&new, &exe) {
        // Put the old one back so the launcher still starts.
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("cannot put the new launcher in place: {error}"));
    }
    Ok(exe)
}

fn download(asset: &Asset, to: &Path) -> Result<(), String> {
    let response = crate::github::agent()
        .get(&asset.browser_download_url)
        .call()
        .map_err(|error| format!("download: {error}"))?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(256 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("download: {error}"))?;
    if bytes.len() as u64 != asset.size {
        return Err(format!("download ended at {} of {} bytes", bytes.len(), asset.size));
    }
    std::fs::write(to, bytes).map_err(|error| format!("cannot keep the download: {error}"))
}

/// The program file at the archive's root.
fn program_from(archive: &Path) -> Result<Vec<u8>, String> {
    let name = program_name();
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    if archive.extension().is_some_and(|extension| extension == "zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
        zip.by_name(&name)
            .map_err(|_| format!("{name} is not in the update"))?
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut entry = tar
            .entries()
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .find(|entry| entry.path().is_ok_and(|path| path.as_os_str() == name.as_str()))
            .ok_or_else(|| format!("{name} is not in the update"))?;
        entry.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    }
    if bytes.is_empty() {
        return Err(format!("{name} in the update is empty"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_sort_as_numbers() {
        assert!(version("v0.2.10") > version("v0.2.9"));
        assert!(version("v0.3.0") > version("0.2.2"));
        assert_eq!(version("v0.3.0-beta"), None);
    }

    /// `cargo test -- --ignored`: the newest release has this computer's archive
    /// with the program inside.
    #[test]
    #[ignore = "asks GitHub and downloads"]
    fn the_newest_release_carries_the_program() {
        let wanted = asset_name().unwrap();
        let releases: Vec<Release> = crate::github::agent()
            .get(&format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=1"))
            .call()
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap();
        let asset = releases[0].assets.iter().find(|asset| asset.name == wanted).unwrap().clone();
        let path = std::env::temp_dir().join(&asset.name);
        download(&asset, &path).unwrap();
        crate::install::verify(&asset, &path).unwrap();
        assert!(program_from(&path).unwrap().len() > 1_000_000);
    }
}
