//! The launcher's state shared by the window, the tray and the background
//! checker. The checker runs even while the window is hidden, when the window
//! itself draws nothing.

use crate::config::Config;
use crate::games::{GAMES, Game};
use crate::github::{self, Release};
use crate::install::Install;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Slot {
    /// The last answer from GitHub, newest first.
    pub releases: Mutex<Option<Result<Vec<Release>, String>>>,
    pub checking: AtomicBool,
    pub install: Install,
    /// The game started from here still runs (a thread waits on it).
    running: AtomicBool,
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
    /// The last game that went down, for the crash window.
    pub exit: Mutex<Option<crate::game_log::Exit>>,
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
            exit: Mutex::new(None),
        }))
    }

    /// Bring the window up: from the tray, a second start, or a game gone down. A
    /// hidden eframe window gets no frames on Windows, so the system shows it.
    pub fn show(&self) {
        self.hidden.store(false, Ordering::Relaxed);
        crate::system::show_window(self.hwnd.load(Ordering::Relaxed));
        self.ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
        self.ctx.request_repaint();
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
        self.slot(game).running.load(Ordering::Acquire)
    }

    pub fn play(&self, game: Game) -> Result<(), String> {
        let game_data = self.game_data().ok_or("choose the GameData folder first")?;
        if self.running(game) {
            return Ok(());
        }
        if self.slot(game).install.running() {
            return Err(format!("{} is installing", game.name()));
        }
        let log = crate::game_log::create(game);
        let mut child = game
            .launch(&game_data, log.as_ref().map(|(file, _)| file))
            .map_err(|error| format!("cannot start {}: {error}", game.name()))?;
        let slot = self.slot(game);
        slot.running.store(true, Ordering::Release);
        let started = Instant::now();
        let core = self.clone();
        let log = log.map(|(_, path)| path);
        std::thread::spawn(move || {
            let status = child.wait();
            core.slot(game).running.store(false, Ordering::Release);
            if let (Ok(status), Some(log)) = (status, log)
                && let Some(what) = crate::game_log::bad_exit(status)
            {
                core.went_down(game, child.id(), what, started.elapsed(), log, &game_data);
            }
            core.ctx.request_repaint();
        });
        Ok(())
    }

    /// `game` (process `pid`) went down: report it for Sol JK, and show the window
    /// with what happened.
    fn went_down(&self, game: Game, pid: u32, what: String, ran: Duration, log: PathBuf, game_data: &std::path::Path) {
        use crate::game_log::{self, Report};
        let report = match game {
            Game::Sjk => {
                let build = self.config().installed.get(&game).map(|tag| game.version_of(tag)).unwrap_or_default();
                game_log::report_sjk(&game_data.join("SJK"), pid, &what, ran, &game_log::report_output(&log), &build)
            }
            Game::Jof => Report::None,
        };
        *lock(&self.exit) = Some(game_log::exit(game, what, ran, log, report));
        self.show();
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
