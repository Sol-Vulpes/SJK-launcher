//! Model sync: players' synced models under `<root>/models/` (docs/model-sync.md in
//! SJK, "Wire"). Outside `repository/`, so the manifest never lists them.
//!
//! ```text
//! <root>/models/
//!   uploads.json            the last day's uploads, for the rate limits
//!   <name>/versions.json    every version (who, when, hashes) and the current one
//!   <name>/<n>.pk3          version n; versions are never deleted
//! ```

use crate::ticket::{self, Act, BadTicket};
use ed25519_dalek::VerifyingKey;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sjk_model_sync::Limits;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const DAY: u64 = 24 * 60 * 60;
const HOUR: u64 = 60 * 60;
/// Stored uploads a key may make in a day.
const UPLOADS_PER_DAY: usize = 5;
const UPLOADS_LOG: &str = "uploads.json";
const VERSIONS: &str = "versions.json";

/// A model folder found in one of the hosts' own PK3s.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HostPack {
    pub group: String,
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    pub n: u64,
    /// The PK3 file's MD5.
    pub md5: String,
    pub content_md5: String,
    pub size: u64,
    /// The sender's key id, from the ticket.
    pub key: String,
    pub tick: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Versions {
    pub current: u64,
    pub versions: Vec<Version>,
}

