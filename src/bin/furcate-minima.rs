// SPDX-License-Identifier: Apache-2.0

//! `furcate-minima` — operator CLI for the Pi-class Minima wrapper.
//!
//! Thin clap-driven dispatch into `furcate_pi_minima::ops`. The library
//! holds the logic; this binary's only job is to parse arguments and
//! map error returns into exit codes that operators can script around.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use furcate_pi_minima::{Error, Paths, Profile};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "furcate-minima",
    about = "Pi-class operator wrapper for the Minima full node",
    version
)]
struct Cli {
    /// Override the install layout root. Default is `/` (i.e. the
    /// production paths under `/etc`, `/var/lib`, `/usr/lib`). Used
    /// almost exclusively for integration tests.
    #[arg(long, env = "FURCATE_MINIMA_ROOT", global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// First-boot install: generate secrets, fetch and verify the
    /// Minima jar, render the systemd unit, enable it.
    Init {
        /// Which profile to install.
        #[arg(long, value_enum, default_value_t = ProfileArg::Attestor)]
        profile: ProfileArg,
        /// Use an existing local Minima jar instead of downloading.
        /// Still SHA256-verified against the in-crate manifest.
        #[arg(long)]
        jar: Option<PathBuf>,
    },
    /// `systemctl start furcate-minima`.
    Start,
    /// `systemctl stop furcate-minima`.
    Stop,
    /// `systemctl restart furcate-minima`.
    Restart,
    /// Combined systemd-and-RPC status table.
    Status,
    /// JSON health output with exit code 0 (healthy) or 1 (not).
    Healthz,
    /// Recompute the installed jar's SHA256 and compare to the pinned
    /// manifest.
    VerifyJar,
    /// `journalctl -u furcate-minima` (default last 100 lines).
    Logs {
        /// Follow (`journalctl -f`).
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(clap::ValueEnum, Copy, Clone, Debug)]
#[clap(rename_all = "kebab-case")]
enum ProfileArg {
    Attestor,
    Minimal,
    Custom,
}

impl From<ProfileArg> for Profile {
    fn from(p: ProfileArg) -> Self {
        match p {
            ProfileArg::Attestor => Self::Attestor,
            ProfileArg::Minimal => Self::Minimal,
            ProfileArg::Custom => Self::Custom,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let paths = cli.root.as_ref().map_or_else(Paths::default, Paths::under);

    match run(cli.cmd, &paths).await {
        Ok(code) => code,
        Err(e) => {
            tracing::error!(error = %e, "command failed");
            ExitCode::from(1)
        }
    }
}

async fn run(cmd: Cmd, paths: &Paths) -> Result<ExitCode, Error> {
    use furcate_pi_minima::ops;
    match cmd {
        Cmd::Init { profile, jar } => {
            ops::init(profile.into(), jar, paths).await?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Start => {
            ops::start().await?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Stop => {
            ops::stop().await?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Restart => {
            ops::restart().await?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Status => {
            let health = ops::status(paths).await?;
            print_status_table(&health);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Healthz => {
            let health = ops::healthz(paths).await?;
            println!("{}", serde_json::to_string(&health)?);
            if health.is_healthy() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        Cmd::VerifyJar => {
            ops::verify_jar(paths).await?;
            println!("jar SHA256 matches pinned manifest");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Logs { follow } => {
            ops::logs(follow).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn print_status_table(h: &furcate_pi_minima::rpc::NodeHealth) {
    println!("systemd active:        {}", h.systemd_active);
    println!("block height:          {}", h.block_height);
    println!("peer count:            {}", h.peer_count);
    println!("last block age (s):    {}", h.last_block_age_seconds);
    println!("archive mode:          {}", h.archive);
    println!("healthy:               {}", h.is_healthy());
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,furcate_pi_minima=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
