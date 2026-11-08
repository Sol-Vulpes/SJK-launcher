//! What the launcher remembers between runs, kept as JSON in the platform's
//! config folder (`%APPDATA%\SJK\Launcher` on Windows).

use crate::games::Game;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Jedi Academy's `GameData`; both games install into it.
    pub game_data: Option<PathBuf>,
    /// The release tag the launcher last installed, per game.
    pub installed: BTreeMap<Game, String>,
    /// Offer pre-releases (JoF betas, SJK alphas are full releases already).
    pub prereleases: BTreeMap<Game, bool>,
    /// Install a new release by itself when the game is not running.
    pub auto_update: bool,
    /// Start with the system, hidden in the tray.
    pub auto_start: bool,
    /// Closing the window keeps the launcher in the tray.
    pub close_to_tray: bool,
    /// Hours between background checks.
    pub check_hours: u32,
    /// JoF content groups and file picks, written as SJK writes
    /// `jof.contentGroups` and `jof.contentFiles`.
    pub jof_groups: String,
    pub jof_files: String,
    /// Another Sith cloud server (`sith-cloud`) set by hand; empty uses the one the SJK
    /// hub names. And the picks made from it.
    pub sith_url: String,
    pub sith_groups: String,
    pub sith_files: String,
    /// MD5s of PK3s the player chose to download although the scan found danger.
    pub trusted: std::collections::BTreeSet<String>,
    /// Developer mode: Sol JK's page also builds and starts a branch of SJK's source
    /// (`dev.rs`).
    pub dev_mode: bool,
    /// The SJK source checkout developer mode builds from.
    pub dev_source: Option<PathBuf>,
    /// The branch it builds; `None` is whatever the source folder has checked out.
    pub dev_branch: Option<String>,
    /// The graphics API Sol JK starts with (Doctor).
    pub graphics: Graphics,
    /// When the player last sent their logs from Doctor, unix seconds (its cooldown).
    pub logs_sent: u64,
}

/// The graphics API Sol JK uses: wgpu's own pick, or one forced through `WGPU_BACKEND`
/// for the game only (a driver whose Vulkan freezes may run DirectX 12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Graphics {
    #[default]
    Auto,
    Vulkan,
    Dx12,
}

impl Graphics {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Vulkan, Self::Dx12];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Vulkan => "Vulkan",
            Self::Dx12 => "DirectX 12",
        }
    }

    /// `WGPU_BACKEND`'s value for the game, `None` to leave wgpu to choose.
    pub fn backend(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Vulkan => Some("vulkan"),
            Self::Dx12 => Some("dx12"),
        }
    }
}

/// The server written in as the default before the hub named it; a saved config holding
/// it asks the hub instead.
const OLD_SITH_URL: &str = "http://64.177.112.251:8080";

impl Default for Config {
    fn default() -> Self {
        Self {
            game_data: None,
            installed: BTreeMap::new(),
            prereleases: BTreeMap::new(),
            auto_update: false,
            auto_start: false,
            close_to_tray: true,
            check_hours: 3,
            jof_groups: sjk_jof_content::plan::DEFAULT.into(),
            jof_files: String::new(),
            sith_url: String::new(),
            sith_groups: sjk_jof_content::plan::DEFAULT.into(),
            sith_files: String::new(),
            trusted: Default::default(),
            dev_mode: false,
            dev_source: None,
            dev_branch: None,
            graphics: Graphics::Auto,
            logs_sent: 0,
        }
    }
}

pub fn folder() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "SJK", crate::brand::FOLDER).map(|dirs| dirs.config_dir().to_owned())
}

fn file() -> Option<PathBuf> {
    folder().map(|folder| folder.join("launcher.json"))
}

impl Config {
    pub fn load() -> Self {
        file()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .map(|mut config| {
                if config.sith_url.trim_end_matches('/') == OLD_SITH_URL {
                    config.sith_url.clear();
                }
                config
            })
            .unwrap_or_default()
    }

    /// Written to a temporary file then renamed, so a crash never leaves half a file.
    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(bytes) = serde_json::to_vec_pretty(self) else { return };
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(&temporary, &path);
        }
    }

    pub fn prereleases(&self, game: Game) -> bool {
        self.prereleases.get(&game).copied().unwrap_or(false)
    }
}
