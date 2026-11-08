//! Doctor: what usually stops Sol JK from starting, checked on the player's PC, the
//! fixes the launcher can make, and Send my logs, which sends the launcher's and SJK's
//! logs to SJK's team unasked (hub `PROTOCOL.md`, "Log requests") when the game does
//! not start or freezes and so never sends a crash report of its own.
//!
//! Since 08/11/2026 (Sol asked, after a friend hit Windows' error 740 and then a
//! white screen, with nothing reaching the hub either time).

use crate::config::Graphics;
use crate::games::Game;
use sjk_identity::crash::without_home;
use sjk_identity::{Hub, HubError, HttpHub, Identity, LogUpload};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Between two sends from the launcher, in seconds (the hub has its own limits).
pub const SEND_EVERY: u64 = 600;
/// The base game's packs; without them nothing loads.
const BASE_PACKS: [&str; 3] = ["assets0.pk3", "assets1.pk3", "assets2.pk3"];
/// The Windows key holding "Run this program as an administrator" and its kin.
#[cfg(windows)]
const LAYERS: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Warn,
    Bad,
}

/// What the launcher can do about a check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Remove "Run as administrator" from `exe`'s compatibility marks; `all_users` is
    /// the machine-wide one, which needs Windows' permission prompt.
    ClearRunAsAdmin { exe: PathBuf, all_users: bool },
    /// Choose the GameData folder (Settings).
    ChooseGameData,
    /// Install Sol JK (its page).
    Install,
}

impl Fix {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClearRunAsAdmin { all_users: false, .. } => "Remove the mark",
            Self::ClearRunAsAdmin { all_users: true, .. } => "Remove it for all users",
            Self::ChooseGameData => "Open Settings",
            Self::Install => "Go to Sol JK",
        }
    }
}

/// One thing Doctor looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub fix: Option<Fix>,
}

impl Check {
    fn new(level: Level, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { level, title: title.into(), detail: detail.into(), fix: None }
    }

