//! Sith cloud: serves a folder of PK3s as a content repository in JoF's
//! manifest format, so the launcher (and SJK's `sjk-jof-content`) can read it.
//!
//! ```text
//! <root>/
//!   groups.json            optional: names, descriptions and types of the groups
//!   repository/
//!     maps/  *.pk3         one folder per group
//!     skins/ *.pk3
//!   hub-model-sync.pub     optional: the SJK hub's model sync key (or --hub-key)
//!   models/                model sync's uploads, see models.rs
//! ```
//!
//! - `GET /manifest.json`: `{"data":{"assetgroups":[...]}}`, every PK3 with its
//!   size and MD5. Hashes are cached in `<root>/.sith-cloud-md5.json` by size
//!   and modification time, so a new or changed file is the only one hashed again.
//! - `GET /repository/<group>/<file>`: the file, with `Range` for resumed downloads.
//!
//! - `/models/...`: model sync, players' synced models ([`models`]).
//!
//! Drop files in with scp; the next manifest request (at most every 10 s) sees them.

mod models;
mod ticket;

use md5::{Digest, Md5};
use models::{HostPack, Models};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const CACHE: &str = ".sith-cloud-md5.json";
const RESCAN: Duration = Duration::from_secs(10);
/// The hub's model sync public key, when `--hub-key` does not give it.
const HUB_KEY_FILE: &str = "hub-model-sync.pub";

/// One group's settings in `groups.json`, keyed by folder.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct GroupSettings {
    name: Option<String>,
    description: String,
    /// `REQUIRED`, `RECOMMENDED` (the default) or `OPTIONAL`.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Files' descriptions, by file name.
    files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Hashed {
    size: u64,
    modified: u64,
    md5: String,
    /// The `models/players/<name>/` folders inside, lowercased (read once per MD5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    models: Option<Vec<String>>,
}

struct Repository {
    root: PathBuf,
    cache: BTreeMap<String, Hashed>,
    manifest: Vec<u8>,
    /// Model names the hosts' own PK3s hold, and where (model sync never overwrites them).
    packs: BTreeMap<String, HostPack>,
    scanned: Option<Instant>,
}

impl Repository {
    fn files(&self) -> PathBuf {
        self.root.join("repository")
    }

    /// The manifest, scanning the folder again when the last scan is old.
    fn manifest(&mut self) -> Vec<u8> {
        self.rescan_if_old();
        self.manifest.clone()
    }

    fn packs(&mut self) -> BTreeMap<String, HostPack> {
        self.rescan_if_old();
        self.packs.clone()
    }

    fn rescan_if_old(&mut self) {
        if self.scanned.is_none_or(|at| at.elapsed() > RESCAN) {
            self.scan();
        }
    }

    fn scan(&mut self) {
        let settings: BTreeMap<String, GroupSettings> = std::fs::read(self.root.join("groups.json"))
            .ok()
            .and_then(|bytes| match serde_json::from_slice(&bytes) {
                Ok(settings) => Some(settings),
                Err(error) => {
                    eprintln!("groups.json: {error}");
                    None
                }
            })
            .unwrap_or_default();
        let mut groups = Vec::new();
        let mut seen = BTreeMap::new();
        let mut packs = BTreeMap::new();
        for folder in sorted_entries(&self.files()).into_iter().filter(|path| path.is_dir()) {
            let Some(folder_name) = plain(&folder) else { continue };
            let group = settings.get(&folder_name).cloned().unwrap_or_default();
            let mut assets = Vec::new();
            for file in sorted_entries(&folder) {
                let Some(name) = plain(&file) else { continue };
                if !file.is_file() || !name.to_ascii_lowercase().ends_with(".pk3") {
                    continue;
                }
                let key = format!("{folder_name}/{name}");
                let Some(mut hashed) = self.hash(&key, &file) else { continue };
                let models = hashed.models.get_or_insert_with(|| pack_models(&key, &file));
                for model in models.iter() {
                    packs
                        .entry(model.clone())
                        .or_insert_with(|| HostPack { group: folder_name.clone(), file: name.clone() });
                }
                assets.push(serde_json::json!({
                    "id": stable_id(&key),
                    "name": name,
                    "size": hashed.size,
                    "hash": hashed.md5,
                    "description": group.files.get(&name).cloned().unwrap_or_default(),
                    "targetfolder": "Base",
                    "localfolder": folder_name,
                }));
                seen.insert(key, hashed);
            }
            groups.push(serde_json::json!({
                "id": stable_id(&folder_name),
                "name": group.name.clone().unwrap_or_else(|| folder_name.clone()),
                "description": group.description,
                "type": group.kind.clone().unwrap_or_else(|| "RECOMMENDED".into()).to_ascii_uppercase(),
                "assets": assets,
            }));
        }
        if seen != self.cache {
            self.cache = seen;
            if let Ok(bytes) = serde_json::to_vec_pretty(&self.cache) {
                let _ = std::fs::write(self.root.join(CACHE), bytes);
            }
        }
        let manifest = serde_json::json!({ "status": "OK", "data": { "assetgroups": groups } });
        self.manifest = serde_json::to_vec(&manifest).unwrap_or_default();
        self.packs = packs;
        self.scanned = Some(Instant::now());
    }

