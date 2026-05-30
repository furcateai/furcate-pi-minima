// SPDX-License-Identifier: Apache-2.0

//! High-level subcommand implementations.
//!
//! Each function here corresponds to one `furcate-minima <subcommand>`.
//! The binary `src/bin/furcate-minima.rs` is a thin clap-driven shell
//! that dispatches into these. Keeping the logic in the library makes
//! it integration-testable from `tests/`.
//!
//! Linux-only. The whole module is gated behind `#[cfg(target_os =
//! "linux")]` at the lib.rs level — there's no value in stubbing
//! "init on Darwin" because the supervised binary is the Linux Minima
//! jar running under systemd.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, Paths};
use crate::error::Error;
use crate::jar::{self, PINNED};
use crate::profile::Profile;
use crate::rpc::{self, NodeHealth};
use crate::secrets;
use crate::systemd;

/// `furcate-minima init` — first-boot install.
///
/// Steps (see `docs/furcate-pi-minima-design.md` for full rationale):
/// 1. Pre-flight: Java present, systemctl available, free disk OK,
///    not already installed (refuses to overwrite dbpassword).
/// 2. Create config + data + jar dirs.
/// 3. Generate dbpassword + rpcpassword (32-byte hex, 0600).
/// 4. Write `EnvironmentFile` referencing those secrets.
/// 5. Acquire `minima.jar` (download from pinned release or use
///    `--jar <path>`), SHA256-verify against the in-crate manifest.
/// 6. Write `config.toml`.
/// 7. Render and write the systemd unit.
/// 8. `systemctl daemon-reload && systemctl enable furcate-minima`.
///
/// Idempotent on partial failure: existing secrets are not
/// overwritten; existing config is rewritten (the only mutable piece
/// at install time is profile selection); existing jar is re-verified.
///
/// # Errors
///
/// Bubbles up any failure from the eight steps: preflight, directory
/// creation, secret generation, jar acquisition/verification, config
/// serialisation, env-file write, unit render/write, or systemctl
/// invocation.
pub async fn init(
    profile: Profile,
    jar_override: Option<PathBuf>,
    paths: &Paths,
) -> Result<(), Error> {
    // Step 1: pre-flight.
    preflight().await?;

    // Step 2: directories. mkdir -p semantics — fine to call when
    // they already exist.
    tokio::fs::create_dir_all(&paths.etc).await?;
    tokio::fs::create_dir_all(&paths.var_lib).await?;
    tokio::fs::create_dir_all(&paths.usr_lib).await?;
    tokio::fs::create_dir_all(paths.data_dir()).await?;
    // The systemd unit's parent (`/etc/systemd/system/`) exists by default on
    // any systemd host, but under `--root <prefix>` (integration tests, dry
    // installs) the prefixed parent doesn't pre-exist. mkdir -p it so step 7
    // doesn't ENOENT.
    if let Some(parent) = paths.systemd_unit.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Step 3: secrets. write_secret_no_overwrite is the load-bearing
    // guarantee for -dbpassword (set-once-forever).
    let dbpassword = if paths.dbpassword().exists() {
        tracing::info!("dbpassword already exists; preserving (set-once-forever)");
        secrets::read_secret(&paths.dbpassword())?
    } else {
        let s = secrets::generate_hex_secret();
        secrets::write_secret_no_overwrite(&paths.dbpassword(), &s)?;
        s
    };
    let rpcpassword = if paths.rpcpassword().exists() {
        tracing::info!("rpcpassword already exists; preserving");
        secrets::read_secret(&paths.rpcpassword())?
    } else {
        let s = secrets::generate_hex_secret();
        secrets::write_secret_no_overwrite(&paths.rpcpassword(), &s)?;
        s
    };

    // Step 4: jar.
    let jar_dest = paths.jar(PINNED.version);
    if let Some(src) = jar_override {
        tracing::info!(src = %src.display(), "using operator-supplied jar");
        // Even with --jar, we SHA256-verify against the pinned manifest.
        jar::verify(&src, &PINNED).await?;
        // Copy (not move — operator may want their source untouched)
        // into our managed path.
        tokio::fs::copy(&src, &jar_dest).await?;
    } else if jar_dest.exists() {
        tracing::info!(path = %jar_dest.display(), "jar already installed; re-verifying");
        jar::verify(&jar_dest, &PINNED).await?;
    } else {
        let client = reqwest::Client::builder()
            .user_agent(concat!("furcate-pi-minima/", env!("CARGO_PKG_VERSION"),))
            .build()?;
        tracing::info!(
            url = PINNED.url,
            "downloading minima.jar from pinned release"
        );
        jar::download_and_verify(&PINNED, &jar_dest, &client).await?;
    }

    // Step 5: config.
    let config = Config {
        profile,
        minima_version: PINNED.version.to_string(),
        base_port: 9001,
    };
    let config_text = toml::to_string_pretty(&config)
        .map_err(|e| Error::Preflight(format!("could not serialize config.toml: {e}")))?;
    tokio::fs::write(paths.config_toml(), config_text).await?;

    // Step 6: EnvironmentFile. Same 0600 model as the secrets.
    let env_body = systemd::render_env_file(&config, &dbpassword, &rpcpassword);
    write_mode_0600(&paths.env_file(), &env_body).await?;

    // Step 7: systemd unit.
    let unit = systemd::render_unit(&config, paths);
    systemd::write_unit(paths, &unit).await?;

    // Step 8: daemon-reload + enable.
    run_systemctl(&["daemon-reload"]).await?;
    run_systemctl(&["enable", "furcate-minima"]).await?;

    tracing::info!(
        profile = ?profile,
        minima_version = PINNED.version,
        "furcate-pi-minima installed; run `furcate-minima start` to launch",
    );
    Ok(())
}

