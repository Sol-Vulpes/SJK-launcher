//! The two games: where their releases are, which archive fits this computer,
//! and what to start once it is extracted into `GameData`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Game {
    Sjk,
    Jof,
}

/// In the launcher's order: its own game first (SJK, or JoF EternalJK for the JoF Launcher).
pub const GAMES: [Game; 2] = crate::brand::GAMES;

impl Game {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sjk => "Sol JK",
            Self::Jof => "JoF EternalJK",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Sjk => "SJK, the Rust Jedi Academy client",
            Self::Jof => "Jedi of Freedom's EternalJK client",
        }
    }

    /// `owner/name` on GitHub.
    pub fn repository(self) -> &'static str {
        match self {
            Self::Sjk => "Sol-Vulpes/SJK",
            Self::Jof => "JediofFreedom/JoF_EJK",
        }
    }

    /// Only these tags are releases of the game (SJK's repository could carry others).
    pub fn is_release_tag(self, tag: &str) -> bool {
        match self {
            Self::Sjk => tag.starts_with("sjk-v"),
            Self::Jof => !tag.starts_with("reserve/"),
        }
    }

    /// The version as players say it: `2026.1027.1`, `1.6.2`.
    pub fn version_of(self, tag: &str) -> String {
        match self {
            Self::Sjk => tag.trim_start_matches("sjk-v").to_owned(),
            Self::Jof => tag.to_owned(),
        }
    }

    /// The release asset for this computer, or `None` when the game has no build for it.
    pub fn asset_name(self, tag: &str) -> Option<String> {
        let version = self.version_of(tag);
        let platform = match (self, std::env::consts::OS, std::env::consts::ARCH) {
            (Self::Sjk, "windows", "x86_64") => "windows-x64.zip",
            (Self::Sjk, "linux", "x86_64") => "linux-x64.zip",
            (Self::Sjk, "macos", "aarch64") => "macos-arm64.zip",
            (Self::Jof, "windows", "x86_64") => "windows-x86_64.zip",
            (Self::Jof, "windows", "x86") => "windows-x86.zip",
            (Self::Jof, "linux", "x86_64") => "linux-x86_64.tar.gz",
            (Self::Jof, "linux", "x86") => "linux-x86.tar.gz",
            (Self::Jof, "macos", "aarch64") => "macos-arm64.tar.gz",
            (Self::Jof, "macos", "x86_64") => "macos-x86_64.tar.gz",
            _ => return None,
        };
        Some(match self {
            Self::Sjk => format!("SJK-{version}-{platform}"),
            Self::Jof => format!("JoF-{platform}"),
        })
    }

    /// The program inside `GameData`, as each release lays it out.
    pub fn executable(self) -> Option<&'static str> {
        Some(match (self, std::env::consts::OS, std::env::consts::ARCH) {
            (Self::Sjk, "windows", _) => "sjk.exe",
            (Self::Sjk, "macos", _) => "sjk-client",
            (Self::Sjk, _, _) => "sjk",
            (Self::Jof, "windows", "x86") => "eternaljk.x86.exe",
            (Self::Jof, "windows", _) => "eternaljk.x86_64.exe",
            (Self::Jof, "macos", "aarch64") => "eternaljk.arm64.app",
            (Self::Jof, "macos", _) => "eternaljk.x86_64.app",
            (Self::Jof, _, "x86") => "eternaljk.i386",
            (Self::Jof, _, _) => "eternaljk.x86_64",
        })
    }

    pub fn executable_in(self, game_data: &Path) -> Option<PathBuf> {
        self.executable().map(|name| game_data.join(name))
    }

    pub fn is_present(self, game_data: &Path) -> bool {
        self.executable_in(game_data).is_some_and(|path| path.exists())
    }

    /// Start `program` from `folder` for `game_data`: a release from `GameData`, or a
    /// developer's own build from its source folder (`dev.rs`).
    pub fn start(self, program: &Path, folder: &Path, game_data: &Path, log: Option<&std::fs::File>) -> std::io::Result<Child> {
        let mut command = if program.extension().is_some_and(|ext| ext == "app") {
            let mut open = Command::new("open");
            open.arg("-W").arg(program);
            open
        } else {
            Command::new(program)
        };
        command.current_dir(folder);
        if self == Self::Sjk {
            // SJK finds the game through these when started without arguments.
            command.env("JKA_GAME_DATA", game_data).env("JKR_GAME_DATA", game_data);
        }
        if let Some(log) = log
            && let (Ok(out), Ok(err)) = (log.try_clone(), log.try_clone())
        {
            command.stdout(out).stderr(err);
        }
        command.spawn()
    }
}
