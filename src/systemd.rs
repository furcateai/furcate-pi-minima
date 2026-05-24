// SPDX-License-Identifier: Apache-2.0

//! systemd unit rendering and supervision.
//!
//! `furcate-pi-minima` writes the unit at install time and shells out to
//! `systemctl` for every subsequent supervisor operation. We don't link
//! against `libsystemd`:
//!
//! - the operator surface is small (`start` / `stop` / `restart` /
//!   `is-active` / `journalctl`),
//! - `systemctl`'s output is the operator's mental model anyway,
//! - and avoiding `libsystemd-dev` keeps the build dependency surface
//!   on the Pi to just a JRE.
//!
//! ## Why this unit looks the way it does
//!
//! The shape is constrained by load-bearing facts about the upstream
//! Minima jar that don't show up in any single doc page. See
//! `docs/minima-reference.md` for primary-source verification of each:
//!
//! - **No `@file` indirection on `-rpcpassword` / `-dbpassword`.** Every
//!   CLI value is consumed literally; passwords on the CLI land in `ps`
//!   output. The only way to keep them out is `EnvironmentFile=` with
//!   `minima_*` env vars (case-insensitive — the parser lowercases).
//! - **`SuccessExitStatus=143`** is required: the JVM exits with 143
//!   (128 + SIGTERM=15) on a clean shutdown. Without this, systemd
//!   logs every clean `systemctl stop` as a failure.
//! - **`MemoryDenyWriteExecute=` must stay off.** JVM JIT requires
//!   write-execute pages; turning this on crashes Minima at startup.
//! - **`TimeoutStopSec=180`** — `MinimaDB.saveAllDB()` can take real
//!   time on Pi-class storage, and being killed mid-save risks H2 db
//!   corruption.
//!
//! See `docs/minima-reference.md` §6 for the full upstream-vs-ours diff.

use crate::config::{Config, Paths};
use crate::error::Error;

/// Render `furcate-minima.service` to a string.
///
/// Secrets do **not** appear in this output — they live in a separate
/// `EnvironmentFile=` (rendered by [`render_env_file`]) that this unit
/// references. That file's mode is 0600 and group is `furcate-minima`.
#[must_use]
pub fn render_unit(config: &Config, paths: &Paths) -> String {
    let exec_start = build_exec_start(config, paths);
    format!(
        r#"# /etc/systemd/system/furcate-minima.service
# Managed by furcate-pi-minima. Do not edit by hand — regenerate via
# `furcate-minima init --reconfigure`.
#
# Profile: {profile:?}
# Secrets: read from EnvironmentFile (NOT on the ExecStart line —
# Minima has no @file indirection, so any password on argv would land
# in `ps` output).
# JVM: no -Xmx; Temurin's container-aware ergonomics handle this on Pi.

[Unit]
Description=Minima full node (Furcate Pi-class wrapper, profile={profile:?})
Documentation=https://github.com/furcateai/furcate-pi-hat
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=furcate-minima
Group=furcate-minima
WorkingDirectory={data_dir}
EnvironmentFile={env_file}
ExecStart={exec_start}
KillSignal=SIGTERM
# JVM clean-shutdown exit code = 128 + SIGTERM(15). Without this,
# every `systemctl stop` reads as "failed" in journalctl.
SuccessExitStatus=143
TimeoutStopSec=180
Restart=always
# Upstream Minima uses 100s; on a single-process Pi 10s is plenty and
# gets the operator a quick recovery from transient failures.
RestartSec=10s
LimitNOFILE=65536

# Hardening — additive on top of Minima's upstream recipe (which sets
# none of these). Verified against the JVM requirements:
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={data_dir} {etc}
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RestrictNamespaces=yes
LockPersonality=yes
# Must stay `no` — JVM JIT requires W^X-violating page mappings.
MemoryDenyWriteExecute=no

[Install]
WantedBy=multi-user.target
"#,
        profile = config.profile,
        data_dir = paths.data_dir().display(),
        etc = paths.etc.display(),
        env_file = paths.env_file().display(),
        exec_start = exec_start,
    )
}

/// Render the `EnvironmentFile=` body.
///
/// Mode 0600, written next to `config.toml`. Contains `minima_*` env
/// vars that Minima's `ParamConfigurer` parses (case-insensitive,
/// lowercased — see `docs/minima-reference.md` §2.3).
///
/// Secrets are passed in by reference, not stored in the unit. The
/// caller (`ops::init`) writes this file with the freshly generated
/// `dbpassword` and `rpcpassword`.
#[must_use]
pub fn render_env_file(_config: &Config, dbpassword: &str, rpcpassword: &str) -> String {
    // Order matters only for readability. The parser is order-
    // independent. Every key is lowercase per Minima's convention.
    format!(
        "# /etc/furcate-pi-minima/minima.env\n\
         # Mode 0600, group furcate-minima. Managed by furcate-pi-minima;\n\
         # do not edit. Contents control the supervised Minima node.\n\
         minima_dbpassword={dbpassword}\n\
         minima_rpcpassword={rpcpassword}\n",
    )
}

fn build_exec_start(config: &Config, paths: &Paths) -> String {
    let jar = paths.jar(&config.minima_version);
    let mut parts: Vec<String> = vec![
        "/usr/bin/java".into(),
        "-jar".into(),
        jar.display().to_string(),
        "-daemon".into(),
        "-data".into(),
        paths.data_dir().display().to_string(),
        "-port".into(),
        config.base_port.to_string(),
    ];

    // Profile-specific flags. Note: -rpcenable is in every profile so
    // `furcate-minima healthz` works; the password is supplied via the
    // EnvironmentFile (`minima_rpcpassword=...`), not here.
    for f in config.profile.flags() {
        parts.push((*f).to_string());
    }

    // Profile::Custom additions live in `custom-flags`; the systemd
    // unit reads them at install time and bakes them into the ExecStart.
    // (We don't read at runtime — the unit is static once installed.)

    parts.join(" ")
}

