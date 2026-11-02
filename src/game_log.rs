//! What a game started from the launcher prints, and what the launcher does when
//! it goes down.
//!
//! The game's output goes straight into a file, `logs/<game>.log` in the launcher's
//! folder (the run before stays as `<game>-previous.log`), so it keeps writing even
//! if the launcher closes. When the game ends with an error code (a crash, or a
//! start that failed before SJK's own crash reports were up), the launcher shows
//! what happened with the last lines, and for Sol JK makes sure SJK's team gets a
//! crash report now, not at a next start that may never work: it writes one into
//! SJK's `crashes/` folder when the game left none (from the game's own
//! `running-<pid>.json` marker when there is one, so it says where the player was),
//! then sends what waits there, signed with the player's SJK identity. SJK's own
//! settings decide: nothing is sent with `cl_crashReports 0` or `cl_identity 0`.

use crate::games::Game;
use sjk_identity::crash::{self, CrashReport, Kind};
use sjk_identity::{HttpHub, Identity};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lines of output the crash window shows.
const SHOWN_LINES: usize = 40;
/// Lines of output a crash report carries.
const REPORT_LINES: usize = 200;
/// A game that went down before this ran had not started.
pub const STARTING: Duration = Duration::from_secs(20);

fn folder() -> Option<PathBuf> {
    crate::config::folder().map(|folder| folder.join("logs"))
}

fn short_name(game: Game) -> &'static str {
    match game {
        Game::Sjk => "sjk",
        Game::Jof => "jof",
    }
}

/// A fresh log for `game`'s run; the last one becomes `<game>-previous.log`.
pub fn create(game: Game) -> Option<(File, PathBuf)> {
    let folder = folder()?;
    std::fs::create_dir_all(&folder).ok()?;
    let path = folder.join(format!("{}.log", short_name(game)));
    let _ = std::fs::rename(&path, folder.join(format!("{}-previous.log", short_name(game))));
    File::create(&path).ok().map(|file| (file, path))
}

/// The last `lines` lines of the file at `path`, without the player's home folder.
pub fn tail(path: &Path, lines: usize) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let kept = all[all.len().saturating_sub(lines)..].join("\n");
    crash::without_home(&kept, &home())
}

fn home() -> String {
    std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// What a bad exit status means, in the player's words; `None` for a normal exit.
pub fn bad_exit(status: ExitStatus) -> Option<String> {
    if status.success() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            let what = match signal {
                4 => "an illegal instruction",
                6 => "an abort",
                7 | 10 => "a bus error",
                8 => "a maths error",
                9 => "killed",
                11 => "a memory error (segmentation fault)",
                15 => "asked to stop",
                _ => "a signal",
            };
            return Some(format!("signal {signal}: {what}"));
        }
    }
    let code = status.code()?;
    Some(describe(code))
}

/// An exit code with what it says, where it says something (Windows' crash codes).
pub fn describe(code: i32) -> String {
    let what = match code as u32 {
        0xC000_0005 => Some("a memory error (access violation)"),
        0xC000_001D => Some("an instruction this processor lacks"),
        0xC000_00FD => Some("a stack overflow"),
        0xC000_0135 => Some("a DLL it needs is missing"),
        0xC000_0139 | 0xC000_007B => Some("a DLL it needs does not fit (wrong version or 32/64-bit)"),
        0xC000_0142 => Some("a DLL failed to start"),
        0xC000_0409 => Some("it stopped itself on an error it could not recover from"),
        0xC000_0374 => Some("its memory got corrupted (heap corruption)"),
        0xC000_0017 | 0xC000_012D => Some("the computer ran out of memory"),
        0x4001_0004 | 0xC000_013A => Some("closed from outside (Task Manager, Ctrl+C or logging off)"),
        0xE06D_7363 => Some("an error in C++ code"),
        _ => None,
    };
    match what {
        Some(what) => format!("exit code 0x{:08X}: {what}", code as u32),
        None if !(0..=0xFFFF).contains(&code) => format!("exit code 0x{:08X}", code as u32),
        None => format!("exit code {code}"),
    }
}

