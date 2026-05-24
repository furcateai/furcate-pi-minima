# Minima node — implementation reference

In-tree, primary-source-verified reference for the surfaces
`furcate-pi-minima` (and `minima-attest`, by extension) talks to.
Compiled from `github.com/minima-global/Minima` source and
`github.com/minima-global/docs` on 2026-05-24 against Minima v1.0.45.

This document is the truth. The Minima docs site `docs.minima.global`
is a rendering of `github.com/minima-global/docs` and is sometimes
behind the source code; where they disagree, source wins, and that
divergence is called out in §10.

> Anything marked **UNVERIFIED** below could not be confirmed from
> primary sources and needs a runtime check before it's relied on in
> production code.

## 1. Jar acquisition and invocation

**Current release: `v1.0.45`**, published 2025-03-28.
Asset URL (reproducible — pin to the tag, never to `master`):

```
https://github.com/minima-global/Minima/releases/download/v1.0.45/minima.jar
```

Size: 72,349,133 bytes.

**Published SHA-256** (from the release body — Minima publishes hashes
inline, not as `.sha256` sidecars):

```
241f9429d0ea2599905fe0950e0dde4c59a2cabe3e480cd89b6e24a628cee56a
```

(Upstream prints it as `0x241F...E56A` uppercase. Lowercase, unprefixed
is the standard `sha256sum` form.)

The release labels the hash `minima-1.0.45.15.jar` but the downloaded
filename is `minima.jar` — same bytes, different name. The version
lives in the JAR manifest, not the filename.

**Invocation** — single canonical shape:

```
java -jar /path/to/minima.jar <flags>
```

No required JVM flags. The upstream systemd unit (§6) and Docker image
both invoke with zero JVM tuning. JIT is required so any unit that
sets `MemoryDenyWriteExecute=true` will crash Minima at startup.

**Heap sizing** — Minima publishes no per-node `-Xmx` recommendations.
Total-RAM specs from `content/docs/run-a-node/node-types.mdx`:

| Node type      | Total RAM | Storage |
|----------------|-----------|---------|
| Full (default) | 2 GB      | 2 GB    |
| Archive        | 4 GB      | 50 GB   |
| Mega (MMR)     | 8–16 GB   | 50 GB   |

Pi-class deployment notes: Pi 4 / Pi 5 4 GB running a default full
node, default JVM ergonomics (Temurin 11 container-aware → ~25% of
system RAM) is what upstream tests against. A Mega node is not
Pi-class — even an 8 GB Pi 5 is at the bottom of the band and SD-card
I/O is the bottleneck.

## 2. CLI flag reference

Authoritative source: `src/org/minima/system/params/ParamConfigurer.java`
(the enum that parses every CLI / env / conf-file argument). Defaults
come from `src/org/minima/system/params/GeneralParams.java` initializers.

### 2.1 Flags `furcate-pi-minima` uses

| Flag             | Type        | Default                        | Role |
|------------------|-------------|--------------------------------|------|
| `-data`          | path        | `~/.minima/<MINIMA_BASE_VERSION>` (auto-appends `/1.0`) | Data folder. Minima always appends the base-version subfolder. |
| `-conf`          | path        | unset                          | Config file (`key=value` lines). |
| `-port`          | int         | `9001`                         | Base port. RPC = base + 4. |
| `-rpcenable`     | bool        | off                            | Start RPC HTTP server. |
| `-rpcpassword`   | string      | none                           | Set RPC password (and enable basic-auth). |
| `-rpcssl`        | bool        | off                            | Self-signed TLS on the RPC port. |
| `-rpcclrf`       | bool        | off                            | CRLF headers (NodeJS / strict HTTP/1.1 clients). |
| `-dbpassword`    | string      | `minima` (== no encryption)    | Wallet/SQL AES password. **Set once, forever.** |
| `-server`        | bool        | on                             | Accept inbound P2P. |
| `-isclient`      | bool        | off                            | Refuse inbound P2P. |
| `-daemon`        | bool        | off                            | No stdin (required under systemd). |
| `-archive`       | bool        | off                            | Archive node — keeps cascade + all sync blocks. |
| `-txpowdbstore`  | days (≥3)   | `3`                            | Days of TxPoW retention in H2. Clamped to ≥3. |
| `-basefolder`    | path        | `~/`                           | Backup / restore root. |

