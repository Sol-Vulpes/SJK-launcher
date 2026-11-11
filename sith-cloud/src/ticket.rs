//! The SJK hub's model sync tickets (docs/model-sync.md in SJK, "Wire"): a short
//! JSON note signed with the hub's Ed25519 key, `<payload>.<signature>`, both
//! base64url without padding.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::fmt;

/// What a ticket allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Upload,
    Rollback,
}

impl Act {
    fn name(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Rollback => "rollback",
        }
    }

    /// The tick groups the hub may give this act to.
    fn allows(self, tick: &str) -> bool {
        match self {
            Self::Upload => matches!(tick, "jof" | "sith"),
            Self::Rollback => tick == "staff",
        }
    }
}

/// A ticket's payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub v: u32,
    pub act: String,
    /// The player's key id.
    pub key: String,
    pub tick: String,
    pub model: String,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub version: Option<u64>,
    /// Unix seconds.
    pub exp: u64,
}

/// A ticket whose signature and fields matched, with its signature text (what
/// the server remembers so the ticket is not used twice).
#[derive(Clone, Debug)]
pub struct Verified {
    pub ticket: Ticket,
    pub signature: String,
}

/// Why a ticket was refused (logged; the client only sees `bad_ticket`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadTicket {
    Format,
    Signature,
    Payload,
    Version,
    Expired,
    Act,
    Tick,
    Model,
}

impl fmt::Display for BadTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Format => "not <payload>.<signature>",
            Self::Signature => "signature does not match the hub key",
            Self::Payload => "payload is not a ticket",
            Self::Version => "unknown ticket version",
            Self::Expired => "expired",
            Self::Act => "for another action",
            Self::Tick => "tick group not allowed",
            Self::Model => "for another model",
        })
    }
}

/// The hub's public key, base64 (standard alphabet), 32 bytes.
pub fn parse_key(text: &str) -> Result<VerifyingKey, String> {
    let bytes = STANDARD.decode(text.trim()).map_err(|error| format!("not base64: {error}"))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| "not 32 bytes".to_owned())?;
    VerifyingKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

/// Checks `text`'s signature against `key`, then that it is ticket version 1, not
/// expired at `now`, for `act` by a tick group allowed it, and for `model`.
pub fn verify(text: &str, key: &VerifyingKey, act: Act, model: &str, now: u64) -> Result<Verified, BadTicket> {
    let (payload, signature) = text.trim().split_once('.').ok_or(BadTicket::Format)?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| BadTicket::Format)?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(signature).map_err(|_| BadTicket::Format)?;
    let signature_bytes: [u8; 64] = signature_bytes.try_into().map_err(|_| BadTicket::Format)?;
    key.verify_strict(&payload_bytes, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| BadTicket::Signature)?;
    let ticket: Ticket = serde_json::from_slice(&payload_bytes).map_err(|_| BadTicket::Payload)?;
    if ticket.v != 1 {
        return Err(BadTicket::Version);
    }
    if now >= ticket.exp {
        return Err(BadTicket::Expired);
    }
    if ticket.act != act.name() {
        return Err(BadTicket::Act);
    }
    if !act.allows(&ticket.tick) {
        return Err(BadTicket::Tick);
    }
    if ticket.model != model {
        return Err(BadTicket::Model);
    }
    Ok(Verified { ticket, signature: signature.to_owned() })
}

#[cfg(test)]
pub mod testing {
    //! A fixed hub key and a ticket maker for tests.
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub fn hub() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    pub fn hub_public_b64() -> String {
        STANDARD.encode(hub().verifying_key().as_bytes())
    }

    pub fn sign(payload: &serde_json::Value) -> String {
        let bytes = serde_json::to_vec(payload).unwrap();
        let signature = hub().sign(&bytes);
        format!("{}.{}", URL_SAFE_NO_PAD.encode(&bytes), URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    pub fn upload(key: &str, model: &str, md5: &str, exp: u64) -> String {
        sign(&serde_json::json!({
            "v": 1, "act": "upload", "key": key, "tick": "jof", "model": model,
            "md5": md5, "version": null, "exp": exp,
        }))
    }

    pub fn rollback(model: &str, version: u64, exp: u64) -> String {
        sign(&serde_json::json!({
            "v": 1, "act": "rollback", "key": "staffkey", "tick": "staff", "model": model,
            "md5": null, "version": version, "exp": exp,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn key() -> VerifyingKey {
        parse_key(&hub_public_b64()).unwrap()
    }

    #[test]
    fn a_good_ticket_passes() {
        let verified = verify(&upload("k1", "vader", "ab", 2000), &key(), Act::Upload, "vader", 1000).unwrap();
        assert_eq!(verified.ticket.key, "k1");
        assert_eq!(verified.ticket.md5.as_deref(), Some("ab"));
        let verified = verify(&rollback("vader", 3, 2000), &key(), Act::Rollback, "vader", 1000).unwrap();
        assert_eq!(verified.ticket.version, Some(3));
    }

    #[test]
    fn a_bad_signature_fails() {
        let ticket = upload("k1", "vader", "ab", 2000);
        let (payload, _) = ticket.split_once('.').unwrap();
        let other = upload("k2", "vader", "ab", 2000);
        let (_, other_signature) = other.split_once('.').unwrap();
        let forged = format!("{payload}.{other_signature}");
        assert_eq!(verify(&forged, &key(), Act::Upload, "vader", 1000).unwrap_err(), BadTicket::Signature);
        let stranger = SigningKey::from_bytes(&[9; 32]).verifying_key();
        assert_eq!(verify(&ticket, &stranger, Act::Upload, "vader", 1000).unwrap_err(), BadTicket::Signature);
        assert_eq!(verify("nonsense", &key(), Act::Upload, "vader", 1000).unwrap_err(), BadTicket::Format);
    }

    use ed25519_dalek::SigningKey;

    #[test]
    fn expired_wrong_act_tick_or_name_fail() {
        let ticket = upload("k1", "vader", "ab", 2000);
        assert_eq!(verify(&ticket, &key(), Act::Upload, "vader", 2000).unwrap_err(), BadTicket::Expired);
        assert_eq!(verify(&ticket, &key(), Act::Rollback, "vader", 1000).unwrap_err(), BadTicket::Act);
        assert_eq!(verify(&ticket, &key(), Act::Upload, "yoda", 1000).unwrap_err(), BadTicket::Model);
        let jawa = sign(&serde_json::json!({
            "v": 1, "act": "upload", "key": "k", "tick": "staff", "model": "vader", "md5": "ab", "version": null, "exp": 2000,
        }));
        assert_eq!(verify(&jawa, &key(), Act::Upload, "vader", 1000).unwrap_err(), BadTicket::Tick);
        let v2 = sign(&serde_json::json!({
            "v": 2, "act": "upload", "key": "k", "tick": "jof", "model": "vader", "md5": "ab", "version": null, "exp": 2000,
        }));
        assert_eq!(verify(&v2, &key(), Act::Upload, "vader", 1000).unwrap_err(), BadTicket::Version);
    }

    #[test]
    fn keys_parse_from_standard_base64() {
        assert!(parse_key("not a key").is_err());
        assert!(parse_key(&STANDARD.encode([1u8; 31])).is_err());
    }
}
