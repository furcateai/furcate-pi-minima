# furcate-pi-minima

**A Rust supervisor for running an upstream Minima full node on Pi-class hardware.**

A `cargo install`-able operator wrapper that brings a healthy Minima full
node up on a Pi (or any aarch64 Linux box) with the production-shaped
defaults — systemd unit, persisted secrets, archive retention, SHA256-pinned
jar — already wired. Built for [Furcate](https://github.com/furcateai)
attestation workloads, but the supervisor surface is generic enough that any
Pi-class Minima operator can use it.

## What this crate does

Runs the upstream Apache-2.0 `minima.jar` unmodified (no fork, no
reimplementation) and adds the Pi-class operational layer that Minima's
[official Pi guide](https://docs.minima.global/docs/runanode/runonpi)
leaves to the operator:

- A healthy Minima full node on `127.0.0.1:9005`.
- Long-window retention so receipts anchored today are still locally
  provable months from now (`-archive` + raised `-txpowdbstore`).
- Secret bootstrap (`dbpassword`, `rpcpassword`) done once, persisted at
  `0600`, never silently rotated.
- The Minima `.jar` downloaded with SHA256 verification against a pinned
  upstream release — not vendored.
- A `systemd` unit with the load-bearing directives already correct
  (`SuccessExitStatus=143` for the JVM, `MemoryDenyWriteExecute=no` for
  the JIT, `TimeoutStopSec=180` for clean `MinimaDB.saveAllDB()`).

Complementary to [Integritas](https://integritas.minima.global) — Integritas
is the hosted/middleware attestation product; `furcate-pi-minima` is the
self-sovereign side, every device runs its own L1 node.

## Install

```bash
cargo install furcate-pi-minima        # gets you the `furcate-minima` binary
sudo apt install openjdk-17-jre-headless
sudo furcate-minima init               # generates secrets, fetches+verifies jar, installs systemd unit
sudo furcate-minima start
furcate-minima healthz                 # JSON; exit 0 = healthy
```

## Profiles

| `--profile`  | What it does | When to pick |
|--------------|--------------|--------------|
| `attestor`   | Archive node + raised TxPoW retention + RPC loopback only + no MDS. The default. | Any Furcate Pi deployment that anchors receipts. |
| `minimal`    | Vanilla full node, default flags. Smallest disk footprint. | Mesh participation only; no historical anchor retrieval. |
| `custom`     | Pass-through flags from `/etc/furcate-pi-minima/custom-flags`. | `-megammr`, MySQL-backed setups, anything bespoke. |

## Layout

```
/etc/furcate-pi-minima/
    config.toml                       # Profile, ports, jar version pin.
    dbpassword                        # 0600, set-once-forever.
    rpcpassword                       # 0600, group-readable.
    custom-flags                      # Profile::Custom only.
/var/lib/furcate-pi-minima/minima/    # Minima's -data directory.
/usr/lib/furcate-pi-minima/           # Installed jar.
/etc/systemd/system/
    furcate-minima.service
```

## Sharp edges

- **`-dbpassword` is set-once-forever.** Lose
  `/etc/furcate-pi-minima/dbpassword` and you lose the wallet. The
  crate refuses to overwrite an existing one. Back up
  `/etc/furcate-pi-minima/` as part of standard ops.
- **RPC has no TLS in the attestor profile.** Bound to `127.0.0.1`. If
  you need remote RPC, terminate TLS in a reverse proxy — don't enable
  `-rpcssl` with a self-signed cert.
- **No retroactive retention.** Switching `minimal` → `attestor` won't
  un-prune old data. Pick the profile at `init` time.
- **The Minima jar is downloaded on install, not vendored.** Air-gapped
  installs use `--jar <path>`; the SHA256 check still runs.

## Notes for general Pi-class Minima operators

This crate exists to serve Furcate's attestation workload, but a few
pieces of the install are not Furcate-specific and may be useful to
anyone running Minima on a Pi:

- **Secrets via `EnvironmentFile=`, not argv.** Minima accepts
  `-dbpassword <value>` / `-rpcpassword <value>` on the command line,
  but `minima_dbpassword` / `minima_rpcpassword` env vars work too and
  keep the values out of `ps`. The generated unit uses the env-var path.
- **`SuccessExitStatus=143`.** The JVM exits 143 on `SIGTERM`. Without
  this directive every clean `systemctl stop` is recorded as a failure.
- **`MemoryDenyWriteExecute=no`** (explicit, with a comment). The
  systemd hardening default trips the JVM's JIT, which needs W^X
  pages.
- **`TimeoutStopSec=180`.** `MinimaDB.saveAllDB()` can take real time on
  Pi-class storage; the systemd default of 90s is sometimes too short
  and a mid-save kill risks H2 database corruption.
- **SHA256-pinned jar acquisition.** The crate fetches a tagged release
  asset (not `master`) and verifies against an in-crate hash before
  install.

`docs/minima-reference.md` writes these up against primary sources
(`status.java`, `block.java`, `ParamConfigurer.java`, the upstream
`Minima.service`) for anyone who wants the receipts.

## Library use

`minima-attest` clients that want to talk to the locally supervised
node:

```rust
use furcate_pi_minima::LocalMinimaNode;

let node = LocalMinimaNode::from_default_install()?;
let attester = node.attester()?;        // requires `minima-attest` feature
```

`attester()` is gated by the `minima-attest` Cargo feature so the
supervisor binary doesn't pull the client crate when it isn't needed.

## Where the design lives

See `docs/furcate-pi-minima-design.md` at the repository root for:

- the full flag rationale (`-archive` + `-txpowdbstore` vs. `-megammr`)
- why systemd instead of Docker
- the JRE strategy
- open questions before 0.1.0 ships

## License

Apache-2.0. The supervised `minima.jar` is Apache-2.0 upstream code,
fetched from `github.com/minima-global/Minima` at install time.