    /// The file's MD5, from the cache while its size and time are unchanged.
    fn hash(&self, key: &str, path: &Path) -> Option<Hashed> {
        let meta = path.metadata().ok()?;
        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
        if let Some(cached) = self.cache.get(key)
            && cached.size == meta.len()
            && cached.modified == modified
        {
            return Some(cached.clone());
        }
        let started = Instant::now();
        let mut file = File::open(path).ok()?;
        let mut hasher = Md5::new();
        std::io::copy(&mut file, &mut hasher).ok()?;
        let md5: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        println!("hashed {key} in {:.1}s", started.elapsed().as_secs_f32());
        Some(Hashed { size: meta.len(), modified, md5, models: None })
    }
}

/// The `models/players/<name>/` folder names in a PK3's central directory,
/// lowercased; none (with a log line) when it cannot be read.
fn pack_models(key: &str, path: &Path) -> Vec<String> {
    let archive = File::open(path).map_err(|error| error.to_string()).and_then(|file| {
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())
    });
    match archive {
        Ok(archive) => models_in(archive.file_names()),
        Err(error) => {
            eprintln!("{key}: cannot list its models: {error}");
            Vec::new()
        }
    }
}

fn models_in<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut models: Vec<String> = paths
        .filter_map(|path| {
            let path = path.replace('\\', "/").to_ascii_lowercase();
            let rest = path.strip_prefix("models/players/")?;
            let (name, _) = rest.split_once('/')?;
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect();
    models.sort();
    models.dedup();
    models
}

fn sorted_entries(folder: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    entries
}

/// The last component's name when it is one the manifest accepts.
fn plain(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_owned();
    plain_name(&name).then_some(name)
}

/// The same rule as `sjk-jof-content`: no separators, no `..`, nothing Windows refuses.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
}

/// An id that stays the same while the folder or file keeps its name, so the
/// launcher's picks survive rescans (31 bits of the name's MD5).
fn stable_id(key: &str) -> u32 {
    let digest = Md5::digest(key.to_ascii_lowercase().as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) & 0x7fff_ffff
}

fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = segment.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("plain ASCII header")
}

/// `bytes=N-` or `bytes=N-M`, clamped to the file.
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => size.checked_sub(1)?,
        end => end.parse::<u64>().ok()?.min(size.checked_sub(1)?),
    };
    (start <= end).then_some((start, end))
}

fn serve_file(request: Request, path: &Path) -> std::io::Result<()> {
    let Ok(mut file) = File::open(path) else {
        return request.respond(Response::from_string("not found").with_status_code(404));
    };
    let size = file.metadata()?.len();
    let range = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Range"))
        .and_then(|header| parse_range(header.value.as_str(), size));
    let head = request.method() == &Method::Head;
    let (status, start, length) = match range {
        Some((start, end)) => (206, start, end - start + 1),
        None => (200, 0, size),
    };
    file.seek(SeekFrom::Start(start))?;
    let mut headers = vec![
        header("Content-Type", "application/octet-stream"),
        header("Accept-Ranges", "bytes"),
    ];
    if status == 206 {
        headers.push(header("Content-Range", &format!("bytes {start}-{}/{size}", start + length - 1)));
    }
    let body: Box<dyn Read + Send> = if head { Box::new(std::io::empty()) } else { Box::new(file.take(length)) };
    let response = Response::new(StatusCode(status), headers, body, Some(length as usize), None);
    request.respond(response)
}