### 2.2 Flags called out as sharp edges (do NOT use blindly)

| Flag                | Why it matters |
|---------------------|---------------|
| `-clean`            | **Destructive.** Wipes the data folder on start. Never put in the default unit. |
| `-rpc`              | **No-op since 1.0.x.** Source: `MinimaLogger.log("-rpc is no longer in use...")`. RPC port is always `-port + 4`. The repo's `docker-compose.yml` still passes `-rpc 9002`; it's silently ignored. |
| `-noshutdownhook`   | Disables the JVM SIGTERM handler. Documented only for Android. Never set under systemd — disables the safe-shutdown path. |
| `-megammr`          | Mega MMR node — not Pi-class. Surface only via `Profile::Custom`. |
| `-rpcssl` (alone)   | Self-signed cert. Without client-side public-key pinning, the connection is effectively `-k`. See §4.3. |

### 2.3 `@file` indirection — does not exist

**Minima does not support `-rpcpassword @/path/to/file`** or
`-rpcpassword:file=...` or any other indirection. Every CLI value is
consumed as a literal string and stored directly in `GeneralParams`.

The two supported ways to keep secrets out of `ps`:

1. **`-conf <path>`** — a `key=value` file. Only the path appears in
   `ps`. Example:
   ```ini
   rpcenable=true
   rpcpassword=Longalphanumericsecret123
   dbpassword=Differentalphanumericsecret456
   daemon=true
   data=/var/lib/furcate-pi-minima/minima
   ```
2. **`minima_*` environment variables** (case-insensitive). Whatever
   you can set via `-flag value`, you can set via `minima_flag=value`.
   This is what the Docker image and the recommended systemd
   `EnvironmentFile=` path use.

`furcate-pi-minima` uses `EnvironmentFile=/etc/furcate-pi-minima/minima.env`
(mode 0600) — it's the path with the smallest surface area and matches
the Docker image's idiom.

There is **no enforced file-mode check** on either `-conf` or env
files. Set 0600 yourself.

## 3. Port allocation

From `GeneralParams.java:86-101` and `Minima.java:128-131`:

```
base + 0    = P2P + Maxima        (default 9001) — public inbound
base + 1    = currently unused
base + 2    = MDSFILE             (default 9003) — MDS web UI; HTTPS unless -nosslmds
base + 3    = MDSCOMMAND          (default 9004) — MDS back-channel
base + 4    = RPC                 (default 9005) — JSON-over-HTTP; HTTPS with -rpcssl
```

`firewall-rules.mdx` claims 9004 is "no longer used after v1.0.37"; the
source still binds it. Best assumption: still bound, just doesn't need
to be reachable externally.

**For Pi deployments:** only 9001 should be externally reachable.
9003/9005 bind loopback or LAN-only.

## 4. RPC surface

### 4.1 Transport

`src/org/minima/system/network/rpc/CMDHandler.java`. One TCP connection
per request, closed after the response. Two HTTP shapes are accepted:

- **GET**: command goes in the URL path. URL-decode anything special.
  ```
  GET /status HTTP/1.1
  Authorization: Basic bWluaW1hOjxwYXNzd29yZD4=
  ```
- **POST**: command goes in the request body (`Content-Length` bytes
  read verbatim). Body is then URL-decoded.

There is **no JSON request body**. The "command" is one
terminal-syntax string: `name param1:value1 param2:value2`. Multiple
commands chained with `;`.

URL shape: `http(s)://host:9005/<urlencoded-command>`.

### 4.2 Authentication

HTTP Basic Auth (RFC 7617).

