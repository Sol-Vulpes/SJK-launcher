//! Developer mode (Settings): build Sol JK from a source checkout and start that
//! build, as SJK's developers do with their own build scripts.
//!
//! The source folder is a clone of SJK (it holds `crates/sjk-viewer`). Its own
//! checked-out branch builds in place; any other branch builds in a throwaway
//! worktree beside it, `<source>-wt/test`, checked out detached and forced, so it
//! never touches the folder the developer works in. Each folder keeps its own
//! `target` (a `CARGO_TARGET_DIR` set outside is left out), so builds of different
//! branches never mix artifacts.
//!
//! Cargo's JSON messages say where the client landed (whatever `CARGO_TARGET_DIR`
//! says), and give the progress shown under the button; everything cargo prints
//! goes into `logs/sjk-build.log` in the launcher's folder.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The packages a build makes: the client, and the server beside it (as releases ship).
const PACKAGES: [&str; 2] = ["sjk-viewer", "sjk-dedicated"];
/// The client's program name in `sjk-viewer`'s `[[bin]]`.
const CLIENT_BIN: &str = "sjk";

/// A folder is SJK's source when it holds the client's crate.
pub fn is_source(path: &Path) -> bool {
    path.join("crates").join("sjk-viewer").join("Cargo.toml").is_file()
}

/// Find a source checkout: `SJK_SOURCE`, then beside the launcher's own source
/// (a launcher built from its repository), then the usual clone places.
pub fn detect() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("SJK_SOURCE") {
        places.push(path.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        for folder in exe.ancestors().skip(1) {
            places.push(folder.to_owned());
            if let Some(parent) = folder.parent() {
                places.push(parent.join("SJK"));
            }
        }
    }
    if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_owned()) {
        for under in ["Git", "git", "source/repos", "Projects", "projects", "Documents/GitHub", "dev", "src", "code", ""] {
            places.push(home.join(under).join("SJK"));
        }
    }
    #[cfg(windows)]
    for drive in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", drive as char));
        for under in ["Git", "git", "Projects", "dev", "src", "code", ""] {
            places.push(root.join(under).join("SJK"));
        }
    }
    places.into_iter().find(|path| is_source(path)).map(|path| path.components().collect())
}

use crate::system::quiet;

fn git(dir: &Path) -> Command {
    let mut command = quiet(Command::new("git"));
    command.arg("-C").arg(dir);
    command
}

