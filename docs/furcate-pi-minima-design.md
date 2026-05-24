# `furcate-pi-minima` — design

Status: **0.1.0 — implemented**.
Standalone Tier 2 crate at
[`github.com/furcateai/furcate-pi-minima`](https://github.com/furcateai/furcate-pi-minima).
Split out of `furcate-pi-hat` on 2026-05-24 for discoverability among the
Minima community (people looking for "Minima on Pi" find this crate without
needing to know about Furcate's GPIO trigger surface).

## Why this crate exists

A productionable Pi-class Furcate deployment anchors receipts into a Minima
full node, and Furcate's needs are narrower and more opinionated than a
general Minima operator's: continuous anchoring of inference receipts with
later inclusion-proof retrieval, on intermittently-connected ARM hardware,
under unattended boot.

`furcate-pi-minima` provides that opinionated layer on top of Minima's
existing Linux deployment story. It runs the upstream Minima `.jar`
unmodified on a Pi, with defaults pre-tuned for the workload Furcate actually
generates and the Pi-class concerns Furcate cares about (secret bootstrap,
archive retention sizing, JRE pinning, RPC loopback, systemd unit shape).
Operators who want different opinions can still hand-roll on top of
Minima upstream — this crate is for the Furcate-shaped deployment.

It is explicitly *not*:

- a fork of Minima
- a reimplementation of any Minima component
- an "Integritas node" — Integritas is hosted SaaS, the Pi crate runs a
  standard Minima full node configured for attestation workloads
- a turnkey OS image

## Relationship to the rest of the bundle

```
  furcate-pi-hat-examples (battery_line_witness)
                │
                ▼ writes via MinimaAttester / MinimaReceiptSink
        ┌───────────────┐
        │ minima-attest │  (Tier 2 client — already shipped)
        └───────┬───────┘
                │ HTTP RPC :9005
                ▼
       ┌────────────────────┐
       │ furcate-pi-minima  │  (this crate — supervises a local Minima node)
       └─────────┬──────────┘
                 │ exec
                 ▼
            minima.jar (upstream, Apache-2.0, fetched on install)
```

`minima-attest` already speaks the right RPC; nothing in it changes. The
Pi crate's only job is to make a healthy Minima node show up on
`127.0.0.1:9005` with appropriate retention so that anchors written today
are still provable months from now.

## Profiles

A single `--profile` flag at `init` time picks the flag set. The crate
ships three.

### `--profile attestor` (default)

The Pi-class anchoring workload. Standard full node plus retention tuned
so historical anchors remain provable.

| Minima flag | Value | Why |
|---|---|---|
| `-server` | (on) | Headless, no MDS web UI by default. |
| `-daemon` | (on) | No stdin; supervised by systemd. |
| `-data` | `/var/lib/furcate-pi-minima/minima` | Standard FHS path. |
| `-port` | `9001` | Base port; RPC lands on +4. |
| `-rpcenable` | (on) | `minima-attest` needs this. |
| `-rpcpassword` | generated | 32-byte hex, persisted `0600`. |
| `-dbpassword` | generated | 32-byte hex, persisted `0600`, **set-once-forever**. |
| `-archive` | (on) | Required: keeps block headers + coin proofs from node start, so anchors remain provable past the 2-month default prune window. |
| `-txpowdbstore` | `9999` | Days of full TxPoW bodies kept. Default 3 days drops the body carrying the `txnstate` write; without this, archive headers alone don't prove `txnstate` content past 3 days. |
| `-mdsenable` | (off) | No MiniDapp surface in the attestor profile — reduces attack surface and RAM. |
| RPC bind | `127.0.0.1` only | RPC is HTTP Basic Auth without TLS by default; never expose. |

This is the default for a reason: the entire Furcate Pi bundle assumes
anchors written today survive into the audit window.

### `--profile minimal`

Vanilla Minima full node. No `-archive`, default `-txpowdbstore`. For
operators who want the smallest disk footprint and don't care about
historical anchor retrieval (e.g. live mesh participation only). Documented
prominently as "anchors older than ~2 months may not be locally provable
from this node — you'll need a peer with archive history."

### `--profile custom`

Pass-through. The flag string is taken from
`/etc/furcate-pi-minima/custom-flags` and appended verbatim. For operators
who need `-megammr`, a MySQL-backed setup, or any combination the crate
doesn't model.

## Subcommand surface

A single binary `furcate-minima`:

```
furcate-minima init [--profile attestor|minimal|custom] [--jar <path>]
furcate-minima start
furcate-minima stop
furcate-minima restart
furcate-minima status
furcate-minima logs [-f]
furcate-minima healthz
furcate-minima verify-jar
```

### `init`

Idempotent first-boot setup. Refuses to overwrite an existing
`/etc/furcate-pi-minima/dbpassword` — losing the dbpassword loses the
wallet, so the crate will not silently rotate it.

Steps:

1. Pre-flight: kernel ≥ 5.10 check, `java -version` present, `systemctl`
   available, `aarch64-unknown-linux-gnu` confirmed, free disk ≥ 4 GiB.
2. Create user `furcate-minima` (no shell, no login), data dir
   `/var/lib/furcate-pi-minima/`, config dir `/etc/furcate-pi-minima/`.
3. Generate 32-byte hex secrets to `dbpassword` and `rpcpassword`, mode
   `0600`, owned by `furcate-minima`.
4. Acquire `minima.jar`:
   - default: download from the pinned GitHub release tag baked into the
     crate, verify SHA256 against the in-crate manifest;
   - `--jar <path>`: skip download, still verify SHA256 against the
     manifest (for offline/air-gapped install).
5. Install `minima.jar` to `/usr/lib/furcate-pi-minima/minima-<version>.jar`.
6. Render `/etc/systemd/system/furcate-minima.service` from a template
   with the profile's flag set baked in.
7. `systemctl daemon-reload && systemctl enable furcate-minima`.
8. Print: "Run `furcate-minima start` when ready. Run
   `furcate-minima healthz` after ~30s to confirm sync."

### `start` / `stop` / `restart`

Thin `systemctl` wraps. Exit code is `systemctl`'s.

### `status`

Two things in one: `systemctl is-active` *and* a Minima RPC `status` call.
A node can be "running" (systemd green) while still cold-syncing or having
lost peer connectivity; reporting only one is misleading. Output is one
table: systemd state, block height, peer count, last-block age, archive on/off.

### `logs`

`journalctl -u furcate-minima --no-pager` (default 100 lines) or `-f` to
follow.

### `healthz`

Same data as `status` but as JSON, exit code 0 / non-zero so it's usable
as a systemd `ExecStartPost` readiness gate, a Cloud probe, or a cron
heartbeat. Definition of "healthy":

- `systemctl is-active` = yes
- RPC `status` returns 200
- `block_height` > 0
- `peer_count` >= 1
- `last_block_age` < 600 s

### `verify-jar`

Re-runs the SHA256 check on the installed jar against the in-crate
manifest. Used by ops to confirm no in-place tampering. Also useful when
upgrading the crate: a new crate version may pin a newer Minima release,
and `verify-jar` reports the mismatch before `restart` blindly relaunches
the old jar.

## Library surface

The same crate exposes a `lib.rs` with one type:

```rust
pub struct LocalMinimaNode {
    pub rpc_url: Url,            // http://127.0.0.1:9005
    pub rpc_password: SecretBox<String>,
}

impl LocalMinimaNode {
    /// Read from the standard install layout. Returns Err if the node
    /// isn't installed or the caller lacks permission to read the
    /// rpcpassword file (group `furcate-minima` is the conventional
    /// answer).
    pub fn from_default_install() -> Result<Self, Error> { ... }

    /// Convenience: build a `minima_attest::MinimaAttester` pointed at
    /// this node. Hides the URL + password plumbing.
    #[cfg(feature = "minima-attest")]
    pub fn attester(&self) -> minima_attest::MinimaAttester { ... }
}
```

`minima-attest`'s clients become a one-liner:

```rust
let attester = LocalMinimaNode::from_default_install()?.attester();
```

That's the only thing the Pi crate adds on the library side. Everything
else is the operator binary.

## Distribution & install

- Published to crates.io as `furcate-pi-minima`.
- The Minima jar is **not** vendored. The crate ships only a SHA256
  manifest pinned to a specific upstream release tag (e.g.
  `v1.0.45 -> <sha256>`). Bumping the supported Minima version = new
  crate release.
- `furcate-minima` binary is installed via `cargo install
  furcate-pi-minima`, then `sudo furcate-minima init` to do the
  privileged steps.
- Optional Debian packaging is a follow-up; not in 0.1.0.

## JRE strategy

Documented as an apt prereq, not bundled:

```
sudo apt install -y openjdk-17-jre-headless
```

`init` refuses to proceed without `java -version` succeeding. Headless
JRE is ~100 MB; bundling Java is a packaging nightmare we won't take on.
Java 8+ works per Minima docs, but the crate's CI tests against 17 and
21 (current LTSes).

## Sharp edges, called out

Each of these gets a paragraph in the README, not buried in docs:

1. **`-dbpassword` is set-once-forever.** Lose the file under
   `/etc/furcate-pi-minima/dbpassword` and you lose the wallet. The
   crate generates and persists it, and refuses to rotate. Operators
   must back up `/etc/furcate-pi-minima/` as part of standard ops.
2. **RPC has no native TLS in the attestor profile.** RPC is bound to
   `127.0.0.1`. If a remote client needs RPC access, the operator must
   front it with a reverse proxy that terminates TLS. The crate will
   not enable `-rpcssl` with a self-signed cert by default — that's a
   security theatre footgun for Pi operators.
3. **Anchor retention is a function of `-archive` + `-txpowdbstore`.**
   The `attestor` profile sets both. Switching from `attestor` to
   `minimal` after the fact does not retroactively prune, but new
   anchors won't be locally provable past the default windows.
4. **Major Minima version upgrades may require resync.** Documented; the
   crate's upgrade path is `stop -> verify-jar -> swap jar -> start`,
   with `verify-jar` flagging the mismatch.
5. **No Integritas-API integration in 0.1.0.** The hosted Integritas
   SaaS API surface is a *client* concern — it belongs in `minima-attest`
   if and when we want to add an alternate sink. The Pi crate runs the
   underlying node, nothing else.

## Why not Docker

The dominant industry pattern (Umbrel, DAppNode, eth-docker, Stereum) is
Docker Compose + bind-mounted data dir. We're choosing systemd instead,
for three reasons specific to this bundle:

1. **The rest of the Pi bundle is systemd.** `furcate-pi-hat-examples`'
   reference deployments run as systemd units. Adding Docker introduces
   a second supervisor model and a Docker daemon on every Pi.
2. **Resource cost.** Docker daemon on a Pi 5 is ~150 MB resident. On a
   2 GB Pi (or a Zero 2 W) that's significant overhead next to a node
   already wanting ~512 MB heap.
3. **Operator familiarity.** Pi operators are more often comfortable
   with `journalctl` than `docker logs`. systemd is the default Debian
   posture and `apt install`-able Java already lives there.

The `--profile custom` escape hatch lets operators who want Docker
generate compose-equivalent flags and run it themselves.

## Open questions before implementation

These don't block the design but should be settled before 0.1.0 ships:

1. **Pinned Minima version for 0.1.0.** Latest stable is v1.0.45
   (2025-03-28). Pin to that, or to whatever is current on the day we
   cut 0.1.0? Recommend: current-on-cut, with a clear changelog rule
   that crate minor bumps may bump the pinned Minima.
2. **Should `init` create a backup of `/etc/furcate-pi-minima/` to a
   user-supplied path?** Probably yes — backup is the single most
   important operator task and the crate is the right place to make it
   trivial.
3. **mDNS announce of the local RPC.** The rest of the Furcate Pi
   bundle uses `furcate-mesh` for peer discovery. Should
   `furcate-pi-minima` announce its RPC via mDNS so the local mesh
   knows it's there? Lean yes, but only behind a feature flag —
   announcing RPC creds-bound endpoints needs careful thought.
4. **Health endpoint as HTTP server.** `healthz` as a CLI returning JSON
   is enough for the first release. A long-lived HTTP server on
   `:9006` answering `/healthz` is a 0.2 feature if operators ask for it.

## Out of scope

- Multi-node clustering / failover
- A web dashboard
- Integritas hosted-API client (belongs in `minima-attest`)
- An OS image (`furcate-pi-image` is its own project)
- Cross-platform: this crate is `target_os = "linux"` and arch-gated to
  `aarch64`, per the no-cross-platform-hedging rule for platform-named
  crates.