    fn with(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// Everything Doctor checks, for `game_data` with the launcher's `graphics` choice.
pub fn checks(game_data: Option<&Path>, graphics: Graphics) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(game_data) = game_data else {
        checks.push(
            Check::new(Level::Bad, "No game folder", "The launcher does not know where Jedi Academy's GameData folder is.")
                .with(Fix::ChooseGameData),
        );
        return checks;
    };
    checks.push(game_folder(game_data));
    let exe = Game::Sjk.executable_in(game_data).filter(|exe| exe.exists());
    match &exe {
        Some(exe) => {
            checks.push(Check::new(Level::Good, "Sol JK is installed", shown(exe)));
            checks.extend(run_as_admin(exe));
        }
        None => checks.push(Check::new(Level::Warn, "Sol JK is not installed", "Install it from its page.").with(Fix::Install)),
    }
    let sjk = game_data.join("SJK");
    checks.push(writable(&sjk));
    checks.push(identity(&sjk));
    checks.push(graphics_check(graphics));
    checks.push(last_run(&read(&crate::game_log::path(Game::Sjk).unwrap_or_default())));
    checks
}

fn game_folder(game_data: &Path) -> Check {
    let missing: Vec<&str> = BASE_PACKS.into_iter().filter(|pack| !game_data.join("base").join(pack).exists()).collect();
    if missing.is_empty() {
        Check::new(Level::Good, "Jedi Academy's files are there", shown(game_data))
    } else {
        Check::new(
            Level::Bad,
            "Jedi Academy's files are missing",
            format!("{} has no {} in base: choose the GameData folder of a full install.", shown(game_data), missing.join(", ")),
        )
        .with(Fix::ChooseGameData)
    }
}

/// The player's folder written as `%USERPROFILE%`.
fn shown(path: &Path) -> String {
    without_home(&path.display().to_string(), &crate::game_log::home())
}

/// SJK writes its settings, key, logs and crash reports into `sjk`: a folder Windows
/// protects (Program Files without permission) keeps it from starting cleanly.
fn writable(sjk: &Path) -> Check {
    let probe = sjk.join(format!(".doctor-{}", std::process::id()));
    let result = std::fs::create_dir_all(sjk).and_then(|()| std::fs::write(&probe, b"doctor"));
    let _ = std::fs::remove_file(&probe);
    match result {
        Ok(()) => Check::new(Level::Good, "SJK can save its settings", shown(sjk)),
        Err(error) => Check::new(
            Level::Bad,
            "SJK cannot save its settings",
            format!(
                "Writing into {} failed ({error}). Windows protects folders like Program Files: move Jedi Academy out of it (Steam: Properties > Installed files > Move), or allow the folder in your antivirus.",
                shown(sjk)
            ),
        ),
    }
}

fn identity(sjk: &Path) -> Check {
    let config = std::fs::read_to_string(sjk.join("config.cfg")).unwrap_or_default();
    if crate::chat::setting(&config, "cl_identity").as_deref() == Some("0") {
        return Check::new(
            Level::Warn,
            "Your SJK identity is off",
            "Crash reports and Send my logs need it (cl_identity 0 in SJK's settings); copy the log by hand instead.",
        );
    }
    match std::fs::read_to_string(sjk.join("identity.key")) {
        Ok(text) => match Identity::parse(&text) {
            Ok(identity) => Check::new(Level::Good, "Your SJK identity is ready", format!("Key {}", identity.key_id())),
            Err(error) => Check::new(Level::Bad, "Your SJK identity cannot be read", format!("identity.key: {error}")),
        },
        Err(_) => Check::new(
            Level::Good,
            "No SJK identity yet",
            "SJK makes it at its first start; Send my logs makes it if the game never got that far.",
        ),
    }
}

fn graphics_check(graphics: Graphics) -> Check {
    let system = std::env::var("WGPU_BACKEND").ok().filter(|value| !value.trim().is_empty());
    match (graphics, system) {
        (Graphics::Auto, Some(value)) => Check::new(
            Level::Warn,
            format!("Your system forces WGPU_BACKEND={value}"),
            "Every program started from here gets it. Pick a graphics API below to override it for Sol JK, or remove the variable (Windows: Edit environment variables).",
        ),
        (Graphics::Auto, None) => Check::new(Level::Good, "Graphics: automatic", "Sol JK picks Vulkan where it can, else DirectX 12."),
        (forced, _) => Check::new(Level::Good, format!("Graphics: {}", forced.label()), "Set below, for Sol JK only."),
    }
}

/// What the last run's log (`text`, the launcher's `sjk.log`) says about how it went.
pub fn last_run(text: &str) -> Check {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return Check::new(Level::Good, "No run started from the launcher yet", "Its log will be checked once there is one.");
    }
    let card = gpu(text).map(|card| format!(" Graphics card: {card}.")).unwrap_or_default();
    let driver = "Update the graphics driver from NVIDIA's, AMD's or Intel's site; on a laptop, set sjk.exe to High performance (Windows Settings > Display > Graphics); or try DirectX 12 below.";
    if let Some(line) = lines.iter().find(|line| line.contains("Out of Memory") || line.contains("out of video memory")) {
        return Check::new(
            Level::Bad,
            "The last run ran out of video memory",
            format!("{}\nLower Texture detail in SJK's settings and close programs that use the graphics card (browsers, videos).{card}", cut(line)),
        );
    }
    if let Some(line) = lines
        .iter()
        .find(|line| line.contains("panicked at") || line.contains("MapRangeError") || line.contains("wgpu error") || line.contains("Device lost"))
    {
        return Check::new(Level::Bad, "The last run crashed on a graphics error", format!("{}\n{driver}{card}", cut(line)));
    }
    match lines.iter().rposition(|line| line.contains("client state: main menu")) {
        // Reaching the menu, SJK builds the scene behind it at once (`world/model
        // filtering` within a second): nothing after the line is a freeze before the
        // first picture, nearly always the graphics driver.
        Some(at) if at + 1 == lines.len() => Check::new(
            Level::Warn,
            "The last run stopped before drawing anything",
            format!("It reached the main menu, then printed nothing more: a white or black window that never draws. {driver}{card}"),
        ),
        Some(_) => Check::new(Level::Good, "The last run reached the menus", format!("No error in its log.{card}")),
        None => Check::new(
            Level::Warn,
            "The last run never reached the main menu",
            format!("Its last line: {}{card}", cut(lines[lines.len() - 1])),
        ),
    }
}

