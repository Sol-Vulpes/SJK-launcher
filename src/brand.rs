//! Which launcher this is: the SJK Launcher, or the JoF Launcher built with
//! `--features jof` for the Jedi of Freedom clan. Both do the same; the JoF one
//! wears the clan's green J o F and colours, puts JoF EternalJK first, and keeps
//! its own settings, tray entry and autostart, so both can live on one computer.

use crate::games::Game;

#[cfg(not(feature = "jof"))]
mod chosen {
    use super::Game;
    pub const NAME: &str = "SJK Launcher";
    /// The settings folder's application name (`%APPDATA%\SJK\<this>`).
    pub const FOLDER: &str = "Launcher";
    /// The local port that keeps it to one launcher at a time (`instance.rs`).
    pub const INSTANCE_PORT: u16 = 47613;
    pub const GAMES: [Game; 2] = [Game::Sjk, Game::Jof];
    pub const EMBLEM: &[u8] = include_bytes!("../assets/sjk-logo-512.png");
    pub const ICON: &[u8] = include_bytes!("../assets/launcher-icon.png");
}

#[cfg(feature = "jof")]
mod chosen {
    use super::Game;
    pub const NAME: &str = "JoF Launcher";
    pub const FOLDER: &str = "JoF Launcher";
    pub const INSTANCE_PORT: u16 = 47614;
    pub const GAMES: [Game; 2] = [Game::Jof, Game::Sjk];
    pub const EMBLEM: &[u8] = include_bytes!("../assets/jof/jof-emblem-512.png");
    pub const ICON: &[u8] = include_bytes!("../assets/jof/launcher-icon.png");
}

pub use chosen::*;