async fn preflight() -> Result<(), Error> {
    // Java present.
    let java = tokio::process::Command::new("java")
        .arg("-version")
        .output()
        .await
        .map_err(|e| {
            Error::Preflight(format!(
                "java not found ({e}); install openjdk-17-jre-headless"
            ))
        })?;
    if !java.status.success() {
        return Err(Error::Preflight(
            "java -version returned non-zero; check JRE install".into(),
        ));
    }

    // systemctl present.
    tokio::process::Command::new("systemctl")
        .arg("--version")
        .output()
        .await
        .map_err(|e| Error::Preflight(format!("systemctl not found ({e}); systemd required")))?;
    Ok(())
}

async fn write_mode_0600(path: &Path, body: &str) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    // Two-step write to set the mode without leaving a window where
    // the file is world-readable.
    tokio::fs::write(path, body).await?;
    let perms = std::fs::Permissions::from_mode(0o600);
    tokio::fs::set_permissions(path, perms).await?;
    Ok(())
}

async fn run_systemctl(args: &[&str]) -> Result<(), Error> {
    let status = tokio::process::Command::new("systemctl")
        .args(args)
        .status()
        .await
        .map_err(|e| Error::Systemd(format!("systemctl {args:?}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Systemd(format!(
            "systemctl {args:?} exited with {status}",
        )))
    }
}

/// `furcate-minima start` — `systemctl start furcate-minima`.
///
/// # Errors
///
/// Propagates [`Error::Systemd`] from the underlying `systemctl` call.
pub async fn start() -> Result<(), Error> {
    crate::systemd::systemctl("start").await
}

/// `furcate-minima stop` — `systemctl stop furcate-minima`.
///
/// # Errors
///
/// Propagates [`Error::Systemd`] from the underlying `systemctl` call.
pub async fn stop() -> Result<(), Error> {
    crate::systemd::systemctl("stop").await
}

/// `furcate-minima restart` — `systemctl restart furcate-minima`.
///
/// # Errors
///
/// Propagates [`Error::Systemd`] from the underlying `systemctl` call.
pub async fn restart() -> Result<(), Error> {
    crate::systemd::systemctl("restart").await
}