/// The graphics adapter SJK named in its log ("graphics card: NAME, 3938 MiB ...").
pub fn gpu(text: &str) -> Option<String> {
    let line = text.lines().find_map(|line| line.split_once("graphics card: ").map(|(_, rest)| rest))?;
    Some(line.split(", ").next().unwrap_or(line).trim().to_owned())
}

fn cut(line: &str) -> String {
    let line = line.trim();
    match line.char_indices().nth(240) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

fn read(path: &Path) -> String {
    std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default()
}

// "Run this program as an administrator": a compatibility mark Windows keeps per user
// (HKCU) and for all users (HKLM). With it, a program started the ordinary way (as the
// launcher does) fails with error 740.

/// The compatibility marks naming `exe`, per hive: the hive, the value's name as
/// written there and its data.
#[cfg(windows)]
fn marks(exe: &Path) -> Vec<(&'static str, String, String)> {
    let mut found = Vec::new();
    for hive in ["HKCU", "HKLM"] {
        let mut command = std::process::Command::new("reg");
        command.args(["query", &format!(r"{hive}\{LAYERS}")]);
        let Ok(output) = crate::system::quiet(command).output() else { continue };
        for (name, data) in parse_layers(&String::from_utf8_lossy(&output.stdout)) {
            if same_path(Path::new(&name), exe) {
                found.push((hive, name, data));
            }
        }
    }
    found
}

/// `reg query`'s lines: `    <name>    REG_SZ    <data>`.
fn parse_layers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, data) = line.trim().split_once("    REG_SZ    ")?;
            Some((name.trim().to_owned(), data.trim().to_owned()))
        })
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |path: &Path| path.to_string_lossy().replace('/', "\\").trim_start_matches(r"\\?\").to_lowercase();
    norm(a) == norm(b)
}

/// The data with `RUNASADMIN` taken out; `None` when nothing but the `~` marker stays.
fn without_admin(data: &str) -> Option<String> {
    let kept: Vec<&str> = data.split_whitespace().filter(|flag| !flag.eq_ignore_ascii_case("RUNASADMIN")).collect();
    (!kept.iter().all(|flag| *flag == "~")).then(|| kept.join(" "))
}

#[cfg(windows)]
fn run_as_admin(exe: &Path) -> Vec<Check> {
    marks(exe)
        .into_iter()
        .filter(|(_, _, data)| data.split_whitespace().any(|flag| flag.eq_ignore_ascii_case("RUNASADMIN")))
        .map(|(hive, _, _)| {
            let all_users = hive == "HKLM";
            Check::new(
                Level::Bad,
                if all_users { "Sol JK is set to run as administrator for all users" } else { "Sol JK is set to run as administrator" },
                "Windows then refuses to start it from the launcher (error 740). The mark is the \"Run this program as an administrator\" box in sjk.exe's Properties > Compatibility.",
            )
            .with(Fix::ClearRunAsAdmin { exe: exe.to_owned(), all_users })
        })
        .collect()
}

#[cfg(not(windows))]
fn run_as_admin(_exe: &Path) -> Vec<Check> {
    Vec::new()
}

