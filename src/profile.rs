// SPDX-License-Identifier: Apache-2.0

//! Node profiles — packaged Minima flag sets for the three operator
//! intents this crate supports.
//!
//! See `docs/furcate-pi-minima-design.md` for the rationale behind each
//! flag in the [`Profile::Attestor`] default. Two facts that drive the
//! defaults:
//!
//! - Minima's default node prunes blocks after ~2 months, and prunes
//!   full `TxPoW` bodies after `-txpowdbstore` days (default 3). For
//!   anchors written today to remain locally provable months from now,
//!   both windows have to be widened.
//! - Integritas, the hosted `SaaS`, does not require a special node
//!   configuration — there is no published "Integritas-compatible
//!   node" config in Minima's docs or repos. The [`Profile::Attestor`]
//!   flag set is our judgment call, documented as such, not an upstream
//!   contract.

use serde::{Deserialize, Serialize};

/// Which packaged flag set the supervised Minima node runs with.
///
/// Selected at `furcate-minima init` time and persisted to
/// `/etc/furcate-pi-minima/config.toml`. Switching profiles after the
/// fact requires `furcate-minima init --profile <new> --reconfigure`
/// (not yet implemented in 0.1.0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// Default. Tuned for the Furcate attestation workload: long-window
    /// retention so anchors remain locally provable, RPC on loopback,
    /// no MDS web surface.
    #[default]
    Attestor,
    /// Vanilla full node, default flags. Smallest disk footprint.
    /// Anchors older than Minima's default prune windows are not
    /// locally provable from this node.
    Minimal,
    /// Pass-through. The flag string is read from
    /// `/etc/furcate-pi-minima/custom-flags` and appended verbatim.
    /// Used by operators who need flag combinations the crate does
    /// not model (e.g. `-megammr`, MySQL-backed setups).
    Custom,
}

impl Profile {
    /// Render this profile to the Minima CLI flag string that goes into
    /// the systemd unit's `ExecStart=` line.
    ///
    /// `data_dir`, `dbpassword_path`, and `rpcpassword_path` come from
    /// the install layout — the renderer doesn't know paths, only the
    /// flags themselves. The caller substitutes paths into the final
    /// `ExecStart` template.
    #[must_use]
    pub fn flags(self) -> Vec<&'static str> {
        match self {
            Self::Attestor => vec![
                "-server",
                "-daemon",
                "-archive",
                "-txpowdbstore",
                "9999",
                "-rpcenable",
                // -mdsenable intentionally absent
            ],
            Self::Minimal => vec!["-server", "-daemon", "-rpcenable"],
            Self::Custom => Vec::new(),
        }
    }

    /// Human-readable description for `furcate-minima status` output
    /// and `init`'s confirmation prompt.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Attestor => {
                "attestor: archive node, long TxPoW retention, RPC loopback only — tuned for anchoring workloads"
            }
            Self::Minimal => {
                "minimal: vanilla full node, default pruning — anchors past ~2mo not locally provable"
            }
            Self::Custom => "custom: pass-through flags from /etc/furcate-pi-minima/custom-flags",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attestor_includes_archive_and_long_txpowdbstore() {
        let flags = Profile::Attestor.flags();
        assert!(flags.contains(&"-archive"), "attestor must enable -archive");
        let idx = flags
            .iter()
            .position(|f| *f == "-txpowdbstore")
            .expect("attestor must set -txpowdbstore");
        assert_eq!(
            flags[idx + 1],
            "9999",
            "attestor must raise TxPoW retention well past the 3-day default",
        );
    }

    #[test]
    fn minimal_does_not_enable_archive() {
        let flags = Profile::Minimal.flags();
        assert!(
            !flags.contains(&"-archive"),
            "minimal profile is intentionally non-archive",
        );
    }

    #[test]
    fn custom_emits_no_flags() {
        assert!(Profile::Custom.flags().is_empty());
    }

    #[test]
    fn no_profile_enables_mds_by_default() {
        for profile in [Profile::Attestor, Profile::Minimal] {
            assert!(
                !profile.flags().contains(&"-mdsenable"),
                "{profile:?} must not enable the MDS web surface by default",
            );
        }
    }
}
