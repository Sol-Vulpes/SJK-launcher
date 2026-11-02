//! SJK chat in the launcher (SJK's `docs/hub-chat.md`): the same conversation as
//! the game's, carried by the SJK hub and signed by the player's own SJK identity.
//!
//! The launcher never makes or changes that identity: it reads `identity.key`, the
//! hub's address (`cl_hubUrl`) and the name worn (`name`) from SJK's settings folder,
//! `GameData/SJK`. It reads the hub's feed only while the chat window is open and no
//! game started from here is running, so it never spends the key's reads (40 a
//! minute) while the game reads them too. The hub tells its messages apart by the
//! `User-Agent`, [`AGENT`].

use sjk_identity::wire::ChatMessage;
use sjk_identity::{HttpHub, Hub, HubError, Identity, chat as rules};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const AGENT: &str = concat!("SJKLauncher/", env!("CARGO_PKG_VERSION"));
/// SJK's own default for `cl_hubUrl`.
const DEFAULT_HUB: &str = "https://sjk.dfox.app";
/// Messages kept, as SJK keeps them.
const KEPT: usize = 200;
/// How long the hub may hold a read open, and how long the launcher waits for it.
const FEED_WAIT: u64 = 25;
const FEED_TIMEOUT: Duration = Duration::from_secs(40);
/// Between two reads: at most 40 a minute, the hub's allowance for one key.
const READ_GAP: Duration = Duration::from_millis(1500);
/// The SJK chat's gold (`sjk_chat_look::GOLD` in SJK).
pub const GOLD: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(0xF5, 0xC7, 0x56);

/// What the window says above the messages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet.
    #[default]
    Idle,
    Connecting,
    Live,
    /// A game started from here is running: the game reads the chat itself.
    Paused,
    /// Cannot chat from here, and why, in words for the player.
    Unavailable(String),
    /// The hub could not be read; it is asked again.
    Failing(String),
}

/// One message as the window shows it, already made safe to show.
#[derive(Clone, Debug)]
pub struct Line {
    pub id: u64,
    pub at: i64,
    pub key_id: String,
    /// Colour codes kept, as SJK shows names.
    pub name: String,
    /// Colour codes dropped: a message is all gold.
    pub text: String,
    pub verified: bool,
    pub staff: bool,
}

impl Line {
    fn new(message: &ChatMessage) -> Self {
        Self {
            id: message.id,
            at: message.at,
            key_id: message.key_id.clone(),
            name: rules::name_for_display(&message.name),
            text: without_colours(&rules::for_display(&message.text)),
            verified: message.verified,
            staff: message.staff || message.creator,
        }
    }
}

/// What the player read and sent so far.
#[derive(Default)]
pub struct View {
    pub status: Status,
    pub lines: VecDeque<Line>,
    pub online: u32,
    /// The player's own key id, to tell their lines apart.
    pub own_key: String,
    pub sending: bool,
    /// Why the last message did not go.
    pub refused: Option<String>,
}

/// Who the player is in SJK, read from its settings folder.
struct Session {
    identity: Identity,
    hub_url: String,
    name: String,
    mutes: Vec<(Option<String>, String)>,
}

pub struct Chat {
    view: Arc<Mutex<View>>,
    /// Read the feed now (window open, no game running).
    active: Arc<AtomicBool>,
    /// SJK's settings folder, `GameData/SJK`.
    folder: Arc<Mutex<Option<PathBuf>>>,
    session: Arc<Mutex<Option<Arc<Session>>>>,
    outbox: Sender<String>,
    inbox: Mutex<Option<Receiver<String>>>,
    started: AtomicBool,
    ctx: eframe::egui::Context,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Chat {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        let (outbox, inbox) = mpsc::channel();
        Self {
            view: Default::default(),
            active: Default::default(),
            folder: Default::default(),
            session: Default::default(),
            outbox,
            inbox: Mutex::new(Some(inbox)),
            started: AtomicBool::new(false),
            ctx,
        }
    }

    pub fn view(&self) -> MutexGuard<'_, View> {
        lock(&self.view)
    }

    /// Read the chat while `on`; `paused` says a game is running. The workers start
    /// the first time the chat is wanted.
    pub fn set_active(&self, on: bool, paused: bool, folder: Option<PathBuf>) {
        *lock(&self.folder) = folder;
        let reading = on && !paused;
        if self.active.swap(reading, Ordering::Relaxed) != reading {
            let mut view = self.view();
            if paused {
                view.status = Status::Paused;
            } else if !reading && view.status != Status::Idle {
                view.status = Status::Idle;
            }
        }
        if reading && !self.started.swap(true, Ordering::Relaxed) {
            self.start();
        }
    }

    /// Check `raw` as the hub will and queue it; `Err` says why it cannot go.
    pub fn send(&self, raw: &str) -> Result<(), String> {
        let text = rules::check(raw).map_err(|error| error.to_string())?;
        let mut view = self.view();
        if view.status != Status::Live {
            return Err("Not connected to SJK chat".to_owned());
        }
        view.sending = true;
        view.refused = None;
        drop(view);
        self.outbox.send(text).map_err(|_| "The chat stopped".to_owned())
    }

    fn start(&self) {
        let reader = Reader {
            view: Arc::clone(&self.view),
            active: Arc::clone(&self.active),
            folder: Arc::clone(&self.folder),
            session: Arc::clone(&self.session),
            ctx: self.ctx.clone(),
        };
        let _ = std::thread::Builder::new().name("sjk-chat-feed".into()).spawn(move || reader.run());
        if let Some(inbox) = lock(&self.inbox).take() {
            let view = Arc::clone(&self.view);
            let session = Arc::clone(&self.session);
            let ctx = self.ctx.clone();
            let _ = std::thread::Builder::new()
                .name("sjk-chat-send".into())
                .spawn(move || send_loop(&inbox, &view, &session, &ctx));
        }
    }
}