/// Take "Run as administrator" off `exe`, keeping its other compatibility marks. The
/// all-users one needs Windows' permission prompt (the player sees it).
#[cfg(windows)]
pub fn clear_run_as_admin(exe: &Path, all_users: bool) -> Result<(), String> {
    let hive = if all_users { "HKLM" } else { "HKCU" };
    let Some((_, name, data)) = marks(exe).into_iter().find(|(found, _, _)| *found == hive) else {
        return Ok(());
    };
    let key = format!(r"{hive}\{LAYERS}");
    let args: Vec<String> = match without_admin(&data) {
        None => vec!["delete".into(), key, "/v".into(), name, "/f".into()],
        Some(kept) => vec!["add".into(), key, "/v".into(), name, "/t".into(), "REG_SZ".into(), "/d".into(), kept, "/f".into()],
    };
    let status = if all_users {
        // reg through PowerShell's Start-Process -Verb RunAs: Windows asks the player.
        let quoted: Vec<String> = args.iter().map(|arg| format!("'\"{}\"'", arg.replace('\'', "''"))).collect();
        let script = format!(
            "$p = Start-Process reg -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList @({}); exit $p.ExitCode",
            quoted.join(",")
        );
        let mut command = std::process::Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        crate::system::quiet(command).status()
    } else {
        let mut command = std::process::Command::new("reg");
        command.args(&args);
        crate::system::quiet(command).status()
    };
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) if all_users => Err("Windows did not allow the change (the permission prompt was refused?)".to_owned()),
        Ok(status) => Err(format!("reg failed ({status})")),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(windows))]
pub fn clear_run_as_admin(_exe: &Path, _all_users: bool) -> Result<(), String> {
    Ok(())
}

/// Put SJK's settings aside (`config.cfg` renamed, nothing deleted): the next start
/// begins with SJK's defaults. Answers the name it was kept under.
pub fn reset_settings(game_data: &Path) -> Result<String, String> {
    let sjk = game_data.join("SJK");
    let config = sjk.join("config.cfg");
    if !config.exists() {
        return Err("SJK has no settings saved yet".to_owned());
    }
    let kept = format!("config-before-reset-{}.cfg", unix_now());
    std::fs::rename(&config, sjk.join(&kept)).map_err(|error| error.to_string())?;
    Ok(kept)
}

// Send my logs.

/// The logs as they would go: what the player sees before sending, and the upload.
#[derive(Clone, Debug)]
pub struct Package {
    pub upload: LogUpload,
}

/// Newest bytes of each part: the whole fits the hub's 2 000 000 characters.
const PARTS: [(&str, usize); 5] = [
    ("the launcher's log of the last run (sjk.log)", 400_000),
    ("the launcher's log of the run before (sjk-previous.log)", 150_000),
    ("SJK's session log (logs/session.log)", 600_000),
    ("SJK's previous session log (logs/previous-session.log)", 200_000),
    ("SJK's last crash report (crashes/last-crash.txt)", 40_000),
];

