//! One launcher at a time. The first one listens on a local port (one per launcher,
//! so the SJK and JoF launchers live side by side); a second start, from the
//! desktop, autostart or a double click, finds it there, asks it to show its window
//! (not when started with `--minimized`) and closes. If something else holds the
//! port, the answer is wrong and the launcher starts as usual.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const SHOW: &str = "show";
const QUIET: &str = "quiet";
/// How long a second start waits for the first one's answer.
const WAIT: Duration = Duration::from_secs(3);

/// What the window does when another start asks: set once the window exists.
static SHOW_WINDOW: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// Asked to show before the window existed.
static SHOW_LATER: AtomicBool = AtomicBool::new(false);

fn address() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, crate::brand::INSTANCE_PORT))
}

/// Whether this start goes on: true for the first launcher, false when one already
/// runs (it was asked to show itself unless `minimized`).
pub fn claim(minimized: bool) -> bool {
    // Started by an update (`self_update.rs`): the old launcher is quitting, wait for its port.
    if std::env::args().any(|arg| arg == crate::self_update::UPDATED) {
        for _ in 0..40 {
            if let Ok(listener) = TcpListener::bind(address()) {
                let _ = std::thread::Builder::new().name("launcher-instance".into()).spawn(move || serve(listener));
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    match TcpListener::bind(address()) {
        Ok(listener) => {
            let _ = std::thread::Builder::new().name("launcher-instance".into()).spawn(move || serve(listener));
            true
        }
        Err(_) => !ask(if minimized { QUIET } else { SHOW }),
    }
}

/// Ask the running launcher; true when it answered as this launcher.
fn ask(request: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address(), WAIT) else { return false };
    let _ = stream.set_read_timeout(Some(WAIT));
    if writeln!(stream, "{request}").is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim_end() == crate::brand::NAME
}

fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(WAIT));
        let mut request = String::new();
        if BufReader::new(&stream).read_line(&mut request).is_err() {
            continue;
        }
        let _ = writeln!(&stream, "{}", crate::brand::NAME);
        if request.trim_end() == SHOW {
            match SHOW_WINDOW.get() {
                Some(show) => show(),
                None => SHOW_LATER.store(true, Ordering::Relaxed),
            }
        }
    }
}

/// Once the window exists: what showing it means. A start that asked before runs it now.
pub fn on_show(show: impl Fn() + Send + Sync + 'static) {
    if SHOW_WINDOW.set(Box::new(show)).is_ok()
        && SHOW_LATER.swap(false, Ordering::Relaxed)
        && let Some(show) = SHOW_WINDOW.get()
    {
        show();
    }
}