- Username: hardcoded `minima`. Source: `MinimaRPCClient.java:26`.
  Don't try to override.
- Password: value of `-rpcpassword`.
- Without `-rpcpassword`, RPC accepts every request. The check at
  `Authorizer.java:47` is `if(!RPC_AUTHENTICATE || password.equals(...))`
  — auth-disabled short-circuits.

### 4.3 TLS

`-rpcssl` enables a Minima-generated self-signed cert stored under
`<data>/ssl/`. Trust model (from `MinimaTrustManager.java`):

- No `-sslpubkey` set: trust manager accepts any cert (`-k`-equivalent).
- `-sslpubkey <hex>` set: pin against the raw
  `X509Certificate.getPublicKey().getEncoded()` bytes.

For Pi-class loopback RPC, leave TLS off — it adds overhead and gives
no security benefit on 127.0.0.1. If the operator wants remote RPC,
front it with a reverse proxy that terminates a real TLS cert. The
crate intentionally does not enable `-rpcssl` in the `Attestor` profile.

### 4.4 Response envelope

`Command.getJSONReply()` and `CommandRunner.java:315-326`.

**Success:**
```json
{
  "command": "status",
  "params":  { },
  "status":  true,
  "pending": false,
  "response": { /* command-specific */ }
}
```

**Failure:**
```json
{
  "command": "status",
  "params":  { },
  "status":  false,
  "pending": false,
  "error":   "<message>"
}
```

**Pending (write-confirmation flow):**
```json
{
  "command":    "<name>",
  "status":     false,
  "pending":    true,
  "pendinguid": "<uid>",
  "error":      "This command needs to be confirmed and is now pending.."
}
```

Multi-command (`;`-separated) returns a JSON **array** of envelopes.

### 4.5 Commands `furcate-pi-minima` actually calls

#### `status`

`src/org/minima/system/commands/base/status.java`. Optional params:
`clean:true` (GC), `debug:true`, `complete:true` (slower, more fields).

Response shape (verbatim field names from source):

```json
{
  "version":  "1.0.45.15",
  "uptime":   "0 hours 12 mins",
  "locked":   false,
  "length":   234567,
  "weight":   "12345...",
  "minima":   "1000000000.0",
  "coins":    "12345",
  "data":     "/home/minima/.minima/1.0",
  "memory":   { "ram": "412.3MB", "disk": "1.4GB", "files": { ... } },
  "chain": {
    "block":      234500,
    "time":       "Sun May 24 09:15:42 UTC 2026",
    "hash":       "0x...",
    "speed":      "50.000",
    "difficulty": "0x000000FFFF...",
    "size":       96,
    "length":     64,
    "branches":   0,
    "weight":     "...",
    "cascade":    { "start": 234436, "length": 16, "weight": "..." }
  },
  "txpow":   { "mempool": 0, "ramdb": 12, "txpowdb": 4321, "archivedb": 0 },
  "network": { /* peers */ }
}
```

**There is no last-block timestamp in `status`** as a usable epoch.
`chain.time` is a `new Date(...).toString()` — human only. For the
machine-readable timestamp, call **`block`** (§4.5.2) and use
`timemilli`.

#### `block`

`src/org/minima/system/commands/base/block.java`. **No params**, returns
the current tip:

```json
{
  "block":     "234567",
  "hash":      "0x...",
  "timemilli": "1748077284000",
  "date":      "Sun May 24 09:15:42 UTC 2026"
}
```

`timemilli` is unix epoch in milliseconds, as a string. Parse and
compute `(now_ms - timemilli)` for staleness. This is the field
`furcate-pi-minima healthz` uses for `last_block_age_seconds`.

#### `quit`

`src/org/minima/system/commands/base/quit.java`. **The shutdown RPC is
called `quit`, not `shutdown`.** Params:

- `compact:true` — compact all H2 DBs before exit.