/// What became of the crash report for SJK's team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// Not a game that sends reports (JoF EternalJK).
    None,
    /// The player turned crash reports (or the identity) off in SJK.
    Off,
    /// The hub took it, with its number.
    Sent(Vec<i64>),
    /// Kept in SJK's folder: SJK or the launcher sends it later.
    Waiting(String),
}

/// A game that went down: what the crash window shows.
#[derive(Clone, Debug)]
pub struct Exit {
    pub game: Game,
    pub what: String,
    pub ran: Duration,
    pub log: PathBuf,
    pub tail: String,
    pub report: Report,
    /// A developer's build (`dev.rs`): playing again builds and starts it again.
    pub dev: bool,
}

/// After `game` (process `pid`) went down with `what`: the crash report for SJK's
/// team, written and sent as SJK would, from SJK's settings folder `sjk`.
pub fn report_sjk(sjk: &Path, pid: u32, what: &str, ran: Duration, output: &str, build: &str) -> Report {
    let config = std::fs::read_to_string(sjk.join("config.cfg")).unwrap_or_default();
    let setting = |name: &str| crate::chat::setting(&config, name);
    let dir = sjk.join("crashes");
    if setting("cl_crashReports").as_deref() == Some("0") || setting("cl_identity").as_deref() == Some("0") {
        return Report::Off;
    }
    if !crash::written_by(&dir, pid) {
        let report = launcher_report(&dir, pid, what, ran, output, build);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("last-crash.txt"), report.readable());
        if let Err(error) = crash::write_pending(&dir, &report) {
            return Report::Waiting(format!("it could not be kept: {error}"));
        }
    }
    let Ok(text) = std::fs::read_to_string(sjk.join("identity.key")) else {
        return Report::Waiting("SJK has no identity yet; it goes once SJK has started".to_owned());
    };
    let Ok(identity) = Identity::parse(&text) else {
        return Report::Waiting("SJK's identity.key cannot be read".to_owned());
    };
    let hub_url = crate::chat::hub_url(sjk);
    let Ok(mut hub) = HttpHub::new(&hub_url, crate::chat::AGENT) else {
        return Report::Waiting(format!("the hub's address is not one the launcher can use: {hub_url}"));
    };
    let name = setting("name").unwrap_or_default();
    let round = crash::send_pending(&mut hub, &identity, &dir, &name, unix_now());
    match round.problem {
        Some(problem) if round.sent.is_empty() => Report::Waiting(problem),
        _ => Report::Sent(round.sent),
    }
}

/// The report the launcher writes for a game that left none: from the game's marker
/// (where it was, its graphics adapter, its last console lines) when it left one,
/// with the exit and the output the launcher caught.
fn launcher_report(dir: &Path, pid: u32, what: &str, ran: Duration, output: &str, build: &str) -> CrashReport {
    let marker = dir.join(format!("running-{pid}.json"));
    let mut report = std::fs::read(&marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CrashReport>(&bytes).ok())
        .unwrap_or_default();
    // SJK turns a marker into a report at its next start unless one exists: this is it.
    let _ = std::fs::remove_file(&marker);
    report.kind = Kind::Unclean;
    report.message = format!("SJK closed with {what}, caught by the launcher after {} s", ran.as_secs());
    if report.build.is_empty() {
        build.clone_into(&mut report.build);
    }
    if report.os.is_empty() {
        report.os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    }
    if !output.is_empty() {
        // The log keeps its end when cut: the game's output, newest, goes last.
        let console = std::mem::take(&mut report.log);
        report.log = if console.is_empty() { output.to_owned() } else { format!("{console}\n--- output ---\n{output}") };
    }
    report.uptime = ran.as_secs();
    report.crashed = unix_now();
    report.pid = pid;
    report
}

/// Everything known after `game` went down.
pub fn exit(game: Game, what: String, ran: Duration, log: PathBuf, report: Report) -> Exit {
    let tail = tail(&log, SHOWN_LINES);
    Exit { game, what, ran, log, tail, report, dev: false }
}

