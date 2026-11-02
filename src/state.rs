//! The launcher's state shared by the window, the tray and the background
//! checker. The checker runs even while the window is hidden, when the window
//! itself draws nothing.

use crate::config::Config;
use crate::games::{GAMES, Game};
use crate::github::{self, Release};
use crate::install::Install;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Default)]
pub struct Slot {
    /// The last answer from GitHub, newest first.
    pub releases: Mutex<Option<Result<Vec<Release>, String>>>,
    pub checking: AtomicBool,
    pub install: Install,
    child: Mutex<Option<Child>>,
}

/// Where a game stands against its newest release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    NotInstalled,
    /// Installed by hand or before the launcher: the version is not known.
    Unknown,
    UpToDate(String),
    Behind { installed: String, latest: String },
    /// Installed, the releases not known yet.
    Installed(String),
}

pub struct Inner {
    pub ctx: eframe::egui::Context,
    pub config: Mutex<Config>,
    slots: [Slot; 2],
    /// The window, for showing it from the tray (Windows).
    pub hwnd: AtomicIsize,
    pub hidden: AtomicBool,
    /// The tray asked for the SJK chat window.
    pub open_chat: AtomicBool,
}

#[derive(Clone)]
pub struct Core(Arc<Inner>);

impl std::ops::Deref for Core {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

impl Core {
    pub fn new(ctx: eframe::egui::Context, config: Config) -> Self {
        Self(Arc::new(Inner {
            ctx,
            config: Mutex::new(config),
            slots: Default::default(),
            hwnd: AtomicIsize::new(0),
            hidden: AtomicBool::new(false),
            open_chat: AtomicBool::new(false),
        }))
    }

    pub fn slot(&self, game: Game) -> &Slot {
        &self.slots[game as usize]
    }

    pub fn config(&self) -> MutexGuard<'_, Config> {
        lock(&self.config)
    }

    /// Change the config and save it.
    pub fn edit(&self, change: impl FnOnce(&mut Config)) {
        let mut config = self.config();
        change(&mut config);
        config.save();
    }

    pub fn game_data(&self) -> Option<PathBuf> {
        self.config().game_data.clone()
    }

    pub fn releases(&self, game: Game) -> Option<Result<Vec<Release>, String>> {
        lock(&self.slot(game).releases).clone()
    }

    pub fn latest(&self, game: Game) -> Option<Release> {
        self.releases(game)?.ok()?.into_iter().next()
    }

    pub fn standing(&self, game: Game) -> Standing {
        let Some(game_data) = self.game_data() else { return Standing::NotInstalled };
        if !game.is_present(&game_data) {
            return Standing::NotInstalled;
        }
        let Some(installed) = self.config().installed.get(&game).cloned() else {
            return Standing::Unknown;
        };
        match self.latest(game) {
            None => Standing::Installed(installed),
            Some(latest) if latest.tag_name == installed => Standing::UpToDate(installed),
            Some(latest) => Standing::Behind { installed, latest: latest.tag_name },
        }
    }

    /// Ask GitHub for the game's releases, then install the newest by itself
    /// when the player wants that and the game is installed and closed.
    pub fn check(&self, game: Game) {
        if self.slot(game).checking.swap(true, Ordering::AcqRel) {
            return;
        }
        let core = self.clone();
        std::thread::spawn(move || {
            let prereleases = core.config().prereleases(game);
            let answer = github::releases(&github::agent(), game, prereleases);
            *lock(&core.slot(game).releases) = Some(answer);
            core.slot(game).checking.store(false, Ordering::Release);
            let auto = core.config().auto_update;
            if auto
                && let Standing::Behind { .. } | Standing::Unknown = core.standing(game)
                && !core.running(game)
                && let Some(latest) = core.latest(game)
            {
                core.install(game, latest);
            }
            core.ctx.request_repaint();
        });
    }

    pub fn check_all(&self) {
        for game in GAMES {
            self.check(game);
        }
    }

    /// Check now, then every `check_hours`, for the launcher's whole run.
    pub fn start_checker(&self) {
        let core = self.clone();
        std::thread::spawn(move || {
            loop {
                core.check_all();
                let hours = core.config().check_hours.max(1);
                std::thread::sleep(Duration::from_secs(u64::from(hours) * 3600));
            }
        });
    }

    pub fn install(&self, game: Game, release: Release) {
        let Some(game_data) = self.game_data() else { return };
        let ctx = self.ctx.clone();
        let core = self.clone();
        self.slot(game).install.start(
            game,
            release,
            game_data,
            move || ctx.request_repaint(),
            move |game, tag| core.edit(|config| _ = config.installed.insert(game, tag)),
        );
    }

    pub fn running(&self, game: Game) -> bool {
        let mut child = lock(&self.slot(game).child);
        match child.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(_) => {
                *child = None;
                false
            }
            None => false,
        }
    }

    pub fn play(&self, game: Game) -> Result<(), String> {
        let game_data = self.game_data().ok_or("choose the GameData folder first")?;
        if self.running(game) {
            return Ok(());
        }
        if self.slot(game).install.running() {
            return Err(format!("{} is installing", game.name()));
        }
        let child = game
            .launch(&game_data)
            .map_err(|error| format!("cannot start {}: {error}", game.name()))?;
        *lock(&self.slot(game).child) = Some(child);
        Ok(())
    }

    /// The tray's tooltip: updates waiting, or what is installed.
    pub fn summary(&self) -> String {
        let mut lines = vec![crate::brand::NAME.to_owned()];
        for game in GAMES {
            let line = match self.standing(game) {
                Standing::Behind { latest, .. } => format!("{}: {} available", game.name(), game.version_of(&latest)),
                Standing::UpToDate(tag) | Standing::Installed(tag) => format!("{} {}", game.name(), game.version_of(&tag)),
                Standing::Unknown => format!("{}: installed", game.name()),
                Standing::NotInstalled => format!("{}: not installed", game.name()),
            };
            lines.push(line);
        }
        lines.join("\n")
    }
}