fn json_reply(request: Request, status: u16, body: &serde_json::Value) -> std::io::Result<()> {
    let response = Response::from_data(serde_json::to_vec(body).unwrap_or_default())
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json"));
    request.respond(response)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn handle(request: Request, repository: &Mutex<Repository>, models: &Models) -> std::io::Result<()> {
    let url = request.url().split('?').next().unwrap_or_default().to_owned();
    if let Some(rest) = url.strip_prefix("/models/") {
        return handle_models(request, rest, repository, models);
    }
    if !matches!(request.method(), Method::Get | Method::Head) {
        return request.respond(Response::from_string("method not allowed").with_status_code(405));
    }
    if url == "/manifest.json" {
        let manifest = repository.lock().unwrap_or_else(|error| error.into_inner()).manifest();
        let response = Response::from_data(manifest).with_header(header("Content-Type", "application/json"));
        return request.respond(response);
    }
    if url == "/" {
        return request.respond(Response::from_string("Sith cloud: /manifest.json"));
    }
    let segments: Option<Vec<String>> = url
        .strip_prefix("/repository/")
        .map(|rest| rest.split('/').map(percent_decode).collect::<Option<Vec<_>>>())
        .unwrap_or(None);
    match segments.as_deref() {
        Some([group, name]) if plain_name(group) && plain_name(name) => {
            let path = repository.lock().unwrap_or_else(|error| error.into_inner()).files().join(group).join(name);
            serve_file(request, &path)
        }
        _ => request.respond(Response::from_string("not found").with_status_code(404)),
    }
}

/// `/models/...`, `rest` the path after `/models/`.
fn handle_models(
    mut request: Request,
    rest: &str,
    repository: &Mutex<Repository>,
    models: &Models,
) -> std::io::Result<()> {
    let packs = || repository.lock().unwrap_or_else(|error| error.into_inner()).packs();
    let not_found = serde_json::json!({ "error": "not_found" });
    if rest == "index.json" {
        if !matches!(request.method(), Method::Get | Method::Head) {
            return json_reply(request, 405, &serde_json::json!({ "error": "method_not_allowed" }));
        }
        return json_reply(request, 200, &models.index(&packs()));
    }
    let segments: Option<Vec<String>> = rest.split('/').map(percent_decode).collect();
    let authorization = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str().to_owned());
    let reply = match (request.method().clone(), segments.as_deref()) {
        (Method::Put, Some([name])) => {
            let limit = sjk_model_sync::Limits::DEFAULT.pk3_bytes;
            if models.writable() && request.body_length().is_some_and(|length| length as u64 > limit) {
                return json_reply(request, 413, &serde_json::json!({ "error": "too_large" }));
            }
            let packs = packs();
            models.upload(name, authorization.as_deref(), request.as_reader(), &packs, unix_now())
        }
        (Method::Post, Some([name, action])) if action == "rollback" => {
            models.rollback(name, authorization.as_deref(), unix_now())
        }
        (Method::Get | Method::Head, Some([name, file])) => {
            let path = file
                .strip_suffix(".pk3")
                .and_then(|n| n.parse::<u64>().ok())
                .and_then(|n| models.pk3_path(name, n))
                .filter(|path| path.is_file());
            return match path {
                Some(path) => serve_file(request, &path),
                None => json_reply(request, 404, &not_found),
            };
        }
        _ => return json_reply(request, 404, &not_found),
    };
    json_reply(request, reply.status, &reply.body)
}