struct Reader {
    view: Arc<Mutex<View>>,
    active: Arc<AtomicBool>,
    folder: Arc<Mutex<Option<PathBuf>>>,
    session: Arc<Mutex<Option<Arc<Session>>>>,
    ctx: eframe::egui::Context,
}

impl Reader {
    fn set(&self, status: Status) {
        lock(&self.view).status = status;
        self.ctx.request_repaint();
    }

    fn run(self) {
        loop {
            if !self.active.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            // Read SJK's settings again each time the chat opens: the key, the hub
            // and the name may have changed in the game meanwhile.
            let folder = lock(&self.folder).clone();
            let session = match folder.as_deref().ok_or_else(|| "Choose the GameData folder in Settings".to_owned()).and_then(Session::read) {
                Ok(session) => Arc::new(session),
                Err(why) => {
                    self.set(Status::Unavailable(why));
                    while self.active.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    continue;
                }
            };
            *lock(&self.session) = Some(Arc::clone(&session));
            lock(&self.view).own_key = session.identity.key_id();
            self.read(&session);
        }
    }

    /// Read the feed while the chat is wanted.
    fn read(&self, session: &Session) {
        let mut hub = match HttpHub::with_timeout(&session.hub_url, AGENT, FEED_TIMEOUT) {
            Ok(hub) => hub,
            Err(error) => return self.set(Status::Failing(error.to_string())),
        };
        self.set(Status::Connecting);
        let mut after = 0;
        let mut backoff = Duration::from_secs(2);
        while self.active.load(Ordering::Relaxed) {
            let started = Instant::now();
            let fresh = after == 0;
            match hub.feed(&session.identity, after, None, if fresh { 0 } else { FEED_WAIT }) {
                Ok(feed) => {
                    let mut view = lock(&self.view);
                    // A first read, or ids going back (the hub restarted): the backlog.
                    if fresh || feed.next < after {
                        view.lines.clear();
                    }
                    view.lines.retain(|line| !feed.deleted.contains(&line.id));
                    for message in &feed.chat {
                        if !session.muted(message) && !view.lines.iter().any(|line| line.id == message.id) {
                            view.lines.push_back(Line::new(message));
                        }
                    }
                    while view.lines.len() > KEPT {
                        view.lines.pop_front();
                    }
                    view.online = feed.online;
                    view.status = Status::Live;
                    drop(view);
                    self.ctx.request_repaint();
                    after = feed.next;
                    backoff = Duration::from_secs(2);
                    std::thread::sleep(READ_GAP.saturating_sub(started.elapsed()));
                }
                Err(error) => {
                    self.set(Status::Failing(words(&error)));
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }
}

fn send_loop(inbox: &Receiver<String>, view: &Mutex<View>, session: &Mutex<Option<Arc<Session>>>, ctx: &eframe::egui::Context) {
    let mut hubs: Option<(String, HttpHub)> = None;
    for text in inbox {
        let Some(session) = lock(session).clone() else { continue };
        if hubs.as_ref().is_none_or(|(url, _)| *url != session.hub_url) {
            hubs = HttpHub::new(&session.hub_url, AGENT).ok().map(|hub| (session.hub_url.clone(), hub));
        }
        let outcome = match &mut hubs {
            Some((_, hub)) => hub.chat(&session.identity, &text, &session.name).map(drop).map_err(|error| words(&error)),
            None => Err("The hub's address is not valid".to_owned()),
        };
        let mut view = lock(view);
        view.sending = false;
        view.refused = outcome.err();
        drop(view);
        ctx.request_repaint();
    }
}

/// SJK's hub address from its settings folder: `cl_hubUrl`, SJK's default when it is
/// not set, empty when the player set none.
pub fn hub_url(folder: &Path) -> String {
    let config = std::fs::read_to_string(folder.join("config.cfg")).unwrap_or_default();
    setting(&config, "cl_hubUrl").unwrap_or_else(|| DEFAULT_HUB.to_owned())
}

impl Session {
    fn read(folder: &Path) -> Result<Self, String> {
        let config = std::fs::read_to_string(folder.join("config.cfg")).unwrap_or_default();
        let setting = |name: &str| setting(&config, name);
        if setting("cl_identity").as_deref() == Some("0") {
            return Err("Your SJK identity is off in SJK's settings".to_owned());
        }
        if setting("cl_sjkChat").as_deref() == Some("0") {
            return Err("SJK chat is off in SJK's settings (Settings > Chat)".to_owned());
        }
        let hub_url = setting("cl_hubUrl").unwrap_or_else(|| DEFAULT_HUB.to_owned());
        if hub_url.is_empty() {
            return Err("SJK has no hub set (cl_hubUrl is empty)".to_owned());
        }
        if !sjk_identity::valid_base_url(&hub_url) {
            return Err(format!("SJK's hub address is not one the launcher can use: {hub_url}"));
        }
        let key = folder.join("identity.key");
        let text = std::fs::read_to_string(&key).map_err(|_| "No SJK identity yet: start SJK once and it makes one".to_owned())?;
        let identity = Identity::parse(&text).map_err(|error| format!("SJK's identity.key cannot be read: {error}"))?;
        let mutes = std::fs::read_to_string(folder.join("chat-mutes.txt")).map(|text| parse_mutes(&text)).unwrap_or_default();
        Ok(Self {
            identity,
            hub_url,
            name: setting("name").unwrap_or_default(),
            mutes,
        })
    }

    /// Muted on this PC in SJK (`chat-mutes.txt`): by key, or by name for a player
    /// muted without one.
    fn muted(&self, message: &ChatMessage) -> bool {
        self.mutes.iter().any(|(key, name)| match key {
            Some(key) => *key == message.key_id,
            None => sjk_identity::names_match(name, &message.name),
        })
    }
}

/// `chat-mutes.txt`'s lines: the key (or `-`), a tab, the name.
fn parse_mutes(text: &str) -> Vec<(Option<String>, String)> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, name)| ((key != "-").then(|| key.to_owned()), name.to_owned()))
        .collect()
}