impl Versions {
    pub fn current(&self) -> Option<&Version> {
        self.versions.iter().find(|version| version.n == self.current)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Upload {
    key: String,
    model: String,
    at: u64,
}

#[derive(Default)]
struct State {
    /// Used tickets' signatures, until they expire.
    used: HashMap<String, u64>,
    uploads: Vec<Upload>,
}

/// An answer: HTTP status and JSON body.
#[derive(Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

fn error(status: u16, code: &str) -> Reply {
    Reply { status, body: json!({ "error": code }) }
}

pub struct Models {
    dir: PathBuf,
    hub_key: Option<VerifyingKey>,
    /// Every write goes through this lock: the server has several workers.
    state: Mutex<State>,
}

impl Models {
    pub fn open(root: &Path, hub_key: Option<VerifyingKey>) -> Self {
        let dir = root.join("models");
        let uploads = std::fs::read(dir.join(UPLOADS_LOG))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { dir, hub_key, state: Mutex::new(State { used: HashMap::new(), uploads }) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Whether uploads and rollbacks are on (the hub's key is configured).
    pub fn writable(&self) -> bool {
        self.hub_key.is_some()
    }

    pub fn versions(&self, name: &str) -> Option<Versions> {
        if !sjk_model_sync::valid_name(name) {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(name).join(VERSIONS)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Version `n`'s file, when `name` is a model name.
    pub fn pk3_path(&self, name: &str, n: u64) -> Option<PathBuf> {
        sjk_model_sync::valid_name(name).then(|| self.dir.join(name).join(format!("{n}.pk3")))
    }

    /// `GET /models/index.json`.
    pub fn index(&self, packs: &BTreeMap<String, HostPack>) -> Value {
        let mut models = serde_json::Map::new();
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if !sjk_model_sync::valid_name(&name) {
                continue;
            }
            let Some(versions) = self.versions(&name) else { continue };
            let Some(current) = versions.current() else { continue };
            models.insert(
                name,
                json!({
                    "version": current.n,
                    "size": current.size,
                    "md5": current.md5,
                    "content_md5": current.content_md5,
                    "at": current.at,
                }),
            );
        }
        json!({ "models": models, "packs": packs })
    }

    /// `PUT /models/<name>`: `authorization` is the header's value, `body` the PK3.
    pub fn upload(
        &self,
        name: &str,
        authorization: Option<&str>,
        body: &mut dyn Read,
        packs: &BTreeMap<String, HostPack>,
        now: u64,
    ) -> Reply {
        let Some(hub_key) = &self.hub_key else { return error(404, "not_found") };
        if !sjk_model_sync::valid_name(name) {
            return error(404, "not_found");
        }
        let verified = match ticket_of(authorization, hub_key, Act::Upload, name, now) {
            Ok(verified) => verified,
            Err(reply) => return reply,
        };
        let limit = Limits::DEFAULT.pk3_bytes;
        let mut pk3 = Vec::new();
        if body.take(limit + 1).read_to_end(&mut pk3).is_err() {
            return error(400, "bad_body");
        }
        if pk3.len() as u64 > limit {
            return error(413, "too_large");
        }
        let checked = match sjk_model_sync::check(&pk3, &Limits::DEFAULT) {
            Ok(checked) => checked,
            Err(rejection) => return Reply { status: 400, body: json!({ "error": "rejected", "why": rejection.to_string() }) },
        };
        let ticket = &verified.ticket;
        if checked.name != name
            || !ticket.md5.as_deref().is_some_and(|md5| md5.eq_ignore_ascii_case(&checked.content_md5))
        {
            println!("model {name}: bad ticket from {}: for other content", ticket.key);
            return error(401, "bad_ticket");
        }

        let mut state = self.lock();
        if state.used.contains_key(&verified.signature) {
            println!("model {name}: ticket from {} used again", ticket.key);
            return error(401, "bad_ticket");
        }
        // A host pack's name may get a newer version too (Sol, 11/11/2026): clients load
        // the synced file after the hosts' packs, so it wins for those who fetch it.
        if let Some(pack) = packs.get(name) {
            println!("model {name}: a newer version over the hosts' {} from {}", pack.file, ticket.key);
        }
        let mut versions = self.versions(name).unwrap_or_default();
        if let Some(current) = versions.current()
            && current.content_md5 == checked.content_md5
        {
            let n = current.n;
            remember(&mut state, &verified, now);
            return Reply { status: 200, body: json!({ "version": n, "linked": true }) };
        }
        state.uploads.retain(|upload| now.saturating_sub(upload.at) < DAY);
        let by_key = state.uploads.iter().filter(|upload| upload.key == ticket.key).count();
        let by_name = state.uploads.iter().any(|upload| upload.model == name && now.saturating_sub(upload.at) < HOUR);
        if by_key >= UPLOADS_PER_DAY || by_name {
            return error(429, "rate_limited");
        }

        let n = versions.versions.iter().map(|version| version.n).max().unwrap_or(0) + 1;
        let folder = self.dir.join(name);
        let stored = std::fs::create_dir_all(&folder)
            .and_then(|()| write_atomic(&folder.join(format!("{n}.pk3")), &pk3));
        let version = Version {
            n,
            md5: hex(&Md5::digest(&pk3)),
            content_md5: checked.content_md5,
            size: pk3.len() as u64,
            key: ticket.key.clone(),
            tick: ticket.tick.clone(),
            at: now,
        };
        versions.versions.push(version);
        versions.current = n;
        if let Err(problem) = stored.and_then(|()| self.save_versions(name, &versions)) {
            eprintln!("model {name}: cannot store version {n}: {problem}");
            return error(500, "storage");
        }
        state.uploads.push(Upload { key: ticket.key.clone(), model: name.to_owned(), at: now });
        self.save_uploads(&state.uploads);
        remember(&mut state, &verified, now);
        println!("model {name}: version {n} from {} ({})", ticket.key, ticket.tick);
        Reply { status: 200, body: json!({ "version": n, "linked": false }) }
    }

    /// `POST /models/<name>/rollback` with a staff rollback ticket.
    pub fn rollback(&self, name: &str, authorization: Option<&str>, now: u64) -> Reply {
        let Some(hub_key) = &self.hub_key else { return error(404, "not_found") };
        if !sjk_model_sync::valid_name(name) {
            return error(404, "not_found");
        }
        let verified = match ticket_of(authorization, hub_key, Act::Rollback, name, now) {
            Ok(verified) => verified,
            Err(reply) => return reply,
        };
        let Some(n) = verified.ticket.version else { return error(401, "bad_ticket") };
        let mut state = self.lock();
        if state.used.contains_key(&verified.signature) {
            return error(401, "bad_ticket");
        }
        match self.set_current(name, n) {
            Ok(()) => {
                remember(&mut state, &verified, now);
                println!("model {name}: rolled back to version {n} by {}", verified.ticket.key);
                Reply { status: 200, body: json!({ "version": n }) }
            }
            Err(None) => error(404, "not_found"),
            Err(Some(problem)) => {
                eprintln!("model {name}: cannot roll back: {problem}");
                error(500, "storage")
            }
        }
    }

    /// Makes version `n` current. `Err(None)`: no such version.
    pub fn set_current(&self, name: &str, n: u64) -> Result<(), Option<std::io::Error>> {
        let mut versions = self.versions(name).ok_or(None)?;
        if !versions.versions.iter().any(|version| version.n == n) {
            return Err(None);
        }
        versions.current = n;
        self.save_versions(name, &versions).map_err(Some)
    }

    fn save_versions(&self, name: &str, versions: &Versions) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(versions).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join(name).join(VERSIONS), &bytes)
    }

    fn save_uploads(&self, uploads: &[Upload]) {
        let saved = std::fs::create_dir_all(&self.dir)
            .and_then(|()| serde_json::to_vec(uploads).map_err(std::io::Error::other))
            .and_then(|bytes| write_atomic(&self.dir.join(UPLOADS_LOG), &bytes));
        if let Err(problem) = saved {
            eprintln!("models/{UPLOADS_LOG}: {problem}");
        }
    }
}

/// The `SJK-Ticket` in an `Authorization` header, verified, or the 401 to answer.
fn ticket_of(
    authorization: Option<&str>,
    key: &VerifyingKey,
    act: Act,
    name: &str,
    now: u64,
) -> Result<ticket::Verified, Reply> {
    let text = authorization
        .and_then(|value| value.trim().strip_prefix("SJK-Ticket "))
        .ok_or(BadTicket::Format)
        .and_then(|text| ticket::verify(text, key, act, name, now));
    text.map_err(|why| {
        println!("model {name}: bad ticket: {why}");
        error(401, "bad_ticket")
    })
}

fn remember(state: &mut State, verified: &ticket::Verified, now: u64) {
    state.used.retain(|_, exp| *exp > now);
    state.used.insert(verified.signature.clone(), verified.ticket.exp);
}

/// Writes `path` whole or not at all: a temporary file next to it, then a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ticket::testing;

    const NOW: u64 = 1_800_000_000;

    /// A one-triangle GLM (the layout of sjk-model-sync's tests).
    fn glm() -> Vec<u8> {
        fn put(data: &mut [u8], at: usize, value: i32) {
            data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        const HEADER: usize = 164;
        const HIERARCHY: usize = HEADER + 4;
        const LOD: usize = HIERARCHY + 144;
        const SURFACE: usize = LOD + 8;
        const VERTICES: usize = 40;
        let triangles = VERTICES + 3 * 32 + 3 * 8;
        let references = triangles + 12;
        let surface_end = references + 4;
        let end = SURFACE + surface_end;
        let mut data = vec![0; end];
        data[..4].copy_from_slice(b"2LGM");
        put(&mut data, 4, 6);
        data[8..13].copy_from_slice(b"model");
        put(&mut data, 140, 1);
        put(&mut data, 144, 1);
        put(&mut data, 148, LOD as i32);
        put(&mut data, 152, 1);
        put(&mut data, 156, HIERARCHY as i32);
        put(&mut data, 160, end as i32);
        put(&mut data, HEADER, (HIERARCHY - HEADER) as i32);
        data[HIERARCHY..HIERARCHY + 5].copy_from_slice(b"torso");
        put(&mut data, HIERARCHY + 136, -1);
        put(&mut data, LOD, (end - LOD) as i32);
        put(&mut data, LOD + 4, (SURFACE - (LOD + 4)) as i32);
        put(&mut data, SURFACE + 12, 3);
        put(&mut data, SURFACE + 16, VERTICES as i32);
        put(&mut data, SURFACE + 20, 1);
        put(&mut data, SURFACE + 24, triangles as i32);
        put(&mut data, SURFACE + 28, 1);
        put(&mut data, SURFACE + 32, references as i32);
        put(&mut data, SURFACE + 36, surface_end as i32);
        for corner in 0..3 {
            put(&mut data, SURFACE + triangles + corner * 4, corner as i32);
        }
        data
    }

    /// A pack of model `name` whose skin says `skin`; returns the PK3 and its content MD5.
    pub(crate) fn pack(name: &str, skin: &str) -> (Vec<u8>, String) {
        let files = vec![
            (format!("models/players/{name}/model.glm"), glm()),
            (format!("models/players/{name}/model_default.skin"), skin.as_bytes().to_vec()),
        ];
        let (pk3, checked) = sjk_model_sync::pack::pack(name, files, &Limits::DEFAULT).unwrap();
        (pk3, checked.content_md5)
    }

    fn models(test: &str) -> (PathBuf, Models) {
        let root = std::env::temp_dir().join(format!("sith-cloud-models-{test}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let key = ticket::parse_key(&testing::hub_public_b64()).unwrap();
        let models = Models::open(&root, Some(key));
        (root, models)
    }

    fn send(models: &Models, name: &str, key: &str, pk3: &[u8], md5: &str, now: u64) -> Reply {
        let ticket = testing::upload(key, name, md5, now + 600);
        models.upload(name, Some(&format!("SJK-Ticket {ticket}")), &mut &pk3[..], &BTreeMap::new(), now)
    }

    #[test]
    fn uploads_store_versions_and_link_the_same_content() {
        let (root, models) = models("store");
        let (pk3, md5) = pack("vader", "torso,a.tga");
        assert_eq!(send(&models, "vader", "k1", &pk3, &md5, NOW).body, json!({ "version": 1, "linked": false }));
        assert_eq!(std::fs::read(root.join("models/vader/1.pk3")).unwrap(), pk3);
        // Same content again, from anyone, at once: linked, nothing stored, not rate limited.
        assert_eq!(send(&models, "vader", "k2", &pk3, &md5, NOW + 1).body, json!({ "version": 1, "linked": true }));
        assert!(!root.join("models/vader/2.pk3").exists());
        let (pk3_b, md5_b) = pack("vader", "torso,b.tga");
        assert_eq!(send(&models, "vader", "k2", &pk3_b, &md5_b, NOW + HOUR).body, json!({ "version": 2, "linked": false }));
        let versions = models.versions("vader").unwrap();
        assert_eq!(versions.current, 2);
        assert_eq!(versions.versions.len(), 2);
        assert_eq!(versions.versions[0].key, "k1");
        assert_eq!(versions.versions[1].tick, "jof");
        assert_eq!(versions.versions[1].content_md5, md5_b);
        assert_eq!(versions.versions[1].md5, hex(&Md5::digest(&pk3_b)));

        let index = models.index(&BTreeMap::from([(
            "kyle".to_owned(),
            HostPack { group: "skins".into(), file: "kyle.pk3".into() },
        )]));
        assert_eq!(index["models"]["vader"]["version"], 2);
        assert_eq!(index["models"]["vader"]["size"], pk3_b.len());
        assert_eq!(index["models"]["vader"]["content_md5"], md5_b.as_str());
        assert_eq!(index["packs"]["kyle"], json!({ "group": "skins", "file": "kyle.pk3" }));
    }

    #[test]
    fn tickets_must_match_the_body_and_serve_once() {
        let (_root, models) = models("tickets");
        let (pk3, md5) = pack("vader", "torso,a.tga");
        let (_, other_md5) = pack("vader", "torso,b.tga");
        assert_eq!(send(&models, "vader", "k1", &pk3, &other_md5, NOW).body, json!({ "error": "bad_ticket" }));
        assert_eq!(models.upload("vader", None, &mut &pk3[..], &BTreeMap::new(), NOW).status, 401);
        let ticket = format!("SJK-Ticket {}", testing::upload("k1", "vader", &md5, NOW + 600));
        assert_eq!(models.upload("vader", Some(&ticket), &mut &pk3[..], &BTreeMap::new(), NOW).status, 200);
        let again = models.upload("vader", Some(&ticket), &mut &pk3[..], &BTreeMap::new(), NOW + 1);
        assert_eq!(again, error(401, "bad_ticket"));
        // A pack of another model under this name.
        let (yoda, yoda_md5) = pack("yoda", "torso,a.tga");
        assert_eq!(send(&models, "vader", "k1", &yoda, &yoda_md5, NOW).status, 401);
    }

    #[test]
    fn rejected_packs_say_why() {
        let (_root, models) = models("rejected");
        let reply = send(&models, "vader", "k1", b"not a zip", "00", NOW);
        assert_eq!(reply.status, 400);
        assert_eq!(reply.body["error"], "rejected");
        assert!(reply.body["why"].as_str().unwrap().starts_with("not a readable PK3"), "{reply:?}");
        let big = vec![0u8; Limits::DEFAULT.pk3_bytes as usize + 1];
        assert_eq!(send(&models, "vader", "k1", &big, "00", NOW), error(413, "too_large"));
    }

    #[test]
    fn a_host_packs_model_gets_newer_versions_too() {
        let (_root, models) = models("host");
        let (pk3, md5) = pack("kyle", "torso,a.tga");
        let packs = BTreeMap::from([("kyle".to_owned(), HostPack { group: "skins".into(), file: "kyle.pk3".into() })]);
        let ticket = format!("SJK-Ticket {}", testing::upload("k1", "kyle", &md5, NOW + 600));
        let reply = models.upload("kyle", Some(&ticket), &mut &pk3[..], &packs, NOW);
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body["version"], 1);
        assert_eq!(models.index(&packs)["models"]["kyle"]["version"], 1);
        assert_eq!(models.index(&packs)["packs"]["kyle"]["file"], "kyle.pk3");
    }

    #[test]
    fn uploads_are_rate_limited_and_the_log_survives_restarts() {
        let (root, models) = models("rate");
        let (pk3, md5) = pack("vader", "torso,a.tga");
        send(&models, "vader", "k1", &pk3, &md5, NOW);
        let (pk3_b, md5_b) = pack("vader", "torso,b.tga");
        assert_eq!(send(&models, "vader", "k2", &pk3_b, &md5_b, NOW + 60), error(429, "rate_limited"));
        for (index, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
            let (pk3, md5) = pack(name, "torso,a.tga");
            assert_eq!(send(&models, name, "k1", &pk3, &md5, NOW + index as u64).status, 200);
        }
        let (pk3_e, md5_e) = pack("e", "torso,a.tga");
        assert_eq!(send(&models, "e", "k1", &pk3_e, &md5_e, NOW + 10), error(429, "rate_limited"));
        let models = Models::open(&root, ticket::parse_key(&testing::hub_public_b64()).ok());
        assert_eq!(send(&models, "e", "k1", &pk3_e, &md5_e, NOW + 20), error(429, "rate_limited"));
        assert_eq!(send(&models, "e", "k2", &pk3_e, &md5_e, NOW + 20).status, 200);
        assert_eq!(send(&models, "vader", "k2", &pk3_b, &md5_b, NOW + HOUR).status, 200);
        assert_eq!(send(&models, "f", "k1", &pk3_e, &md5_e, NOW + DAY).status, 401, "the e pack is not f");
        let (pk3_f, md5_f) = pack("f", "torso,a.tga");
        assert_eq!(send(&models, "f", "k1", &pk3_f, &md5_f, NOW + DAY).status, 200);
    }

    #[test]
    fn staff_roll_back_to_an_existing_version() {
        let (_root, models) = models("rollback");
        let (pk3, md5) = pack("vader", "torso,a.tga");
        send(&models, "vader", "k1", &pk3, &md5, NOW);
        let (pk3_b, md5_b) = pack("vader", "torso,b.tga");
        send(&models, "vader", "k1", &pk3_b, &md5_b, NOW + HOUR);
        let missing = format!("SJK-Ticket {}", testing::rollback("vader", 9, NOW + 600));
        assert_eq!(models.rollback("vader", Some(&missing), NOW), error(404, "not_found"));
        let upload = format!("SJK-Ticket {}", testing::upload("k1", "vader", &md5, NOW + 600));
        assert_eq!(models.rollback("vader", Some(&upload), NOW).status, 401);
        let ticket = format!("SJK-Ticket {}", testing::rollback("vader", 1, NOW + 600));
        assert_eq!(models.rollback("vader", Some(&ticket), NOW).body, json!({ "version": 1 }));
        assert_eq!(models.versions("vader").unwrap().current, 1);
        assert_eq!(models.versions("vader").unwrap().versions.len(), 2, "never deleted");
        assert_eq!(models.rollback("vader", Some(&ticket), NOW + 1).status, 401, "used");
        // Version 2's content again is a new version (the current one is 1).
        assert_eq!(send(&models, "vader", "k2", &pk3_b, &md5_b, NOW + 2 * HOUR).body["version"], 3);
    }

    #[test]
    fn without_the_hub_key_writes_are_not_there() {
        let root = std::env::temp_dir().join("sith-cloud-models-nokey");
        let models = Models::open(&root, None);
        assert!(!models.writable());
        assert_eq!(models.rollback("vader", None, NOW).status, 404);
        assert_eq!(models.upload("vader", None, &mut &b""[..], &BTreeMap::new(), NOW).status, 404);
    }
}