`quit` calls the same `Main.shutdown(compact)` path as the JVM SIGTERM
hook. So `systemctl stop` (SIGTERM) is equally safe.

#### Commands `furcate-pi-minima` does NOT call

Owned by `minima-attest`, listed here only for completeness:

- `balance`, `coins`, `scripts` — wallet / coin search
- `coincheck`, `coinexport`, `mmrproof` — anchor proof retrieval / verification
- `backup`, `restore` — operator backup tooling

**Important correction:** `txnstate` is **not** the "anchor a 32-byte
hash" command. It's a transaction-builder helper that sets a single
state variable (port 0–255) on a transaction being constructed in the
local custom-txn DB. Anchoring requires the full
`txncreate` → `txninput` → `txnoutput` → `txnstate` → `txnpost` flow,
where the hash lands in a state variable on a coin or burn output.
`minima-attest` owns this — `furcate-pi-minima` does not need to know it.

## 5. First-boot behaviour

### 5.1 Files Minima creates under `-data`

From `MinimaDB.java:286-440`. `<data>` is `~/.minima/<base-version>` by
default (e.g. `~/.minima/1.0`). On first launch:

```
<data>/
  databases/
    walletsql/   wallet.*.db        (H2, AES-encrypted iff -dbpassword set)
    archivesql/  archive.*.db
    txpowsql/    txpow.*.db
    maximasql/   maxima.*.db
    mdssql/      mds.*.db
  cascade.db
  p2p.db
  mds/                              (MiniDapp filesystem)
  ssl/                              (RPC/MDS self-signed cert + key)
  backup/                           (created by `backup` command)
  restore/                          (created by `restore` command)
  archiverestore/                   (archive resync workspace)
```

Under systemd, stdout/stderr go to journald — no separate file logging
is configured by default.

### 5.2 First-boot prompts

**None.** `-dbpassword <value>` on the command line works
non-interactively. In `-daemon` mode, Minima never reads stdin. The
interactive REPL exists only without `-daemon` and without a `-conf`-
only launch.

**`-dbpassword` must be set on first launch and cannot be changed
later.** The default literal `minima` means no encryption. Generate a
random `-dbpassword` at install time, persist it (mode 0600), back it
up. Losing it = losing the wallet.

### 5.3 SIGTERM safety

`Minima.main()` registers a JVM shutdown hook that calls
`Main.shutdown(false)`. The hook:

1. Marks the wallet shutting-down (stops key gen).
2. Stops all network managers.
3. Stops the TxPoW processor.
4. `MinimaDB.saveAllDB(false)` — flushes every H2 DB.
5. Stops the message processor.

So `systemctl stop furcate-minima` is safe. Slow Pi storage means
`saveAllDB` can take real time; set `TimeoutStopSec=180` minimum in
the unit.

`kill -9` / SIGKILL bypasses the hook and can corrupt H2.

### 5.4 Ready-for-anchoring probe

There is no `sd_notify` and no documented "ready" signal. RPC starts
answering before any chain data exists. A meaningful readiness check
is:

```
status.chain.length > 0
AND status.network.peers >= 1
AND (now_ms - block.timemilli) < 600_000
```

This is the criterion `furcate-pi-minima healthz` implements.

## 6. Upstream systemd unit

From `content/docs/run-a-node/linux-vps-service.mdx`. Two variants are
published; this is the RPC-only one (the closer match to our use case):

```ini
[Unit]
Description=minima

[Service]
User=minima
Type=simple
ExecStart=/usr/bin/java -jar /home/minima/minima.jar \
    -rpcenable -rpcpassword <PASSWORD> -rpcssl \
    -daemon -basefolder /home/minima -data /home/minima/.minima
Restart=always
RestartSec=100

[Install]
WantedBy=multi-user.target
```

**What upstream does NOT set** — relevant to whether
`furcate-pi-minima`'s unit contradicts upstream:

