// SPDX-License-Identifier: Apache-2.0

//! Minimal Minima RPC client — only the calls `status` and `block`
//! need.
//!
//! This is **not** a replacement for `minima-attest`'s client. That
//! crate owns the anchor/proof RPC surface (the txn-build flow + the
//! mmrproof / coincheck retrieval). Here we want one thing: "is the
//! node up, synced, and peered?" — for the supervisor's own health
//! check.
//!
//! ## Transport facts (from `docs/minima-reference.md` §4)
//!
//! - HTTP GET with the command in the URL path, URL-encoded:
//!   `GET /status HTTP/1.1`. No JSON request body.
//! - HTTP Basic Auth, username hardcoded `minima` upstream, password
//!   is the `-rpcpassword` value.
//! - Response envelope: `{"status":bool, "response":{...}, ...}`.
//!   We unwrap `.response` (or surface `.error` on failure).
//! - **`status` has no usable last-block epoch.** `chain.time` is a
//!   `new Date(...).toString()` (human only). We call **`block`**
//!   for `timemilli` (ms epoch, as a string).
//! - **The shutdown RPC is `quit`, not `shutdown`.** We don't call
//!   it — SIGTERM via systemd is equivalent and safer.

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use url::Url;

use crate::error::Error;

/// What `furcate-minima status` and `furcate-minima healthz` report.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct NodeHealth {
    /// Whether `systemctl is-active furcate-minima` returned 0.
    pub systemd_active: bool,
    /// Current chain tip block number, from `status.chain.block`.
    pub block_height: u64,
    /// Connected peer count, from `status.network` (subfield TBD —
    /// see `RawStatus::peer_count`).
    pub peer_count: u32,
    /// Seconds since the last block was received, derived from
    /// `block.timemilli` vs `now`.
    pub last_block_age_seconds: u64,
    /// Whether the node is running with `-archive` (read from the
    /// installed config, not from the RPC — the RPC doesn't expose
    /// this directly).
    pub archive: bool,
    /// Reported Minima version (`status.version`), e.g.
    /// `"1.0.45.15"`. Useful when a Cloud Build pin diverges from the
    /// running jar.
    pub version: String,
}

impl NodeHealth {
    /// `healthz`'s pass/fail criterion.
    ///
    /// Definition of healthy (from `docs/minima-reference.md` §5.4):
    /// - systemd unit active,
    /// - chain length > 0 (i.e. past first sync),
    /// - at least one peer,
    /// - last block age < 10 minutes.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        self.systemd_active
            && self.block_height > 0
            && self.peer_count >= 1
            && self.last_block_age_seconds < 600
    }
}

/// Call Minima's RPC `status` command.
///
/// URL: `GET <rpc_url>/status` with `Authorization: Basic
/// base64("minima:<rpc_password>")`.
///
/// Returns the parsed `RawStatus` (a subset of Minima's response —
/// the only fields the supervisor cares about).
///
/// # Errors
///
/// Returns [`Error::Rpc`] on a malformed RPC URL or a `status:false`
/// envelope, and [`Error::Http`] on transport failures or non-2xx
/// responses.
pub async fn status(
    rpc_url: &Url,
    rpc_password: &SecretString,
    client: &reqwest::Client,
) -> Result<RawStatus, Error> {
    let url = rpc_url
        .join("status")
        .map_err(|e| Error::Rpc(format!("malformed rpc_url: {e}")))?;
    let resp = client
        .get(url)
        .basic_auth("minima", Some(rpc_password.expose_secret()))
        .send()
        .await?
        .error_for_status()?;
    let env: Envelope<RawStatus> = resp.json().await?;
    env.into_response("status")
}

/// Call Minima's RPC `block` command — current tip only.
///
/// URL: `GET <rpc_url>/block`. Returns the tip's `timemilli` (ms
/// epoch, as a string in Minima's payload — we parse to `u64`).
///
/// # Errors
///
/// Same shape as [`status`]: [`Error::Rpc`] on URL or envelope
/// failures, [`Error::Http`] on transport failures.
pub async fn block(
    rpc_url: &Url,
    rpc_password: &SecretString,
    client: &reqwest::Client,
) -> Result<RawBlock, Error> {
    let url = rpc_url
        .join("block")
        .map_err(|e| Error::Rpc(format!("malformed rpc_url: {e}")))?;
    let resp = client
        .get(url)
        .basic_auth("minima", Some(rpc_password.expose_secret()))
        .send()
        .await?
        .error_for_status()?;
    let env: Envelope<RawBlock> = resp.json().await?;
    env.into_response("block")
}

/// Common Minima response envelope.
///
/// Source: `Command.getJSONReply()` and
/// `CommandRunner.java:315-326`. Successful responses have
/// `status:true` and a `response` payload; failures have
/// `status:false` and an `error` string.
#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    status: bool,
    // `Option::default()` is `None` — no `#[serde(default)]` needed,
    // and adding it would force a spurious `T: Default` bound on the
    // derive macro's generated impl.
    response: Option<T>,
    error: Option<String>,
}

impl<T> Envelope<T> {
    fn into_response(self, cmd: &str) -> Result<T, Error> {
        if !self.status {
            return Err(Error::Rpc(format!(
                "{cmd}: {}",
                self.error
                    .unwrap_or_else(|| "status:false with no error message".into()),
            )));
        }
        self.response
            .ok_or_else(|| Error::Rpc(format!("{cmd}: status:true but no response field")))
    }
}