/// A setting's value in a `config.cfg` (`seta name "value"`, quotes optional).
pub(crate) fn setting(config: &str, wanted: &str) -> Option<String> {
    config.lines().rev().find_map(|line| {
        let line = line.trim();
        let rest = ["seta ", "set ", "sets ", "setu "].iter().find_map(|command| line.strip_prefix(command))?;
        let (name, value) = rest.trim_start().split_once(char::is_whitespace)?;
        if !name.eq_ignore_ascii_case(wanted) {
            return None;
        }
        let value = value.trim();
        Some(match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().unwrap_or_default().to_owned(),
            None => value.split_whitespace().next().unwrap_or_default().to_owned(),
        })
    })
}

/// A hub error in the player's words: the hub's own text when it refused.
fn words(error: &HubError) -> String {
    match error {
        HubError::Rejected { message, .. } if !message.is_empty() => message.clone(),
        HubError::Network(_) => "Cannot reach the SJK hub".to_owned(),
        other => other.to_string(),
    }
}

/// Text without Jedi Academy's colour codes (`^` and a digit).
pub fn without_colours(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(char::is_ascii_digit) {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// "now", "3 minutes ago", "yesterday", as SJK's chat page words it.
pub fn ago(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0);
    match seconds {
        0..60 => "now".to_owned(),
        60..120 => "a minute ago".to_owned(),
        120..3600 => format!("{} minutes ago", seconds / 60),
        3600..7200 => "an hour ago".to_owned(),
        7200..86400 => format!("{} hours ago", seconds / 3600),
        86400..172800 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_read_as_the_game_writes_them() {
        let config = "seta name \"^2Sol ^7Vulpes\"\nseta cl_hubUrl \"https://sjk.dfox.app\"\nset cl_sjkChat 1\nseta cl_identity \"1\"\n";
        assert_eq!(setting(config, "name").as_deref(), Some("^2Sol ^7Vulpes"));
        assert_eq!(setting(config, "cl_hubUrl").as_deref(), Some("https://sjk.dfox.app"));
        assert_eq!(setting(config, "cl_sjkchat").as_deref(), Some("1"));
        assert_eq!(setting(config, "cl_missing"), None);
        // The last line wins, as when the game runs the file.
        assert_eq!(setting("seta name A\nseta name B\n", "name").as_deref(), Some("B"));
    }

    #[test]
    fn mutes_are_read_by_key_or_name() {
        let mutes = parse_mutes("0123456789abcdef\tSol\n-\tTroll\nnonsense\n");
        assert_eq!(mutes, vec![(Some("0123456789abcdef".to_owned()), "Sol".to_owned()), (None, "Troll".to_owned())]);
    }

    #[test]
    fn colour_codes_leave_the_text() {
        assert_eq!(without_colours("^1red ^7and ^ alone^"), "red and ^ alone^");
    }

    #[test]
    fn times_read_like_sjk() {
        assert_eq!(ago(100, 130), "now");
        assert_eq!(ago(0, 180), "3 minutes ago");
        assert_eq!(ago(0, 90_000), "yesterday");
    }
}