/// Gather what Send my logs sends for `game_data`: a header (launcher, build, system,
/// graphics, Doctor's findings), then the launcher's and SJK's logs, newest lines, chat
/// left out of the launcher's (SJK's own are written without it), the player's folder
/// as `%USERPROFILE%`.
pub fn package(game_data: &Path, checks: &[Check], graphics: Graphics, build: &str) -> Package {
    let sjk = game_data.join("SJK");
    let launcher_logs = crate::game_log::path(Game::Sjk).and_then(|path| path.parent().map(Path::to_owned)).unwrap_or_default();
    let files = [
        launcher_logs.join("sjk.log"),
        launcher_logs.join("sjk-previous.log"),
        sjk.join("logs").join("session.log"),
        sjk.join("logs").join("previous-session.log"),
        sjk.join("crashes").join("last-crash.txt"),
    ];
    let home = crate::game_log::home();
    let last = read(&files[0]);
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let gpu = gpu(&last).unwrap_or_default();
    let mut text = format!(
        "SJK Launcher {} Doctor, sent unasked\nSol JK build: {}\nSystem: {os}\nGraphics card: {}\nGraphics API: {}\nGame folder: {}\n\nDoctor:\n",
        env!("CARGO_PKG_VERSION"),
        if build.is_empty() { "unknown" } else { build },
        if gpu.is_empty() { "unknown" } else { &gpu },
        graphics.label(),
        shown(game_data),
    );
    for check in checks {
        let mark = match check.level {
            Level::Good => "ok  ",
            Level::Warn => "warn",
            Level::Bad => "BAD ",
        };
        text.push_str(&format!("  [{mark}] {}: {}\n", check.title, check.detail.replace('\n', " ")));
    }
    for (index, ((what, max), file)) in PARTS.iter().zip(&files).enumerate() {
        let body = if index == 0 { last.clone() } else { read(file) };
        if body.trim().is_empty() {
            continue;
        }
        let body = newest(&body, *max);
        // The launcher's logs are the game's raw output, chat included: left out.
        let body = if index < 2 { crate::game_log::without_chat(&body, usize::MAX) } else { body };
        text.push_str(&format!("\n--- {what} ---\n{}\n", without_home(&body, &home)));
    }
    Package { upload: LogUpload { logs: text, build: build.to_owned(), os, gpu }.fitted() }
}

/// The newest `max` bytes of `text`, from a line's start.
fn newest(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut at = text.len() - max;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    let rest = &text[at..];
    let rest = rest.find('\n').map_or(rest, |end| &rest[end + 1..]);
    format!("[earlier lines left out]\n{rest}")
}

/// Send `package` with the player's `note`, signed with SJK's identity in `game_data`
/// (made and registered here when SJK never got to it). Answers what to tell the
/// player, or why it did not go.
pub fn send(game_data: &Path, package: &Package, note: &str) -> Result<String, String> {
    let sjk = game_data.join("SJK");
    let config = std::fs::read_to_string(sjk.join("config.cfg")).unwrap_or_default();
    let setting = |name: &str| crate::chat::setting(&config, name);
    if setting("cl_identity").as_deref() == Some("0") {
        return Err("Your SJK identity is off in SJK's settings (cl_identity 0), so nothing can be sent: copy the log and pass it on instead".to_owned());
    }
    let hub_url = crate::chat::hub_url(&sjk);
    if hub_url.is_empty() || !sjk_identity::valid_base_url(&hub_url) {
        return Err(format!("SJK's hub address is not one the launcher can use: {hub_url:?}"));
    }
    std::fs::create_dir_all(&sjk).map_err(|error| format!("cannot write into {}: {error}", shown(&sjk)))?;
    let identity = Identity::load_or_create(&sjk.join("identity.key")).map_err(|error| format!("SJK's identity: {error}"))?;
    let mut hub = HttpHub::new(&hub_url, crate::chat::AGENT).map_err(|error| words(&error))?;
    let name = setting("name").filter(|name| !name.trim().is_empty());
    hub.register(&identity, name.as_deref()).map_err(|error| words(&error))?;
    match hub.send_own_logs(&identity, &package.upload, note) {
        Ok(id) => Ok(format!("Sent: SJK's team has your logs (number {id})")),
        Err(HubError::Rejected { code, .. }) if code == "log_repeat" => Ok("SJK's team already has these logs".to_owned()),
        Err(error) => Err(words(&error)),
    }
}

/// A hub error in the player's words.
fn words(error: &HubError) -> String {
    match error {
        HubError::Network(why) => format!("Cannot reach SJK's hub ({why}): check your connection and try again"),
        HubError::Rejected { code, message, .. } => match code.as_str() {
            "log_quota" => "You have sent your logs several times already: try again in an hour, or tomorrow".to_owned(),
            "logs_full" | "rate_limited" => "SJK's hub takes no more logs right now: try again later".to_owned(),
            _ => format!("SJK's hub refused them: {message}"),
        },
        other => other.to_string(),
    }
}

