// SPDX-License-Identifier: Apache-2.0

//! Install layout and persisted configuration.
//!
//! ## Layout
//!
//! ```text
//! /etc/furcate-pi-minima/
//!     config.toml         # Profile, ports, jar version pin.
//!     dbpassword          # 0600, set-once-forever (losing this loses
//!                         # the wallet).
//!     rpcpassword         # 0600, group-readable by furcate-minima.
//!     custom-flags        # Profile::Custom only.
//! /var/lib/furcate-pi-minima/
//!     minima/             # Minima's -data directory.
//! /usr/lib/furcate-pi-minima/
//!     minima-<version>.jar
//! /etc/systemd/system/
//!     furcate-minima.service
//! ```
//!
//! All paths are FHS-conformant: config under `/etc`, mutable state
//! under `/var/lib`, jars under `/usr/lib`, unit under
//! `/etc/systemd/system`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::profile::Profile;

/// Filesystem paths the crate reads from and writes to.
///
/// [`Paths::default`] returns the production layout. Tests construct
/// custom [`Paths`] under `tempfile::TempDir` to exercise the install
/// logic in isolation.
#[derive(Clone, Debug)]
pub struct Paths {
    /// Config root — `/etc/furcate-pi-minima` in production. Holds
    /// `config.toml`, `dbpassword`, `rpcpassword`, `minima.env`,
    /// `custom-flags`.
    pub etc: PathBuf,
    /// Mutable state root — `/var/lib/furcate-pi-minima` in production.
    /// Holds the `minima/` data directory used by `-data`.
    pub var_lib: PathBuf,
    /// Read-only artefacts root — `/usr/lib/furcate-pi-minima` in
    /// production. Holds the pinned `minima-<version>.jar`.
    pub usr_lib: PathBuf,
    /// Path to the systemd unit file —
    /// `/etc/systemd/system/furcate-minima.service` in production.
    pub systemd_unit: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            etc: PathBuf::from("/etc/furcate-pi-minima"),
            var_lib: PathBuf::from("/var/lib/furcate-pi-minima"),
            usr_lib: PathBuf::from("/usr/lib/furcate-pi-minima"),
            systemd_unit: PathBuf::from("/etc/systemd/system/furcate-minima.service"),
        }
    }
}

impl Paths {
    /// Path to the persisted `config.toml`.
    #[must_use]
    pub fn config_toml(&self) -> PathBuf {
        self.etc.join("config.toml")
    }
    /// Path to the `dbpassword` file (wallet DB key, set-once-forever).
    #[must_use]
    pub fn dbpassword(&self) -> PathBuf {
        self.etc.join("dbpassword")
    }
    /// Path to the `rpcpassword` file (HTTP Basic Auth credential).
    #[must_use]
    pub fn rpcpassword(&self) -> PathBuf {
        self.etc.join("rpcpassword")
    }
    /// Path to `custom-flags` (whitespace-separated pass-through flags,
    /// read by `Profile::Custom` only).
    #[must_use]
    pub fn custom_flags(&self) -> PathBuf {
        self.etc.join("custom-flags")
    }
    /// systemd `EnvironmentFile=` body — `minima_dbpassword=` /
    /// `minima_rpcpassword=` go here. Mode 0600, group
    /// `furcate-minima`. Written by `ops::init` from the freshly
    /// generated secrets; the systemd unit references it by path so
    /// the actual values never appear in `ps`.
    #[must_use]
    pub fn env_file(&self) -> PathBuf {
        self.etc.join("minima.env")
    }
    /// Minima's `-data` directory.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.var_lib.join("minima")
    }
    /// Path to the installed jar, parametrised by the pinned upstream
    /// version (e.g. `minima-v1.0.45.jar`).
    #[must_use]
    pub fn jar(&self, minima_version: &str) -> PathBuf {
        self.usr_lib.join(format!("minima-{minima_version}.jar"))
    }

    /// Build a [`Paths`] rooted under an arbitrary prefix.
    ///
    /// Used by integration tests to install the entire layout under a
    /// `TempDir`, e.g. `Paths::under("/tmp/test-install")` produces
    /// `/tmp/test-install/etc/furcate-pi-minima`, etc.
    #[must_use]
    pub fn under(prefix: impl AsRef<Path>) -> Self {
        let prefix = prefix.as_ref();
        Self {
            etc: prefix.join("etc/furcate-pi-minima"),
            var_lib: prefix.join("var/lib/furcate-pi-minima"),
            usr_lib: prefix.join("usr/lib/furcate-pi-minima"),
            systemd_unit: prefix.join("etc/systemd/system/furcate-minima.service"),
        }
    }
}

/// `/etc/furcate-pi-minima/config.toml` schema.
///
/// Written once at `init` time, read by every subsequent invocation of
/// the binary plus by [`crate::LocalMinimaNode::from_default_install`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Which profile this install was bootstrapped with.
    pub profile: Profile,
    /// Pinned upstream Minima release tag (e.g. `"v1.0.45"`).
    pub minima_version: String,
    /// Base port. Minima's other ports are derived: P2P = base + 0,
    /// MDS = base + 2, RPC = base + 4.
    #[serde(default = "default_base_port")]
    pub base_port: u16,
}

const fn default_base_port() -> u16 {
    9001
}

impl Config {
    /// Computed RPC port. Always `base_port + 4` per Minima's port
    /// allocation scheme.
    #[must_use]
    pub const fn rpc_port(&self) -> u16 {
        self.base_port + 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_under_prefix_compose() {
        let p = Paths::under("/tmp/x");
        assert_eq!(
            p.config_toml(),
            PathBuf::from("/tmp/x/etc/furcate-pi-minima/config.toml")
        );
        assert_eq!(
            p.jar("v1.0.45"),
            PathBuf::from("/tmp/x/usr/lib/furcate-pi-minima/minima-v1.0.45.jar")
        );
    }

    #[test]
    fn rpc_port_is_base_plus_four() {
        let c = Config {
            profile: Profile::Attestor,
            minima_version: "v1.0.45".into(),
            base_port: 9001,
        };
        assert_eq!(c.rpc_port(), 9005);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let c = Config {
            profile: Profile::Attestor,
            minima_version: "v1.0.45".into(),
            base_port: 9001,
        };
        let s = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.profile, Profile::Attestor);
        assert_eq!(back.minima_version, "v1.0.45");
        assert_eq!(back.base_port, 9001);
    }
}