/// The output a report carries: its last lines without what other players said.
/// SJK's own reports leave chat out, so the launcher's do too: every line the
/// server sent (chat, and the prints JA+ uses for private messages and amsay) and
/// any line about chat, whispers or tells.
pub fn report_output(log: &Path) -> String {
    without_chat(&tail(log, REPORT_LINES * 4), REPORT_LINES)
}

fn without_chat(text: &str, lines: usize) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            !(lower.contains("] server ") || ["chat", "whisper", "tell", "say"].iter().any(|word| lower.contains(word)))
        })
        .collect();
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_codes_are_named() {
        assert_eq!(describe(0xC000_0005_u32 as i32), "exit code 0xC0000005: a memory error (access violation)");
        assert_eq!(describe(1), "exit code 1");
        assert_eq!(describe(0xC0DE_0001_u32 as i32), "exit code 0xC0DE0001");
    }

    #[test]
    fn a_marker_becomes_the_report_and_goes() {
        let dir = std::env::temp_dir().join(format!("sjk-launcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = CrashReport { map: "maps/ffa_bespin.bsp".into(), log: "console line".into(), build: "2026.1102.1".into(), ..Default::default() };
        std::fs::write(dir.join("running-42.json"), serde_json::to_vec(&marker).unwrap()).unwrap();
        let report = launcher_report(&dir, 42, "exit code 1", Duration::from_secs(3), "printed line", "other");
        assert_eq!(report.kind, Kind::Unclean);
        assert_eq!(report.map, "maps/ffa_bespin.bsp");
        assert_eq!(report.build, "2026.1102.1");
        assert_eq!(report.log, "console line\n--- output ---\nprinted line");
        assert_eq!(report.pid, 42);
        assert!(!dir.join("running-42.json").exists());
        let fresh = launcher_report(&dir, 7, "exit code 1", Duration::from_secs(3), "", "2026.1028.1");
        assert_eq!(fresh.build, "2026.1028.1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_an_identity_the_report_waits_and_with_reports_off_none_is_kept() {
        let sjk = std::env::temp_dir().join(format!("sjk-launcher-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sjk);
        std::fs::create_dir_all(&sjk).unwrap();
        let report = report_sjk(&sjk, 9, "exit code 1", Duration::from_secs(2), "line", "2026.1102.1");
        assert!(matches!(report, Report::Waiting(_)), "{report:?}");
        assert_eq!(crash::pending(&sjk.join("crashes")).len(), 1);
        assert!(sjk.join("crashes").join("last-crash.txt").exists());
        // Asked again for the same process: no second report.
        report_sjk(&sjk, 9, "exit code 1", Duration::from_secs(2), "line", "2026.1102.1");
        assert_eq!(crash::pending(&sjk.join("crashes")).len(), 1);
        std::fs::write(sjk.join("config.cfg"), "seta cl_crashReports \"0\"\n").unwrap();
        assert_eq!(report_sjk(&sjk, 10, "exit code 1", Duration::from_secs(2), "line", ""), Report::Off);
        assert_eq!(crash::pending(&sjk.join("crashes")).len(), 1);
        let _ = std::fs::remove_dir_all(&sjk);
    }

    #[test]
    fn reports_leave_chat_out() {
        let output = "[+1.0 ms] loaded map\n[+2.0 ms] server Chat: ^7Kyle: ^2hi\n[+3.0 ms] server Print: [private] secret\n[+4.0 ms] e2e whisper from x\n[+5.0 ms] wgpu error";
        assert_eq!(without_chat(output, 10), "[+1.0 ms] loaded map\n[+5.0 ms] wgpu error");
        assert_eq!(without_chat(output, 1), "[+5.0 ms] wgpu error");
    }

    #[test]
    fn the_tail_keeps_the_last_lines() {
        let path = std::env::temp_dir().join(format!("sjk-launcher-tail-{}.log", std::process::id()));
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(tail(&path, 2), "c\nd");
        let _ = std::fs::remove_file(&path);
    }
}