| Directive                | Upstream | `furcate-pi-minima` | Note |
|--------------------------|----------|---------------------|------|
| `Type=notify`            | no       | no                  | Minima doesn't call `sd_notify`. |
| `TimeoutStopSec=`        | default 90s | **180s**         | Pi storage is slow; `saveAllDB` needs time. |
| `KillSignal=SIGTERM`     | default  | explicit            | Document the safe-shutdown path. |
| `SuccessExitStatus=143`  | no       | **yes**             | JVM exits 143 (128 + SIGTERM=15) on clean stop. Without this, systemd marks every stop as failed. |
| `Restart=always`         | yes      | yes                 | |
| `RestartSec=`            | 100      | 10                  | 100 s is conservative for a non-Pi VPS; 10 s is fine for a single-process Pi. |
| `NoNewPrivileges=`       | no       | yes                 | Pure additive hardening. |
| `PrivateTmp=`            | no       | yes                 | Additive. |
| `ProtectSystem=strict`   | no       | yes                 | Additive — Minima only writes to `-data` and `-basefolder`. |
| `ProtectHome=`           | no       | `read-only`         | Additive — daemon runs as `furcate-minima`, not as a real user. |
| `MemoryDenyWriteExecute=`| no       | **`no`**            | Must stay `no` — JVM JIT requires W^X violation. |

The `furcate-pi-minima` unit is additive on top of upstream's recipe —
nothing it sets *contradicts* what upstream blesses. The only
deliberate divergences are `EnvironmentFile=` (for secret hygiene),
`SuccessExitStatus=143` (which upstream simply omits), and the
hardening directives.

## 7. Upgrade procedure

Upstream recipe (`linux-vps-service.mdx`):

```
systemctl stop minima
systemctl disable minima
# back up old jar
mv minima.jar minima.jar_old
# fetch new jar
wget https://github.com/minima-global/Minima/releases/download/<TAG>/minima.jar
# verify the SHA256 against the release page
systemctl daemon-reload
systemctl enable minima
systemctl start minima
```

`furcate-pi-minima`'s upgrade story is:

1. Crate version bump that pins a new Minima tag + SHA256 in
   `jar.rs::PINNED`.
2. Operator runs `cargo install furcate-pi-minima --force` (or apt
   upgrade once we package).
3. `furcate-minima verify-jar` reports the mismatch.
4. `furcate-minima init --reconfigure` re-downloads + verifies + swaps.

Wire compatibility across micro versions is **UNVERIFIED** —
empirically the network has run continuously through 1.0.40 → 1.0.45
with peers on different micros, so minor patches appear non-breaking.
H2 schema migrations are handled internally on first start of a new
version (no operator step).

## 8. Sharp edges

Things that bite operators and inform the crate's design:

1. **`-dbpassword` is set-once-forever.** No rekey command. Source:
   `ParamConfigurer.java:177-180`. `furcate-pi-minima` generates on
   first `init` and refuses to overwrite an existing one.
2. **`-clean` wipes everything.** Never in the default unit; available
   only via an explicit operator command (not in 0.1.0).
3. **`-rpc` is a no-op.** Don't add it to the unit hoping to change
   the RPC port — change `-port` instead and RPC moves with it.
4. **RPC username is hardcoded `minima`.** Don't try to override.
5. **No `@file` indirection** — pass secrets via `EnvironmentFile=`.
6. **No file-mode checks on conf / env files** — set 0600 yourself.
7. **Self-signed TLS without pinning is `-k`.** Either pin
   `-sslpubkey` client-side or terminate TLS in a reverse proxy.
8. **`SuccessExitStatus=143`** is not optional — without it, every
   clean stop is logged as a failure.
9. **`MemoryDenyWriteExecute=true` crashes Minima.** JVM JIT needs
   write-execute pages.
10. **Default full node retains only ~24 h of TxPoW.** Long downtime
    requires `-rescuenode` or full resync. `Attestor` profile's
    `-archive` + `-txpowdbstore 9999` extends this to the practical
    limit.
