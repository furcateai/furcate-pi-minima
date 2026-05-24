// SPDX-License-Identifier: Apache-2.0

//! # `furcate-pi-minima`
//!
//! Pi-class operator wrapper that supervises an upstream Minima full node
//! tuned for Furcate attestation workloads.
//!
//! ## What this crate is
//!
//! - A binary, `furcate-minima`, that installs / starts / stops / monitors
//!   a Minima node on a Raspberry Pi via systemd.
//! - A small library surface ([`LocalMinimaNode`]) that downstream code
//!   uses to discover the local node's RPC URL + password from the standard
//!   install layout, without each caller hand-coding paths.
//!
//! ## What this crate is not
//!
//! - Not a fork or reimplementation of any part of Minima — the `.jar` is
//!   the upstream Apache-2.0 artefact, fetched at install time and SHA256-
//!   verified against the manifest pinned in this crate.
//! - Not an Integritas node: Integritas is hosted `SaaS`, not a separable
//!   node binary. The [`Profile::Attestor`] default sets the flags
//!   appropriate for the anchoring workload that `minima-attest` generates.
//! - Not a turnkey OS image — operators install the crate's binary and run
//!   `furcate-minima init`.
//!
//! ## Profiles
//!
//! See [`Profile`] for the exact flag sets. Short version:
//!
//! - [`Profile::Attestor`] (default) — `-archive` + raised `-txpowdbstore`,
//!   RPC on loopback, no MDS surface. Tuned so anchors written today
//!   remain locally provable months from now.
//! - [`Profile::Minimal`] — vanilla full node; anchors older than the
//!   default prune windows are not locally provable.
//! - [`Profile::Custom`] — pass-through flags from
//!   `/etc/furcate-pi-minima/custom-flags`.
//!
//! See `docs/furcate-pi-minima-design.md` in the repository root for the
//! full design rationale.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

pub mod config;
pub mod error;
pub mod jar;
pub mod profile;
pub mod rpc;
pub mod secrets;
pub mod systemd;

#[cfg(target_os = "linux")]
pub mod ops;

pub use config::{Config, Paths};
pub use error::Error;
pub use profile::Profile;

use std::sync::Arc;

use secrecy::SecretString;
use url::Url;

/// A handle to a Minima node running on the local host under
/// `furcate-pi-minima`'s standard install layout.
///
/// Downstream code uses this to discover the RPC URL and password without
/// hand-coding paths. The expected use is one-line construction at startup
/// and then passing the `attester()` (or the raw `rpc_url` / `rpc_password`)
/// into whichever sink is doing the anchoring.
#[derive(Clone)]
pub struct LocalMinimaNode {
    /// Always `http://127.0.0.1:9005` in the default install. Stored as
    /// a typed [`Url`] so callers don't reparse.
    pub rpc_url: Url,
    /// Read from `/etc/furcate-pi-minima/rpcpassword` (mode `0600`,
    /// owned by group `furcate-minima`). Wrapped in [`SecretString`] so
    /// it doesn't land in panic messages or `Debug` output.
    pub rpc_password: Arc<SecretString>,
}

impl LocalMinimaNode {
    /// Load from the standard install layout produced by
    /// `furcate-minima init`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotInstalled`] if the config dir is missing, or
    /// [`Error::PermissionDenied`] if the caller can't read
    /// `rpcpassword` (the file is `0600` by design — the calling
    /// process must be in group `furcate-minima` or root).
    pub fn from_default_install() -> Result<Self, Error> {
        Self::from_paths(&Paths::default())
    }

    /// Load from an explicit [`Paths`] layout. Useful for tests and for
    /// non-standard installs (e.g. a packaged Docker image that places
    /// config under `/run/secrets/`).
    ///
    /// # Errors
    ///
    /// Same as [`Self::from_default_install`]: missing config dir,
    /// unreadable `rpcpassword`, or malformed `config.toml`.
    pub fn from_paths(paths: &Paths) -> Result<Self, Error> {
        // Pull the persisted config first — it tells us which port
        // the supervised node is on. Then read the rpcpassword.
        let config_bytes = std::fs::read(paths.config_toml()).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotInstalled(paths.config_toml()),
            std::io::ErrorKind::PermissionDenied => Error::PermissionDenied(paths.config_toml()),
            _ => Error::Io(e),
        })?;
        let config_text = String::from_utf8(config_bytes)
            .map_err(|e| Error::Preflight(format!("config.toml not UTF-8: {e}")))?;
        let config: Config = toml::from_str(&config_text)?;

        let rpc_password = secrets::read_secret(&paths.rpcpassword())?;

        let rpc_url = Url::parse(&format!("http://127.0.0.1:{}/", config.rpc_port()))
            .map_err(|e| Error::Preflight(format!("could not build RPC URL: {e}")))?;

        Ok(Self {
            rpc_url,
            rpc_password: Arc::new(SecretString::new(rpc_password)),
        })
    }

    /// Build a fully-wired `minima_attest::MinimaAttester` pointed at this
    /// node.
    ///
    /// Available only when the `minima-attest` feature is enabled, to keep
    /// the dep tree minimal for operators who only want the supervisor
    /// binary and not the client library.
    ///
    /// `id` is the operator-chosen attester name — it shows up in
    /// `Attestation::kind` downstream (e.g. `"minima:local"`).
    ///
    /// Internally this constructs a `MinimaClient` (which does a `status`
    /// probe to confirm the node is reachable) and wraps it in a
    /// `MinimaAttester`. Exact constructor names and argument shape are
    /// pinned by `minima-attest`'s public API. If this fails to compile
    /// after a `minima-attest` major bump, the binding here is the single
    /// place to update.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] if the node is unreachable, returns a
    /// non-success status, or rejects the credentials.
    #[cfg(feature = "minima-attest")]
    pub async fn attester(
        &self,
        id: impl Into<String>,
    ) -> Result<minima_attest::MinimaAttester, Error> {
        use secrecy::ExposeSecret;
        let cfg = minima_attest::MinimaConfig {
            rpc_url: self.rpc_url.clone(),
            password: Some(self.rpc_password.expose_secret().to_owned()),
            ..minima_attest::MinimaConfig::default()
        };
        let client = minima_attest::MinimaClient::connect(cfg)
            .await
            .map_err(|e| Error::Rpc(format!("MinimaClient connect failed: {e}")))?;
        Ok(minima_attest::MinimaAttester::new(id, client))
    }
}