/// Seconds before Send my logs may go again, after a send at `sent` (unix seconds).
pub fn wait_left(sent: u64) -> u64 {
    (sent + SEND_EVERY).saturating_sub(unix_now())
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_ending_at_the_main_menu_is_a_freeze_before_the_first_picture() {
        let log = "[+00019.633 ms] graphics card: NVIDIA GeForce GTX 1650, 3938 MiB video memory, budget 3347 MiB\n[+01393.602 ms] client state: main menu\n";
        let check = last_run(log);
        assert_eq!(check.level, Level::Warn);
        assert!(check.title.contains("before drawing"), "{check:?}");
        assert!(check.detail.contains("NVIDIA GeForce GTX 1650."), "{check:?}");
        let fine = format!("{log}[+02073.076 ms] world/model filtering: mode=GL_LINEAR_MIPMAP_LINEAR\n");
        assert_eq!(last_run(&fine).level, Level::Good);
        assert_eq!(last_run("").level, Level::Good);
        assert_eq!(last_run("[+1 ms] loading\n[+2 ms] still loading").level, Level::Warn);
    }

    #[test]
    fn graphics_errors_and_memory_are_named() {
        let oom = last_run("[+1 ms] client state: main menu\nwgpu error: Out of Memory (In Device::create_buffer, label = 'skin vertices')");
        assert_eq!((oom.level, oom.title.as_str()), (Level::Bad, "The last run ran out of video memory"));
        let lost = last_run("thread 'main' panicked at device.rs:72:18:\nFailed to get mapped range: MapRangeError");
        assert_eq!(lost.level, Level::Bad);
        assert!(lost.title.contains("graphics error"));
    }

    #[test]
    fn the_gpu_is_read_from_the_log() {
        assert_eq!(gpu("x\n[+1 ms] graphics card: AMD Radeon RX 6600, 8176 MiB video memory\n").as_deref(), Some("AMD Radeon RX 6600"));
        assert_eq!(gpu("nothing"), None);
    }

    #[test]
    fn run_as_admin_marks_are_read_and_taken_out_alone() {
        let query = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers\r\n    E:\\JKA SOL\\GameData\\sjk.exe    REG_SZ    ~ RUNASADMIN\r\n    C:\\Other\\a.exe    REG_SZ    ~ WIN8RTM\r\n";
        let marks = parse_layers(query);
        assert_eq!(marks[0], (r"E:\JKA SOL\GameData\sjk.exe".to_owned(), "~ RUNASADMIN".to_owned()));
        assert_eq!(marks.len(), 2);
        assert!(same_path(Path::new(r"e:\jka sol\gamedata\SJK.EXE"), Path::new("E:/JKA SOL/GameData/sjk.exe")));
        assert_eq!(without_admin("~ RUNASADMIN"), None);
        assert_eq!(without_admin("~ RUNASADMIN HIGHDPIAWARE").as_deref(), Some("~ HIGHDPIAWARE"));
    }

    #[test]
    fn the_package_holds_the_findings_and_the_newest_lines() {
        assert_eq!(newest("aaaa\nbbbb\ncccc\n", 7), "[earlier lines left out]\ncccc\n");
        assert_eq!(newest("short", 100), "short");
        let dir = std::env::temp_dir().join(format!("sjk-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("SJK").join("logs")).unwrap();
        std::fs::write(dir.join("SJK").join("logs").join("session.log"), "[+1 ms] client state: main menu\n").unwrap();
        let checks = [Check::new(Level::Bad, "Sol JK is set to run as administrator", "error 740")];
        let package = package(&dir, &checks, Graphics::Dx12, "2026.1107.1");
        let logs = &package.upload.logs;
        assert!(logs.contains("Sol JK build: 2026.1107.1"), "{logs}");
        assert!(logs.contains("Graphics API: DirectX 12"));
        assert!(logs.contains("[BAD ] Sol JK is set to run as administrator: error 740"));
        assert!(logs.contains("--- SJK's session log (logs/session.log) ---\n[+1 ms] client state: main menu"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