const USAGE: &str = "usage: sith-cloud [--root <folder>] [--listen 0.0.0.0:8080] [--hub-key <base64>]
       sith-cloud [--root <folder>] versions <model>
       sith-cloud [--root <folder>] rollback <model> <version>";

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// `dd/mm/yyyy HH:MM` (UTC).
fn date_time(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let seconds = unix % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}/{month:02}/{year} {:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// `versions <name>` and `rollback <name> <n>`, run on the host.
fn command(root: &Path, words: &[String]) -> i32 {
    let models = Models::open(root, None);
    match words {
        [word, name] if word == "versions" => {
            let Some(versions) = models.versions(name) else {
                eprintln!("no synced model {name}");
                return 1;
            };
            for version in &versions.versions {
                let current = if version.n == versions.current { "  current" } else { "" };
                println!(
                    "{:>4}  {} UTC  {:<5}  {}  {} bytes{current}",
                    version.n,
                    date_time(version.at),
                    version.tick,
                    version.key,
                    version.size
                );
            }
            0
        }
        [word, name, n] if word == "rollback" => {
            let Ok(n) = n.parse::<u64>() else { usage() };
            match models.set_current(name, n) {
                Ok(()) => {
                    println!("{name}: version {n} is current");
                    0
                }
                Err(None) => {
                    eprintln!("{name} has no version {n}");
                    1
                }
                Err(Some(error)) => {
                    eprintln!("{name}: {error}");
                    1
                }
            }
        }
        _ => usage(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from("."));
    let mut address = "0.0.0.0:8080".to_owned();
    let mut hub_key = None;
    let mut words = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from).unwrap_or(root),
            "--listen" => address = args.next().unwrap_or(address),
            "--hub-key" => hub_key = Some(args.next().unwrap_or_else(|| usage())),
            word if !word.starts_with('-') => words.push(arg),
            _ => usage(),
        }
    }
    if !words.is_empty() {
        std::process::exit(command(&root, &words));
    }
    let hub_key = hub_key
        .map(|key| (key, "--hub-key".to_owned()))
        .or_else(|| std::fs::read_to_string(root.join(HUB_KEY_FILE)).ok().map(|key| (key, HUB_KEY_FILE.to_owned())))
        .map(|(key, source)| match ticket::parse_key(&key) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("{source}: not the hub's model sync key: {error}");
                std::process::exit(2);
            }
        });
    let _ = std::fs::create_dir_all(root.join("repository"));
    let cache = std::fs::read(root.join(CACHE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut repository =
        Repository { root: root.clone(), cache, manifest: Vec::new(), packs: BTreeMap::new(), scanned: None };
    repository.scan();
    let repository = Arc::new(Mutex::new(repository));
    let models = Arc::new(Models::open(&root, hub_key));
    let server = match Server::http(&address) {
        Ok(server) => Arc::new(server),
        Err(error) => {
            eprintln!("cannot listen on {address}: {error}");
            std::process::exit(1);
        }
    };
    println!("Sith cloud serving {} on http://{address}", root.display());
    if !models.writable() {
        println!("model sync uploads are off: no --hub-key or {HUB_KEY_FILE}");
    }
    // A few workers, so one slow download does not hold up the manifest.
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let server = Arc::clone(&server);
            let repository = Arc::clone(&repository);
            let models = Arc::clone(&models);
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    let _ = handle(request, &repository, &models);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_read_as_downloads_send_them() {
        assert_eq!(parse_range("bytes=100-", 1000), Some((100, 999)));
        assert_eq!(parse_range("bytes=0-9", 1000), Some((0, 9)));
        assert_eq!(parse_range("bytes=990-5000", 1000), Some((990, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("lines=1-", 1000), None);
    }

    #[test]
    fn paths_cannot_leave_the_repository() {
        assert_eq!(percent_decode("%7BJoF%7D%27s-GWing.pk3").as_deref(), Some("{JoF}'s-GWing.pk3"));
        assert!(!plain_name(&percent_decode("..").unwrap()));
        assert!(!plain_name(&percent_decode("%2E%2E").unwrap()));
        assert!(!plain_name(&percent_decode("a%2Fb").unwrap()));
        assert!(!plain_name(".sith-cloud-md5.json"));
    }

    #[test]
    fn the_manifest_reads_as_jof_content_reads_it() {
        let root = std::env::temp_dir().join("sith-cloud-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repository/maps")).unwrap();
        std::fs::write(root.join("repository/maps/{Sith} arena.pk3"), b"pk3").unwrap();
        std::fs::write(root.join("repository/maps/notes.txt"), b"x").unwrap();
        std::fs::write(
            root.join("groups.json"),
            r#"{"maps":{"name":"Sith Maps","description":"Arenas","type":"required"}}"#,
        )
        .unwrap();
        let mut repository = Repository {
            root: root.clone(),
            cache: BTreeMap::new(),
            manifest: Vec::new(),
            packs: BTreeMap::new(),
            scanned: None,
        };
        let manifest = sjk_jof_content::Manifest::parse(&repository.manifest()).unwrap();
        assert_eq!(manifest.skipped, 0);
        let group = &manifest.groups[0];
        assert_eq!(group.name, "Sith Maps");
        assert_eq!(group.kind, sjk_jof_content::GroupKind::Required);
        assert_eq!(group.assets.len(), 1, "only PK3s");
        let asset = &group.assets[0];
        assert_eq!(asset.md5, <[u8; 16]>::from(Md5::digest(b"pk3")));
        assert_eq!(asset.repository_path, ["maps"]);
        assert!(root.join(CACHE).exists());
    }

    fn zip_of(paths: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for path in paths {
            zip.start_file(*path, options).unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn host_pk3s_list_their_models_once_per_change() {
        let root = std::env::temp_dir().join("sith-cloud-test-packs");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repository/skins")).unwrap();
        std::fs::create_dir_all(root.join("repository/maps")).unwrap();
        let skins = zip_of(&["models/players/Kyle/model.glm", "Models/Players/kyle/a.skin", "models/players/reborn_x/b.tga"]);
        std::fs::write(root.join("repository/skins/kyle.pk3"), skins).unwrap();
        std::fs::write(root.join("repository/maps/broken.pk3"), b"not a zip").unwrap();
        let mut repository = Repository {
            root: root.clone(),
            cache: BTreeMap::new(),
            manifest: Vec::new(),
            packs: BTreeMap::new(),
            scanned: None,
        };
        let packs = repository.packs();
        assert_eq!(packs.keys().collect::<Vec<_>>(), ["kyle", "reborn_x"]);
        assert_eq!(packs["kyle"], HostPack { group: "skins".into(), file: "kyle.pk3".into() });
        let cached = &repository.cache["skins/kyle.pk3"];
        assert_eq!(cached.models.as_deref(), Some(&["kyle".to_owned(), "reborn_x".to_owned()][..]));
        assert_eq!(repository.cache["maps/broken.pk3"].models.as_deref(), Some(&[][..]), "read once, skipped");
        // The cache file keeps the listing, so a restart does not read the PK3 again.
        let saved: BTreeMap<String, Hashed> = serde_json::from_slice(&std::fs::read(root.join(CACHE)).unwrap()).unwrap();
        assert_eq!(saved["skins/kyle.pk3"], *cached);
    }

    /// Sends one raw HTTP request (`head`: request line and extra headers, one per
    /// item) and returns the status code and body.
    fn http(address: std::net::SocketAddr, head: &[&str], body: &[u8]) -> (u16, Vec<u8>) {
        use std::io::Write;
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        let mut request = head.join("\r\n");
        request.push_str(&format!("\r\nHost: test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
        stream.write_all(request.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        let split = answer.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let status = std::str::from_utf8(&answer[9..12]).unwrap().parse().unwrap();
        (status, answer[split + 4..].to_vec())
    }

    #[test]
    fn models_over_http() {
        let root = std::env::temp_dir().join("sith-cloud-test-http");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repository")).unwrap();
        let repository = Arc::new(Mutex::new(Repository {
            root: root.clone(),
            cache: BTreeMap::new(),
            manifest: Vec::new(),
            packs: BTreeMap::new(),
            scanned: None,
        }));
        let key = ticket::parse_key(&ticket::testing::hub_public_b64()).ok();
        let models = Arc::new(Models::open(&root, key));
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let address = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let _ = handle(request, &repository, &models);
            }
        });
        let (pk3, md5) = models::tests::pack("vader", "torso,a.tga");
        let upload = format!("Authorization: SJK-Ticket {}", ticket::testing::upload("k1", "vader", &md5, unix_now() + 600));
        assert_eq!(http(address, &["PUT /models/vader HTTP/1.1"], &pk3).0, 401);
        let (status, body) = http(address, &["PUT /models/vader HTTP/1.1", &upload], &pk3);
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!((status, body), (200, serde_json::json!({ "version": 1, "linked": false })));
        assert_eq!(http(address, &["GET /models/vader/1.pk3 HTTP/1.1"], b""), (200, pk3.clone()));
        let range = http(address, &["GET /models/vader/1.pk3 HTTP/1.1", "Range: bytes=10-"], b"");
        assert_eq!(range, (206, pk3[10..].to_vec()));
        assert_eq!(http(address, &["GET /models/vader/2.pk3 HTTP/1.1"], b"").0, 404);
        assert_eq!(http(address, &["GET /models/..%2Fx/1.pk3 HTTP/1.1"], b"").0, 404);
        let (status, body) = http(address, &["GET /models/index.json HTTP/1.1"], b"");
        let index: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!((status, &index["models"]["vader"]["content_md5"]), (200, &serde_json::json!(md5)));
        let rollback = format!("Authorization: SJK-Ticket {}", ticket::testing::rollback("vader", 1, unix_now() + 600));
        assert_eq!(http(address, &["POST /models/vader/rollback HTTP/1.1", &rollback], b"").0, 200);
        assert_eq!(http(address, &["GET /manifest.json HTTP/1.1"], b"").0, 200);
    }

    #[test]
    fn dates_read_as_eu_dates() {
        assert_eq!(date_time(0), "01/01/1970 00:00");
        assert_eq!(date_time(1_794_396_180), "11/11/2026 11:23");
        assert_eq!(date_time(1_709_251_140), "29/02/2024 23:59");
    }
}