/// `furcate-minima status` — systemd + RPC combined view.
///
/// Composes `systemctl is-active` with the RPC `status` and `block`
/// calls. If the node isn't running, RPC fields are zeroed and the
/// systemd half is still meaningful.
///
/// # Errors
///
/// Returns [`Error::NotInstalled`] if `config.toml` is missing, or
/// other errors from `config` parsing or the `reqwest::Client`
/// builder. Transient RPC failures are logged and absorbed — they
/// produce a partial `NodeHealth`, not an `Err`.
pub async fn status(paths: &Paths) -> Result<NodeHealth, Error> {
    let config = load_config(paths).await?;
    let systemd_active = systemd::is_active().await.unwrap_or(false);

    // Default zero-filled, archive-from-config — these are the fields
    // we can answer even if the RPC is unreachable.
    let mut health = NodeHealth {
        systemd_active,
        block_height: 0,
        peer_count: 0,
        last_block_age_seconds: 0,
        archive: matches!(config.profile, Profile::Attestor),
        version: String::new(),
    };

    if !systemd_active {
        return Ok(health);
    }

    // RPC half. Best-effort — a transient RPC failure shouldn't
    // crash `status`; it should report what we got and let the
    // operator see the gap.
    let node = match crate::LocalMinimaNode::from_paths(paths) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "could not read RPC credentials");
            return Ok(health);
        }
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        // Minima's RPC HTTP parser is case-sensitive on header names and
        // only accepts `Authorization` (Title-Case). reqwest/hyper write
        // header names lowercased by default (`authorization`), which the
        // node silently rejects with 401. This flag makes hyper emit
        // Title-Case header names so Basic auth is recognised. Verified
        // against a live Minima 1.0.45 node.
        .http1_title_case_headers()
        .build()?;

    match rpc::status(&node.rpc_url, &node.rpc_password, &client).await {
        Ok(s) => {
            health.block_height = s.chain.block;
            health.peer_count = s.network.peer_count();
            health.version = s.version;
        }
        Err(e) => tracing::warn!(error = %e, "rpc status call failed"),
    }
    match rpc::block(&node.rpc_url, &node.rpc_password, &client).await {
        Ok(b) => {
            // `as u64` truncation is fine here: u128 ms epoch only
            // overflows u64 in the year 584554051223, well outside
            // anyone's worry budget.
            #[allow(clippy::cast_possible_truncation)]
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let tip_ms = b.timemilli_u64();
            health.last_block_age_seconds = now_ms.saturating_sub(tip_ms) / 1000;
        }
        Err(e) => tracing::warn!(error = %e, "rpc block call failed"),
    }
    Ok(health)
}

/// `furcate-minima healthz` — same as [`status`] but with exit code
/// driven by [`crate::rpc::NodeHealth::is_healthy`].
///
/// Suitable for systemd `ExecStartPost` readiness gates, cron
/// heartbeats, and container liveness probes.
///
/// # Errors
///
/// Same as [`status`] — non-transient failures bubble up; transient
/// RPC failures are logged and absorbed into a partial `NodeHealth`.
pub async fn healthz(paths: &Paths) -> Result<NodeHealth, Error> {
    status(paths).await
}

/// `furcate-minima verify-jar` — re-runs the SHA256 check against the
/// in-crate manifest.
///
/// Used by ops to confirm no in-place tampering and by the upgrade
/// path to detect a crate/jar mismatch before restarting blindly.
///
/// # Errors
///
/// Returns [`Error::NotInstalled`] if no jar is on disk,
/// [`Error::Preflight`] if the installed version doesn't match the
/// crate-pinned one, and [`Error::JarHashMismatch`] on hash
/// disagreement.
pub async fn verify_jar(paths: &Paths) -> Result<(), Error> {
    let config = load_config(paths).await?;
    let installed = paths.jar(&config.minima_version);
    if !installed.exists() {
        return Err(Error::NotInstalled(installed));
    }
    // Resolve which pinned entry corresponds to the installed version.
    // In 0.1.0 we only ship one pinned version, so config.minima_version
    // must match PINNED.version or the crate / jar are out of step.
    if config.minima_version != PINNED.version {
        return Err(Error::Preflight(format!(
            "installed minima version ({}) does not match crate-pinned version ({}); \
             upgrade crate and re-run `furcate-minima init --reconfigure`",
            config.minima_version, PINNED.version,
        )));
    }
    jar::verify(&installed, &PINNED).await
}

/// `furcate-minima logs [-f]` — thin `journalctl` wrap.
///
/// `exec`-style: replaces the current process with `journalctl` so
/// Ctrl-C and `--no-pager` behavior match operator expectations.
///
/// # Errors
///
/// Returns [`Error::Preflight`] if `journalctl` is unavailable or
/// exits non-zero.
pub async fn logs(follow: bool) -> Result<(), Error> {
    let mut args: Vec<&str> = vec!["-u", "furcate-minima", "--no-pager"];
    if follow {
        args.push("-f");
    } else {
        args.extend(["-n", "100"]);
    }
    let status = tokio::process::Command::new("journalctl")
        .args(&args)
        .status()
        .await
        .map_err(|e| Error::Preflight(format!("journalctl not available: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Preflight(format!("journalctl exited with {status}")))
    }
}

/// Read the on-disk [`Config`] from `paths.config_toml()`. Helper for
/// the subcommands that need to know which profile is installed.
///
/// # Errors
///
/// Returns [`Error::NotInstalled`] if the file is missing, or
/// [`Error::Toml`] / [`Error::Preflight`] for malformed contents.
pub async fn load_config(paths: &Paths) -> Result<Config, Error> {
    let path = paths.config_toml();
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotInstalled(path));
        }
        Err(e) => return Err(e.into()),
    };
    let text = String::from_utf8(bytes)
        .map_err(|e| Error::Preflight(format!("config.toml not UTF-8: {e}")))?;
    Ok(toml::from_str(&text)?)
}