11. **GitHub repo's `docker-compose.yml` is outdated** — passes
    `-rpc 9002` (no-op) and the Dockerfile only EXPOSEs 9001–9004
    (no 9005). Don't base configuration on it.

## 9. Reference resolutions for the scaffold

Concrete corrections this document forces back into the scaffold:

| Scaffold assumption                                                    | Real behaviour                                                                 | Action |
|------------------------------------------------------------------------|--------------------------------------------------------------------------------|--------|
| `-dbpassword @/etc/...` (file indirection)                             | Not supported. Must use `EnvironmentFile=` with `minima_dbpassword=...`.       | Rewrite `systemd::build_exec_start` and `systemd::render_unit`. Move secrets out of the `ExecStart=` line. |
| `shutdown` RPC                                                         | Command is `quit`. Field name in design doc / module comments was wrong.       | Update `rpc.rs` module doc; no behaviour change (we already prefer SIGTERM). |
| `furcate-pi-minima` would track last-block timestamp from `status`     | `status` doesn't expose it as ms-epoch. Use the `block` RPC's `timemilli`.    | Add `block` to `rpc.rs` alongside `status`. |
| `MemoryDenyWriteExecute=no` (comment said "JVMs JIT")                  | Correct, but reason worth keeping explicit.                                    | Keep as-is. |
| `RestartSec=10` (we picked this)                                       | Upstream uses 100. We diverge intentionally; document why.                     | Add comment in `render_unit`. |
| Pinned SHA256 placeholder `0000...`                                    | Real value is `241f...e56a` for v1.0.45.                                       | Fill in. |
| Pinned URL `.../raw/v1.0.45/jar/minima.jar`                            | Upstream-blessed URL is `.../releases/download/v1.0.45/minima.jar`.            | Update. |
| `-rpcclrf` not mentioned                                               | Needed by some HTTP/1.1 strict clients (incl. our previous `minima-attest` header-case workaround). | Add to `Attestor` profile flags. |

## 10. Where docs.minima.global lags source

Don't rely on the rendered docs for any of these:

- `startup-parameters.mdx` is missing several flags that exist in
  `ParamConfigurer.java` (`-shownetcalls`, `-megaprune*`, `-p2p2`,
  `-rpcclrf`, `-allowallip`, `-publicmds*`, `-rescuenode`,
  `-slavenode`).
- `startup-parameters.mdx` still documents `-rpc <port>`; it's a
  silent no-op in current source.
- `firewall-rules.mdx` says 9004 is "no longer used after v1.0.37" —
  it's still bound by current source.
- No documented "Integritas-compatible node configuration" exists in
  the docs repo or anywhere on `minima-global` GitHub. The
  `Attestor` profile is `furcate-pi-minima`'s judgment call, not an
  upstream contract.

## 11. Primary sources

All from `github.com/minima-global`:

- `Minima/src/org/minima/Minima.java`
- `Minima/src/org/minima/system/params/{ParamConfigurer,GeneralParams}.java`
- `Minima/src/org/minima/system/Main.java`
- `Minima/src/org/minima/system/network/rpc/{CMDHandler,Authorizer}.java`
- `Minima/src/org/minima/utils/MinimaRPCClient.java`
- `Minima/src/org/minima/utils/ssl/MinimaTrustManager.java`
- `Minima/src/org/minima/system/commands/Command.java`
- `Minima/src/org/minima/system/commands/CommandRunner.java`
- `Minima/src/org/minima/system/commands/base/{status,block,quit,balance,mmrproof}.java`
- `Minima/src/org/minima/system/commands/txn/txnstate.java`
- `Minima/src/org/minima/database/MinimaDB.java`
- `Minima/docker/Dockerfile`
- `Minima/releases/tag/v1.0.45`
- `docs/content/docs/run-a-node/{startup-parameters,linux-vps-service,linux-vps-docker,archive-node,mega-node,firewall-rules,node-types,desktop-cli}.mdx`
- `docs/content/docs/development/terminal-commands.mdx`
