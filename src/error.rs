// SPDX-License-Identifier: Apache-2.0

//! Crate-wide error type.
//!
//! Every fallible function in `furcate-pi-minima` returns this. The
//! variants are picked for what the operator can actually do about
//! each — `NotInstalled` says "run `furcate-minima init`",
//! `JarHashMismatch` says "the jar on disk doesn't match the pinned
//! release; refusing to start," etc.

use std::path::PathBuf;

/// All errors the crate can surface.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No install found at the expected path — `furcate-minima init`
    /// has not been run.
    #[error("furcate-pi-minima is not installed (missing {0})")]
    NotInstalled(PathBuf),

    /// The current user can't read a `0600` config file (typically
    /// `rpcpassword` / `dbpassword`). Operator should run the binary
    /// as root or add the user to group `furcate-minima`.
    #[error("permission denied reading {0} — try running as root or joining group furcate-minima")]
    PermissionDenied(PathBuf),

    /// `init` refused to overwrite an existing secret.
    ///
    /// `-dbpassword` is set-once-forever in Minima; rotating it loses
    /// the wallet. The crate will not silently overwrite. To force a
    /// fresh install, the operator must move
    /// `/etc/furcate-pi-minima/` aside manually.
    #[error(
        "refusing to overwrite existing secret at {0}; move it aside if you really want a fresh install"
    )]
    SecretAlreadyExists(PathBuf),

    /// Downloaded `.jar` SHA256 does not match the pinned manifest.
    #[error("jar SHA256 mismatch: expected {expected}, got {actual} ({path})")]
    JarHashMismatch {
        /// On-disk jar path that failed verification.
        path: PathBuf,
        /// SHA256 from the in-crate pinned manifest, lowercase hex.
        expected: String,
        /// SHA256 computed from the bytes on disk, lowercase hex.
        actual: String,
    },

    /// Pre-flight check failed (missing Java, kernel too old, etc.).
    #[error("pre-flight check failed: {0}")]
    Preflight(String),

    /// systemd command failed (typically `systemctl is-active` / `start`).
    #[error("systemd: {0}")]
    Systemd(String),

    /// Local Minima RPC returned non-2xx or unparseable JSON.
    #[error("Minima RPC: {0}")]
    Rpc(String),

    /// Transport error from `reqwest`.
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    /// I/O failure.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parse failure (RPC response or config file).
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML parse failure (`config.toml`).
    #[error("TOML: {0}")]
    Toml(#[from] toml::de::Error),
}