/// `systemctl <action> furcate-minima`. Thin wrapper.
///
/// # Errors
///
/// Returns [`Error::Systemd`] if `systemctl` cannot be invoked or
/// returns a non-zero exit code.
pub async fn systemctl(action: &str) -> Result<(), Error> {
    use tokio::process::Command;
    let status = Command::new("systemctl")
        .arg(action)
        .arg("furcate-minima")
        .status()
        .await
        .map_err(|e| Error::Systemd(format!("failed to invoke systemctl: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Systemd(format!(
            "systemctl {action} furcate-minima exited with {status}",
        )))
    }
}

/// Returns true iff `systemctl is-active furcate-minima` returns
/// exit code 0.
///
/// # Errors
///
/// Returns [`Error::Systemd`] if `systemctl` cannot be invoked at all
/// (binary missing, permission denied, etc.). A non-zero exit is
/// reported as `Ok(false)`, since `is-active` uses exit code 3 to
/// mean "inactive" — which is information, not an error.
pub async fn is_active() -> Result<bool, Error> {
    use tokio::process::Command;
    let status = Command::new("systemctl")
        .args(["is-active", "furcate-minima"])
        .status()
        .await
        .map_err(|e| Error::Systemd(format!("failed to invoke systemctl is-active: {e}")))?;
    // `is-active` returns 0 for active, 3 for inactive, others for
    // errors. We only care active/not.
    Ok(status.success())
}

/// Write the rendered unit to `paths.systemd_unit`, with mode 0644.
///
/// Does not call `daemon-reload` — `ops::init` batches the reload with
/// the other setup steps.
///
/// # Errors
///
/// Returns [`Error::Io`] if the unit file can't be written or its
/// mode can't be set.
pub async fn write_unit(paths: &Paths, unit: &str) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::write(&paths.systemd_unit, unit).await?;
    let perms = std::fs::Permissions::from_mode(0o644);
    tokio::fs::set_permissions(&paths.systemd_unit, perms).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;

    fn fixture_config() -> Config {
        Config {
            profile: Profile::Attestor,
            minima_version: "v1.0.45".into(),
            base_port: 9001,
        }
    }

    #[test]
    fn attestor_unit_includes_archive_flag() {
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(
            unit.contains("-archive"),
            "attestor unit must pass -archive"
        );
        assert!(
            unit.contains("-txpowdbstore 9999"),
            "attestor must raise TxPoW retention"
        );
    }

    #[test]
    fn unit_does_not_leak_secrets_into_exec_start() {
        // Critical: passwords must come from EnvironmentFile, NOT argv.
        // If they leak into ExecStart they appear in `ps -ef`.
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(
            !unit.contains("-dbpassword"),
            "ExecStart must not carry -dbpassword"
        );
        assert!(
            !unit.contains("-rpcpassword"),
            "ExecStart must not carry -rpcpassword"
        );
        // The env file reference is the way:
        assert!(unit.contains("EnvironmentFile="));
    }

    #[test]
    fn unit_runs_as_dedicated_user() {
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(unit.contains("User=furcate-minima"));
        assert!(unit.contains("Group=furcate-minima"));
    }

    #[test]
    fn unit_sets_success_exit_status_143_for_jvm_sigterm() {
        // Without this, every clean systemctl stop is logged as a
        // failure (JVM exits 143 on SIGTERM by default).
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(unit.contains("SuccessExitStatus=143"));
    }

    #[test]
    fn unit_does_not_enable_memory_deny_write_execute() {
        // Setting this `yes` crashes the JVM at startup (JIT needs
        // W^X-violating pages). Comment + value must be explicit so
        // it's not "improved" by a well-meaning future edit.
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(unit.contains("MemoryDenyWriteExecute=no"));
    }

    #[test]
    fn unit_extends_timeout_stop_for_pi_storage() {
        let unit = render_unit(&fixture_config(), &Paths::default());
        assert!(unit.contains("TimeoutStopSec=180"));
    }

    #[test]
    fn unit_hardening_directives_present() {
        let unit = render_unit(&fixture_config(), &Paths::default());
        for directive in [
            "NoNewPrivileges=yes",
            "PrivateTmp=yes",
            "ProtectSystem=strict",
            "ProtectHome=read-only",
            "ProtectKernelTunables=yes",
        ] {
            assert!(unit.contains(directive), "missing hardening: {directive}");
        }
    }

    #[test]
    fn minimal_unit_does_not_include_archive() {
        let mut config = fixture_config();
        config.profile = Profile::Minimal;
        let unit = render_unit(&config, &Paths::default());
        assert!(!unit.contains("-archive"));
    }

    #[test]
    fn env_file_renders_lowercase_minima_prefix_keys() {
        // Minima's ParamConfigurer lowercases env var names before
        // matching against the param table, but lower-case is the
        // documented convention. Either works; we stick to the
        // convention.
        let env = render_env_file(&fixture_config(), "dbsecret123", "rpcsecret456");
        assert!(env.contains("minima_dbpassword=dbsecret123"));
        assert!(env.contains("minima_rpcpassword=rpcsecret456"));
    }
}