/// `git` in `dir`, its output when it worked.
fn git_out(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(dir).args(args).output().map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// The branch the source folder has checked out, `None` when detached.
pub fn current_branch(source: &Path) -> Option<String> {
    git_out(source, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().filter(|name| !name.is_empty())
}

/// The source's branches, newest commit first: local ones, then `origin`'s that
/// have no local branch.
pub fn branches(source: &Path) -> Vec<String> {
    let list = |pattern: &str| {
        git_out(source, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)", pattern])
            .map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let mut names = list("refs/heads");
    for remote in list("refs/remotes/origin") {
        let Some(name) = remote.strip_prefix("origin/") else { continue };
        if name != "HEAD" && !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The branch list, read on its own thread (a fetch can take a while).
#[derive(Clone, Default)]
pub struct Branches {
    list: Arc<Mutex<Vec<String>>>,
    busy: Arc<AtomicBool>,
}

impl Branches {
    pub fn get(&self) -> Vec<String> {
        self.list.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Read the branches again, after fetching `origin` when `fetch`.
    pub fn refresh(&self, source: PathBuf, fetch: bool, repaint: impl FnOnce() + Send + 'static) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            if fetch {
                let _ = git(&source).args(["fetch", "-q", "--prune", "origin"]).status();
            }
            *this.list.lock().unwrap_or_else(|error| error.into_inner()) = branches(&source);
            this.busy.store(false, Ordering::Release);
            repaint();
        });
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// Checking the branch out.
    Preparing(String),
    /// Cargo runs: what it did last, and how many crates are done.
    Building { line: String, done: usize },
    /// Built and started.
    Started(String),
    Failed(String),
}

impl Phase {
    pub fn running(&self) -> bool {
        matches!(self, Self::Preparing(_) | Self::Building { .. })
    }
}

/// What a finished build made: the client, and the folder to start it from.
pub struct Built {
    pub client: PathBuf,
    pub folder: PathBuf,
    pub label: String,
}

/// One build on its own thread; the UI reads [`Build::phase`].
#[derive(Clone, Default)]
pub struct Build {
    phase: Arc<Mutex<Phase>>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl Build {
    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    pub fn running(&self) -> bool {
        self.phase().running()
    }

    pub fn set(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(|error| error.into_inner()) = phase;
    }

    pub fn clear(&self) {
        if !self.running() {
            self.set(Phase::Idle);
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(child) = self.child.lock().unwrap_or_else(|error| error.into_inner()).as_mut() {
            let _ = child.kill();
        }
    }

    /// Build `branch` (`None`: whatever `source` has checked out); `done` gets what
    /// was built, on the worker thread, and says whether the game started.
    pub fn start(
        &self,
        source: PathBuf,
        branch: Option<String>,
        repaint: impl Fn() + Send + 'static,
        done: impl FnOnce(Built) -> Result<String, String> + Send + 'static,
    ) {
        if self.running() {
            return;
        }
        self.cancelled.store(false, Ordering::Release);
        self.set(Phase::Preparing("Preparing…".into()));
        let this = self.clone();
        std::thread::spawn(move || {
            let result = this.run(&source, branch, &repaint).and_then(done);
            this.set(match result {
                Ok(text) => Phase::Started(text),
                Err(_) if this.cancelled.load(Ordering::Acquire) => Phase::Idle,
                Err(error) => Phase::Failed(error),
            });
            repaint();
        });
    }

    fn run(&self, source: &Path, branch: Option<String>, repaint: &dyn Fn()) -> Result<Built, String> {
        let current = current_branch(source);
        let (folder, label) = match branch {
            Some(branch) if current.as_deref() != Some(branch.as_str()) => {
                self.set(Phase::Preparing(format!("Checking out {branch}…")));
                repaint();
                (checkout(source, &branch)?, branch)
            }
            _ => (source.to_owned(), current.unwrap_or_else(|| "the source folder".into())),
        };
        let commit = git_out(&folder, &["log", "-1", "--format=%h %s"]).unwrap_or_default();
        let client = self.cargo(&folder, &format!("{label}: {commit}"), repaint)?;
        Ok(Built { client, folder, label })
    }

    /// `cargo build --release` in `folder`; the client's path when it worked.
    fn cargo(&self, folder: &Path, heading: &str, repaint: &dyn Fn()) -> Result<PathBuf, String> {
        let mut log = crate::config::folder().and_then(|folder| {
            let folder = folder.join("logs");
            std::fs::create_dir_all(&folder).ok()?;
            std::fs::File::create(folder.join("sjk-build.log")).ok()
        });
        if let Some(log) = log.as_mut() {
            let _ = writeln!(log, "{heading}\n{}\n", folder.display());
        }
        let mut command = quiet(Command::new("cargo"));
        // Each folder keeps its own `target`: one shared between branches mixes their artifacts.
        command.env_remove("CARGO_TARGET_DIR");
        command.current_dir(folder).args(["build", "--release", "--message-format=json-render-diagnostics"]);
        for package in PACKAGES {
            command.args(["-p", package]);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start cargo (is Rust installed?): {error}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self.child.lock().unwrap_or_else(|error| error.into_inner()) = Some(child);

        // Cargo's own lines (Compiling…, warnings, errors) come on stderr.
        let last_error = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = Arc::new(Mutex::new(log));
        let reader = stderr.map(|stderr| {
            let (log, last_error, phase) = (log.clone(), last_error.clone(), self.phase.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Some(log) = log.lock().unwrap_or_else(|error| error.into_inner()).as_mut() {
                        let _ = writeln!(log, "{line}");
                    }
                    let trimmed = line.trim();
                    if trimmed.starts_with("Compiling") || trimmed.starts_with("Finished") {
                        let mut phase = phase.lock().unwrap_or_else(|error| error.into_inner());
                        if let Phase::Building { line, .. } = &mut *phase {
                            *line = trimmed.to_owned();
                        }
                    }
                    if trimmed.starts_with("error") {
                        let mut errors = last_error.lock().unwrap_or_else(|error| error.into_inner());
                        if errors.len() < 3 {
                            errors.push(trimmed.to_owned());
                        }
                    }
                }
            })
        });

        self.set(Phase::Building { line: "Starting cargo…".into(), done: 0 });
        repaint();
        let mut client = None;
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if message["reason"] != "compiler-artifact" {
                    continue;
                }
                if let Phase::Building { done, .. } = &mut *self.phase.lock().unwrap_or_else(|error| error.into_inner()) {
                    *done += 1;
                }
                if message["target"]["name"] == CLIENT_BIN
                    && let Some(path) = message["executable"].as_str()
                {
                    client = Some(PathBuf::from(path));
                }
                repaint();
            }
        }
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        let status = self.child.lock().unwrap_or_else(|error| error.into_inner()).take().map(|mut child| child.wait());
        if self.cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        match status {
            Some(Ok(status)) if status.success() => client.ok_or_else(|| "cargo built no sjk client".into()),
            _ => {
                let errors = last_error.lock().unwrap_or_else(|error| error.into_inner()).join(" · ");
                Err(if errors.is_empty() { "Build failed: see the build log".into() } else { format!("Build failed: {errors}") })
            }
        }
    }
}

/// Check `branch` out in the test worktree beside `source` (local branch first,
/// else `origin`'s), detached and forced: that folder is only for building.
fn checkout(source: &Path, branch: &str) -> Result<PathBuf, String> {
    let reference = if git_out(source, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")]).is_ok() {
        branch.to_owned()
    } else {
        let remote = format!("origin/{branch}");
        git_out(source, &["rev-parse", "-q", "--verify", &format!("{remote}^{{commit}}")])
            .map_err(|_| format!("no branch {branch} here or on origin"))?;
        remote
    };
    let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "SJK".into());
    let parent = source.parent().ok_or("the source folder has no parent folder")?;
    let test = parent.join(format!("{name}-wt")).join("test");
    if !test.join(".git").exists() {
        git_out(source, &["worktree", "add", "-q", "--detach", &test.to_string_lossy(), &reference])?;
    }
    git_out(&test, &["checkout", "-q", "--force", "--detach", &reference])?;
    Ok(test)
}