/// Subset of Minima's `status` response that the supervisor uses.
///
/// Real response is much larger (chain.weight, chain.cascade,
/// memory.files.*, etc.). We extract only what `NodeHealth` needs.
/// Field names verified verbatim from `status.java`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawStatus {
    /// `status.version`, e.g. `"1.0.45.15"`.
    #[serde(default)]
    pub version: String,
    /// `status.chain` sub-object.
    pub chain: ChainStatus,
    /// `status.network` sub-object. Peer count field name is
    /// **UNVERIFIED** — `NetworkManager.getStatus()`'s payload shape
    /// was not pinned down in the docs research pass. Likely
    /// candidates: `peers`, `connected`, `numpeers`. Confirm at
    /// runtime against a real node.
    #[serde(default)]
    pub network: NetworkStatus,
}

/// `status.chain` sub-object — only the fields the supervisor reads.
#[derive(Clone, Debug, Deserialize)]
pub struct ChainStatus {
    /// Tip block number. Source field: `chain.block` (number).
    pub block: u64,
}

/// **UNVERIFIED field names** — see `RawStatus::network` note.
///
/// `serde(default)` everywhere so partial / unknown shapes still
/// deserialize and the supervisor reports `peer_count: 0` rather
/// than crashing on a schema drift.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct NetworkStatus {
    /// Best guess: total peer count.
    pub connected: Option<u32>,
    /// Alternate name some clients use.
    pub peers: Option<u32>,
}

impl NetworkStatus {
    /// Resolve whichever field this Minima version uses.
    #[must_use]
    pub fn peer_count(&self) -> u32 {
        self.connected.or(self.peers).unwrap_or(0)
    }
}

/// `block` RPC response (current tip).
///
/// Field shapes from `src/org/minima/system/commands/base/block.java:47`:
/// `block` and `timemilli` are emitted as **strings**, not numbers
/// (`topblock.getTimeMilli().toString()`). We deserialize as
/// strings and parse at call sites.
#[derive(Clone, Debug, Deserialize)]
pub struct RawBlock {
    /// Block number, as a string.
    #[serde(default)]
    pub block: String,
    /// Block hash (`0x...`).
    #[serde(default)]
    pub hash: String,
    /// Unix epoch in milliseconds, as a string. Parse to `u64`.
    #[serde(default)]
    pub timemilli: String,
}

impl RawBlock {
    /// Parse `timemilli` to a `u64`. Returns 0 on parse failure
    /// (which is what we want for the staleness check — an unparseable
    /// timestamp treated as "epoch start" makes `last_block_age`
    /// huge and trips the healthz threshold cleanly).
    #[must_use]
    pub fn timemilli_u64(&self) -> u64 {
        self.timemilli.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_status_passes_threshold() {
        let h = NodeHealth {
            systemd_active: true,
            block_height: 12_345,
            peer_count: 7,
            last_block_age_seconds: 42,
            archive: true,
            version: "1.0.45.15".into(),
        };
        assert!(h.is_healthy());
    }

    #[test]
    fn stale_tip_fails_healthz() {
        let h = NodeHealth {
            systemd_active: true,
            block_height: 12_345,
            peer_count: 7,
            last_block_age_seconds: 1_000,
            archive: true,
            version: "1.0.45.15".into(),
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn no_peers_fails_healthz() {
        let h = NodeHealth {
            systemd_active: true,
            block_height: 12_345,
            peer_count: 0,
            last_block_age_seconds: 10,
            archive: true,
            version: "1.0.45.15".into(),
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn cold_node_fails_healthz() {
        let h = NodeHealth {
            systemd_active: true,
            block_height: 0,
            peer_count: 5,
            last_block_age_seconds: 10,
            archive: true,
            version: "1.0.45.15".into(),
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn systemd_down_fails_healthz() {
        let h = NodeHealth {
            systemd_active: false,
            block_height: 12_345,
            peer_count: 5,
            last_block_age_seconds: 10,
            archive: true,
            version: "1.0.45.15".into(),
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn network_peer_count_resolves_either_field() {
        let connected = NetworkStatus {
            connected: Some(5),
            peers: None,
        };
        assert_eq!(connected.peer_count(), 5);
        let peers = NetworkStatus {
            connected: None,
            peers: Some(3),
        };
        assert_eq!(peers.peer_count(), 3);
        // `connected` wins if both present
        let both = NetworkStatus {
            connected: Some(5),
            peers: Some(3),
        };
        assert_eq!(both.peer_count(), 5);
        let none = NetworkStatus::default();
        assert_eq!(none.peer_count(), 0);
    }

    #[test]
    fn raw_block_parses_timemilli() {
        let b = RawBlock {
            block: "234567".into(),
            hash: "0xdeadbeef".into(),
            timemilli: "1748077284000".into(),
        };
        assert_eq!(b.timemilli_u64(), 1_748_077_284_000);
    }

    #[test]
    fn raw_block_handles_unparseable_timemilli() {
        let b = RawBlock {
            block: "0".into(),
            hash: String::new(),
            timemilli: "not-a-number".into(),
        };
        // Unparseable → 0, which produces a huge `last_block_age`
        // and trips healthz off — the right failure mode.
        assert_eq!(b.timemilli_u64(), 0);
    }

    #[test]
    fn envelope_surfaces_error_on_status_false() {
        let json = r#"{"status":false,"error":"node not ready"}"#;
        let env: Envelope<RawBlock> = serde_json::from_str(json).unwrap();
        let err = env.into_response("block").unwrap_err();
        assert!(matches!(err, Error::Rpc(msg) if msg.contains("node not ready")));
    }

    #[test]
    fn envelope_extracts_response_on_status_true() {
        let json = r#"{"status":true,"response":{"block":"5","hash":"0x00","timemilli":"100"}}"#;
        let env: Envelope<RawBlock> = serde_json::from_str(json).unwrap();
        let b = env.into_response("block").unwrap();
        assert_eq!(b.block, "5");
        assert_eq!(b.timemilli_u64(), 100);
    }
}
