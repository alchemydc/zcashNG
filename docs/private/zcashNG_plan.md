# Plan: `z3-lite` — a straightforward Docker Compose harness for running Zebra + Zallet + RPC Router in production

**Status:** Handoff plan. This document is the complete specification for an implementing agent.
**Source repo:** fork of `ZcashFoundation/z3` (`main` branch as of 2026-06-10).
**Informed by:** PR #43 (`feat: introduce platform contract, per-network projects, and integration archetypes`) and the review feedback on it. PR #43 is **not** merged into this fork; we cherry-pick its good ideas and reject its complexity (see §2).

---

## 1. Goal and non-goals

### Goal

A node operator should be able to do this, and nothing more, to run a production mainnet Zcash full node + wallet + unified RPC endpoint with monitoring:

```bash
git clone <fork-url> && cd z3-lite
./setup.sh                      # creates data dirs, generates wallet identity, writes .env
docker compose up -d
```

And answer their #1 question — *"where does my persistent state live, and what do I back up?"* — in the first screen of the README.

### Hard requirements (from the project owner)

1. **One compose file.** No overlays, no `docker-compose.<network>.yml`, no per-network Compose projects, no auto-loaded override conventions.
2. **Mainnet by default** with **`latest`** images of Zebra, Zallet, and the rpc-router. Operators stay current by `docker compose pull && docker compose up -d`.
3. **rpc-router is a first-class citizen** — built/published as an image, runs in every network mode, has a healthcheck, and is the single RPC endpoint operators and downstream apps talk to.
4. **Testnet and regtest supported only if they don't blow up complexity** — via env file, not via compose-file changes. If a network mode needs structural compose changes, it gets cut, not accommodated.
5. **Durable, obvious storage** — chain state, wallet DB, and wallet keys live in operator-visible bind-mounted directories, configurable with a single variable.
6. **Monitoring stack** (Prometheus, Grafana, Alertmanager) included and working out of the box.
7. **All critical documentation in a single `README.md`.**
8. **Zebra's P2P port published** so the node accepts inbound peer connections (PR-43 review finding #1).
9. **No logging-driver override** in compose — respect the operator's Docker daemon logging config (journald, etc.) (review finding #5).
10. **No cookie auth** (review finding #6).

### Non-goals

- Running mainnet + testnet + regtest concurrently on one host with a maintained port-offset matrix. (Possible with two clones; documented in 5 lines, not engineered for.)
- A machine-readable platform contract, JSON Schema, or contract-validation CI gates.
- Serving Zaino / lightwalletd clients. **Zaino is out of scope for this fork** (see §2.2).
- zcashd comparator runs. zcashd is EOL after 6.2.0 (per PR-43 review discussion); it has no place in a forward-looking production harness.
- Jaeger / distributed tracing. Optional later; not in the default stack.

---

## 2. Relationship to z3 `main` and PR #43

### 2.1 What we keep from z3 `main`

- The general service shape of `zebra` and `zallet` (env-driven Zebra config, healthcheck pattern, `cap_drop`/`no-new-privileges` hardening — **minus** the logging override).
- The `observability/` directory: Prometheus config + rules, Grafana provisioning + dashboards, Alertmanager config. Trim Jaeger-specific pieces.
- The `rpc-router/` crate (source stays in-repo; it gains a published image and small hardening changes, §5.4).
- The spirit of `.env.example`: every variable defaulted in the compose file, `.env` optional.

### 2.2 What we drop

| Dropped | Why |
|---|---|
| **Zaino service** | Out of stated scope (Zebra + Zallet + router). Removing it deletes the TLS cert generation, the `configs:` blocks, the gRPC port matrix, and its amd64-only pin. Operators who need lightwalletd service should use a dedicated Zaino deployment. |
| **zcashd service + profile** | EOL software; drags in activation-height env vars, comparator docs, and an amd64-only image. |
| **`cookie-permissions` sidecar** | Exists only because of cookie auth. Cookie auth is removed (§4.3), so the sidecar dies with it. |
| **All git submodules** (`zebra/`, `zallet/`, `zaino/`, `zcashd/`) and all `build:` blocks except rpc-router's | We consume published images. Submodules are the single largest source of fresh-clone friction (the PR-43 live test failed because compose tried to *build* Zebra from the submodule when the image tag didn't match a local image). |
| **Jaeger** | Tracing is not a day-1 operator need; removes 4 published ports and a service dependency chain. |
| **PR #43's contract (`z3-contract.yaml`, schema, 3 CI validators)** | Per the review: overkill for a compose project; operators won't tolerate it. Downstream-integration stability is provided more cheaply by §2.3's "stable names" rule. |
| **PR #43's per-network Compose projects, +10000 port offsets, override auto-load machinery, `setup-network.sh`** | Violates the single-compose-file requirement. |
| **PR #43's `docs/integrations/` archetype guides** | Replaced by one short "Connecting to the stack" README section. |
| **`docs/memory-bank/`** and stale docs | Review finding: stale; delete. |
| **`docker-compose.regtest.yml`** | Regtest folds into the single compose file via env (§6). |

### 2.3 What we adopt from PR #43 (the good ideas, without the machinery)

- **Stable, declared identifiers** — but enforced by convention and a fixed `name:` in the compose file, not by a contract+CI. Set top-level `name: z3` so volumes/network names never depend on the clone directory name. Container names, the Docker network name, and service DNS names are frozen and listed in a README table. Renaming any of them is treated as a breaking change in release notes. That is the entire "contract."
- **Tracked `.example` config files copied into place by setup** — one setup script, not per-network machinery.
- **Regtest activation heights shipped in-tree** so regtest boots on a fresh clone.
- **Multi-arch awareness** — Zebra's image is multi-arch; do not hardcode `platform:` on services whose images are multi-arch.

---

## 3. Repository layout (target state)

```
z3-lite/
├── README.md                  # the single doc (outline in §8)
├── LICENSE
├── docker-compose.yml         # the single compose file
├── .env.example               # tracked; setup.sh copies to .env
├── env/
│   ├── testnet.env            # tracked; small (§6)
│   └── regtest.env            # tracked; small (§6)
├── setup.sh                   # the single setup script (§7)
├── config/
│   ├── zallet.toml.example    # mainnet/testnet wallet config template
│   ├── zallet.regtest.toml.example
│   └── zebra.regtest.toml     # regtest activation heights (tracked as-is, mounted only in regtest)
├── scripts/
│   └── regtest-mine.sh        # mines initial blocks for regtest (only extra script allowed)
├── rpc-router/                # crate kept from z3; gains /health and retry (§5.4)
│   ├── Dockerfile
│   └── src/...
├── observability/
│   ├── prometheus/prometheus.yaml
│   ├── prometheus/rules/
│   ├── alertmanager/alertmanager.yml
│   └── grafana/{provisioning,dashboards}/
└── .github/workflows/
    ├── rpc-router-image.yaml  # builds & pushes ghcr.io/<org>/z3-rpc-router (§5.4)
    └── smoke.yaml             # compose config + regtest boot smoke test (§9)
```

Everything else in current z3 is deleted.

---

## 4. Architecture decisions

### 4.1 Services

| Service | Image | Role | Host exposure (defaults) |
|---|---|---|---|
| `zebra` | `zfnd/zebra:latest` | Full node | `8233:8233` p2p (all interfaces); health `127.0.0.1:8080`; **RPC not published** |
| `zallet` | `zodlinc/zallet:latest` *(verify — §5.2)* | Wallet | **Nothing published** |
| `rpc-router` | `ghcr.io/<org>/z3-rpc-router:latest` (with local `build:` fallback) | Single JSON-RPC front door routing to Zebra/Zallet | `127.0.0.1:8232:8232` |
| `prometheus` | `prom/prometheus:<pinned>` | Metrics | `127.0.0.1:9094` |
| `grafana` | `grafana/grafana:<pinned>` | Dashboards | `127.0.0.1:3000` |
| `alertmanager` | `prom/alertmanager:<pinned>` | Alerts | `127.0.0.1:9093` |

Notes:

- **Only two things touch non-loopback interfaces by default: Zebra p2p (required for inbound peers) and nothing else.** Every RPC/UI port binds `127.0.0.1` by default; the bind address is an env var (`Z3_BIND_ADDR=127.0.0.1`) so operators who front with a reverse proxy or trust their network can widen it deliberately. This is the simple answer to "how do I not leak my wallet RPC to the internet."
- **The rpc-router listens on host port 8232** — the canonical Zcash RPC port — because it *is* the node's RPC endpoint from the operator's perspective. Zebra's direct RPC and Zallet's direct RPC remain reachable inside the Docker network (`zebra:8232`, `zallet:28232`) for debugging via `docker compose exec`.
- **Monitoring images stay version-pinned.** "Latest" applies to the Zcash stack (the thing operators must keep current for consensus); infra images get deliberate bumps. This is consistent with the review (finding #7 was about zebra/zallet/zaino, not Prometheus).
- **Monitoring is on by default.** Use a `monitoring` profile in the compose file, but ship `COMPOSE_PROFILES=monitoring` in `.env.example`, so plain `docker compose up -d` includes it and an operator who wants a bare node comments out one line. One mechanism, no extra flags to remember.

### 4.2 Networking / dependency graph

```
internet ⇄ :8233 zebra ←─ rpc-router ─→ zallet ─→ zebra (RPC, internal)
                 ↑              ↑
            prometheus     127.0.0.1:8232 (operator / apps)
```

- Single bridge network, fixed name `z3` (via top-level `name: z3`; network key `z3_net` → external name `z3_z3_net`... **no**: set `networks: z3_net: name: z3-net` explicitly so the name is literal and stable regardless of project name). Downstream containers attach with `docker run --network z3-net ...`.
- `depends_on`: `zallet` → `zebra: service_healthy`; `rpc-router` → `zebra: service_healthy`, `zallet: service_started` (Zallet healthcheck constraints, §5.3).
- `restart: unless-stopped` everywhere.

### 4.3 Authentication model (kill cookie auth)

- Zebra: `ZEBRA_RPC__ENABLE_COOKIE_AUTH=false` in all modes. Zebra's RPC is never published to the host; the Docker network is its trust boundary. This deletes the shared cookie volume, the chmod sidecar, and the read-the-cookie-out-of-a-volume integration archetype in one stroke.
- Zallet: RPC unauthenticated inside the network (same trust boundary), not published. If Zallet's config requires an auth stanza, setup.sh generates credentials into `config/zallet.toml` from the template and the router is configured with them; but prefer the no-auth-internal mode if Zallet supports it. **Agent: check Zallet's `rpc` config options and pick the simplest working mode; document the choice in the compose file comments.**
- rpc-router: the host-facing surface. Default loopback bind is the day-1 protection. Add **optional** static credentials (`Z3_RPC_USER` / `Z3_RPC_PASSWORD` env; router enforces HTTP Basic if both are set, anonymous if unset). This gives the zcashd-style user/password convention everyone's tooling assumes, with zero mandatory setup. (Small router change, §5.4.)

### 4.4 Storage and keys (operator question #1)

One variable rules everything: **`Z3_DATA_DIR`** (default `./data`). The compose file derives every stateful mount from it:

```
${Z3_DATA_DIR}/zebra        → /home/zebra/.cache/zebra      (chain state, ~300 GB+ mainnet)
${Z3_DATA_DIR}/zallet       → /var/lib/zallet               (wallet DB — CRITICAL, contains funds metadata)
${Z3_DATA_DIR}/prometheus   → /prometheus
${Z3_DATA_DIR}/grafana      → /var/lib/grafana
${Z3_DATA_DIR}/alertmanager → /alertmanager
```

Keys live beside config, not inside container-opaque volumes:

```
config/zallet_identity.txt   (age identity encrypting wallet secrets — generated by setup.sh, mode 0600)
config/zallet.toml           (copied from .example by setup.sh)
```

Rules:

- **Bind mounts, not named volumes.** Named volumes hide state in `/var/lib/docker` and are exactly why "where do I persist the state" keeps being asked. Bind mounts make `Z3_DATA_DIR` the obvious backup/disk-mount target (`Z3_DATA_DIR=/mnt/nvme/z3`).
- `setup.sh` creates the subdirectories with the uid:gid each image runs as (Zebra `10001:10001`, Zallet `1000:1000`, Grafana `472:472`, Prometheus `65534:65534` — **agent: verify each against the actual images** and encode in setup.sh). This replaces `fix-permissions.sh`.
- README backup section: *back up `config/` (tiny, contains keys) and `${Z3_DATA_DIR}/zallet` (wallet); `${Z3_DATA_DIR}/zebra` is re-syncable and optional to back up.*

### 4.5 Logging

Delete the `x-common` logging block entirely. Add a README note: "Container logs use your Docker daemon's default driver. If you use `json-file`, configure rotation in `/etc/docker/daemon.json`," with a 4-line example. Compose must not fight journald users.

### 4.6 Security hardening (keep, lightly)

Keep `security_opt: [no-new-privileges:true]` and `cap_drop: [ALL]` + the minimal `cap_add` sets z3 `main` already worked out per service (Zebra's entrypoint needs `CHOWN, DAC_OVERRIDE, FOWNER, SETUID, SETGID`). These cost operators nothing. Keep them in a shared `x-hardening` anchor (logging removed from it).

---

## 5. Service specifications (what the agent implements)

### 5.1 zebra

- `image: ${Z3_ZEBRA_IMAGE:-zfnd/zebra:latest}`; **no `build:`**, **no `platform:`** (image is multi-arch).
- Env-driven config as in z3 `main`, with changes:
  - `ZEBRA_NETWORK__NETWORK=${Z3_NETWORK:-Mainnet}`
  - `ZEBRA_RPC__LISTEN_ADDR=0.0.0.0:8232` (fixed container port; per-network host port matrices are gone because we don't publish it)
  - `ZEBRA_RPC__ENABLE_COOKIE_AUTH=false`
  - `ZEBRA_METRICS__ENDPOINT_ADDR=0.0.0.0:9999` — **enable Prometheus metrics by default** so the monitoring stack works without edits (verify the env var path against current Zebra config; `metrics.endpoint_addr` in TOML).
  - Health endpoint config as in `main` (`ZEBRA_HEALTH__LISTEN_ADDR=0.0.0.0:8080`, etc.).
  - For regtest only: mount `./config/zebra.regtest.toml` — see §6 for how this works without an overlay.
- Ports:
  - `"${Z3_P2P_PORT:-8233}:${Z3_P2P_PORT:-8233}"` — published on all interfaces. Container-side listen port must match (set `ZEBRA_NETWORK__LISTEN_ADDR=0.0.0.0:${Z3_P2P_PORT:-8233}`) so testnet's 18233 works by changing one env var.
  - `"${Z3_BIND_ADDR:-127.0.0.1}:${Z3_HEALTH_PORT:-8080}:8080"`.
- Healthcheck: as in `main` (`curl -sf http://127.0.0.1:8080/ready`).
- Volume: `${Z3_DATA_DIR:-./data}/zebra:/home/zebra/.cache/zebra`.

### 5.2 zallet

- `image: ${Z3_ZALLET_IMAGE:-zodlinc/zallet:latest}` — **agent: verify the exact ZODL Docker Hub repo/tag** (ZODL took over zcashd/Zallet publishing from ECC; the zcashd image is `zodlinc/zcashd`, so `zodlinc/zallet` is expected). If no `latest` tag exists, use the newest published tag and leave a `TODO` comment; do **not** fall back to the unmaintained `electriccoinco/zallet`.
- No `build:`. Check whether the ZODL image is multi-arch; only add `platform:` if it is amd64-only, and then via `${Z3_ZALLET_PLATFORM:-linux/amd64}` with a README note for arm64 hosts.
- Command/config as in `main`: `--datadir /var/lib/zallet --config /etc/zallet/zallet.toml start`.
- Mounts: data dir per §4.4; `./config/zallet.toml:ro`; `./config/zallet_identity.txt:ro`.
- Validator connection in `zallet.toml`: point at `zebra:8232`, **no cookie** (set the user/password or no-auth mode chosen in §4.3). The `.example` templates must reflect this.
- Healthcheck (review finding #7 — distroless has no shell):
  1. **Preferred:** `test: ["CMD", "/path/to/zallet", "<status-subcommand>"]` if the binary offers any cheap self-check (exec-form CMD needs no shell). Agent: check `zallet --help`.
  2. **Fallback:** no container healthcheck; the router's `/health` (§5.4) reports Zallet reachability, and a Prometheus blackbox-style alert rule fires on router-reported Zallet failure. Document whichever lands.

### 5.3 rpc-router (first-class citizen)

- `image: ${Z3_RPC_ROUTER_IMAGE:-ghcr.io/<org>/z3-rpc-router:latest}` **plus** `build: ./rpc-router` retained as fallback,
  with `pull_policy: always` not set (let operators `docker compose pull`).
- Env: `ZEBRA_URL=http://zebra:8232/`, `ZALLET_URL=http://zallet:28232/`, `RUST_LOG=info`, optional `Z3_RPC_USER`/`Z3_RPC_PASSWORD`.
- Port: `"${Z3_BIND_ADDR:-127.0.0.1}:${Z3_RPC_PORT:-8232}:8232"`.
- Healthcheck: `GET /health` (added below), exec-form against the router binary or a tiny static probe — agent picks based on what the router's base image contains; if the Dockerfile uses distroless, add a `/health` self-probe subcommand to the binary (`rpc-router healthcheck`) and use exec-form CMD.

### 5.4 rpc-router code changes (small, in-repo Rust work)

The current router **exits immediately if either backend is unreachable at startup** (it calls `rpc.discover` on both to build the merged OpenRPC schema). That's unacceptable for production where Zebra restarts or syncs slowly. Required changes, in priority order:

1. **Startup retry loop:** retry backend `rpc.discover` with backoff (e.g., 5s → 60s cap) instead of exiting; serve `503` + log clearly until both schemas load. `restart: unless-stopped` then covers crashes, and `depends_on` ordering stops mattering for correctness.
2. **`GET /health` endpoint:** returns 200 when both backends answered a liveness probe recently, 503 otherwise, with a JSON body `{"zebra": "ok|down", "zallet": "ok|down"}`. This becomes the stack-level health signal (and the Zallet healthcheck surrogate).
3. **Optional HTTP Basic auth** from `Z3_RPC_USER`/`Z3_RPC_PASSWORD` (§4.3).
4. **`healthcheck` self-probe subcommand** if the image ends up distroless.
5. **CI workflow** (`rpc-router-image.yaml`): build multi-arch (amd64+arm64) on push to main + tags, push `ghcr.io/<org>/z3-rpc-router:{latest,<semver>,<sha>}`.

Keep each change minimal; no refactors.

### 5.5 Monitoring stack

- Keep `prometheus`, `grafana`, `alertmanager` services from `main`, modified:
  - Remove the `jaeger` service and its `depends_on` edges.
  - Bind all three UIs to `${Z3_BIND_ADDR:-127.0.0.1}`.
  - Bind-mount data dirs per §4.4.
  - `GF_SECURITY_ADMIN_PASSWORD=${Z3_GRAFANA_PASSWORD:-admin}` with a README warning to change it before widening `Z3_BIND_ADDR`.
- `prometheus.yaml`: scrape jobs for `zebra:9999` (Zebra metrics), `rpc-router` (if/when it exports metrics — otherwise probe `/health` via up-ness), Prometheus itself. Delete Jaeger/spanmetrics jobs.
- Grafana: keep existing provisioning + `zebra_overview.json` dashboard as default home; delete Jaeger datasource; verify dashboards render with only the Prometheus datasource.
- Alert rules (keep/trim from `observability/prometheus/rules/`): Zebra not ready > 10 min (health endpoint), Zebra peer count == 0 > 10 min, sync height stalled > 30 min, router `/health` != 200 > 5 min, disk-space rule commented out with instructions (needs node-exporter, which we deliberately omit — note it as the one suggested add-on in the README).

---

## 6. Network modes without overlays

Mechanism: **one compose file + interchangeable env files.** A network mode is `docker compose --env-file env/<net>.env up -d` (mainnet needs no flag; `.env` is mainnet).

The compose file is written so that *everything* that differs per network is an env var with a mainnet default:

| Variable | Mainnet (default) | `env/testnet.env` | `env/regtest.env` |
|---|---|---|---|
| `Z3_NETWORK` | `Mainnet` | `Testnet` | `Regtest` |
| `Z3_P2P_PORT` | `8233` | `18233` | `18233` (unused; no peers) |
| `Z3_DATA_DIR` | `./data` | `./data-testnet` | `./data-regtest` |
| `COMPOSE_PROJECT_NAME` | `z3` | `z3-testnet` | `z3-regtest` |
| `Z3_ZEBRA_EXTRA_CONFIG` | *(empty file)* | *(empty file)* | `./config/zebra.regtest.toml` |
| `ZEBRA_HEALTH__MIN_CONNECTED_PEERS` | `1` | `1` | `0` |

The one structural trick (and the only one allowed): Zebra always gets a mount

```yaml
- ${Z3_ZEBRA_EXTRA_CONFIG:-./config/zebra.empty.toml}:/etc/zebra/extra.toml:ro
```

where `zebra.empty.toml` is a tracked empty file, and the regtest env points the same mount at the tracked regtest TOML carrying activation heights (`NU5=1, NU6=1, …` — **agent: include every upgrade through the current network protocol level, incl. NU6.1/NU6.2 if Zebra's regtest params require them; this was an open question on PR #43, resolve it by testing**). Confirm how Zebra merges a config file with env vars (env should win; if Zebra only takes one `--config`, mount it as the primary config path in regtest and replicate the few needed settings inside it).

- **Coexistence:** Mainnet+testnet on one host works because `COMPOSE_PROJECT_NAME`, `Z3_DATA_DIR`, and `Z3_P2P_PORT` all differ and nothing else is published off-loopback (loopback ports do collide — testnet env also offsets `Z3_RPC_PORT`, `Z3_GRAFANA_PORT`, etc. by +10000; that's 5 lines in `env/testnet.env`, not an engineered matrix). README covers this in one short subsection.
- **Regtest extras:** `scripts/regtest-mine.sh` (port of `regtest-init.sh`, minus the TLS and Zaino parts): generates Zallet regtest config if missing, mines activation blocks via the router, prints next steps. Must use `docker compose` and fall back to `docker-compose` (PR-43 live-test failure on macOS).
- **Cut rule (restated for the agent):** if regtest ends up needing anything beyond `env/regtest.env`, the two tracked TOMLs, and `regtest-mine.sh`, stop and flag it rather than adding mechanism.

---

## 7. `setup.sh` (the only setup script)

Idempotent; safe to re-run; ~80 lines; POSIX sh. Steps:

1. Detect `docker compose` vs `docker-compose`; fail with a clear message if neither.
2. `cp -n .env.example .env`.
3. Read `Z3_DATA_DIR` from `.env` (default `./data`); `mkdir -p` the per-service subdirs; `chown` to the verified uid:gid map (sudo only if needed; print what it's doing).
4. `cp -n config/zallet.toml.example config/zallet.toml`.
5. Generate `config/zallet_identity.txt` (age identity) if missing — prefer running `rage-keygen`/`zallet generate-mnemonic`-equivalent **inside the zallet image** via `docker run` so the host needs no extra tools (**agent: confirm what Zallet's identity generation actually requires**); `chmod 600`.
6. Print: data location, the backup-these-paths list, and `docker compose up -d`.

`--network testnet|regtest` flag: same steps against the corresponding env file/data dir.

---

## 8. README.md outline (the single doc)

Order matters — storage before anything else after the quick start:

1. **What this is** (3 sentences) + architecture diagram (ASCII, the §4.2 one).
2. **Quick start** (clone / setup.sh / up — the §1 block) + expected sync time note.
3. **Where your data lives & what to back up** (§4.4 table; `Z3_DATA_DIR=/mnt/bigdisk/z3` example; keys warning).
4. **Ports & exposure** (table: what's public (8233), what's loopback, how `Z3_BIND_ADDR` widens it, reverse-proxy hint).
5. **Using the RPC** (one curl example against `127.0.0.1:8232` via the router; note that node + wallet methods share the endpoint; optional user/pass).
6. **Monitoring** (Grafana URL, default creds + change-it warning, what the alerts cover, how to point Alertmanager at email/Slack).
7. **Upgrading** (`docker compose pull && docker compose up -d`; note on pinning via `Z3_*_IMAGE` for operators who want change control).
8. **Testnet & regtest** (`--env-file env/testnet.env`; regtest workflow incl. `regtest-mine.sh`; coexistence note).
9. **Stable names for integrators** (the §2.3 table: network `z3-net`, DNS names `zebra`/`zallet`/`rpc-router`, container ports — replaces PR #43's contract + archetype docs).
10. **Troubleshooting** (5–8 entries: permissions on data dir, p2p port unreachable/NAT, slow sync, router 503 while Zebra syncs, log rotation/daemon.json).
11. **Logging note** (§4.5).

Length target: readable top-to-bottom in ~10 minutes. Anything that doesn't fit gets cut, not moved to a docs/ tree.

---

## 9. CI (minimal)

Two workflows only:

1. **`rpc-router-image.yaml`** — §5.4.5.
2. **`smoke.yaml`** on every PR:
   - `docker compose config` (mainnet, and with each env file) — catches YAML/env drift, replacing PR #43's three validators with one command.
   - `shellcheck setup.sh scripts/*.sh`.
   - Regtest boot test: `setup.sh --network regtest && docker compose --env-file env/regtest.env up -d --wait && scripts/regtest-mine.sh && curl` one node method and one wallet method through the router; assert success. This is the real guardrail — it proves the whole chain (Zebra ← router → Zallet, auth mode, configs) on every change, which is more protection than the contract gave.

No contract validation, no parity checkers, no schema.

---

## 10. Implementation order

1. **Strip:** fork repo; delete submodules, Zaino/zcashd/Jaeger services, cookie sidecar + volume, TLS configs, overlay files, `docs/` (except content folded into README), stale scripts. Get `docker compose config` clean.
2. **Reshape compose:** single file per §4–§5 (latest images, bind mounts via `Z3_DATA_DIR`, p2p published, loopback RPC, no logging block, fixed names, monitoring profile default-on via `.env.example`).
3. **Router hardening:** retry loop, `/health`, optional basic auth, healthcheck probe; image CI.
4. **setup.sh** + verified uid:gid map.
5. **Monitoring rewire:** scrape Zebra metrics + router health; trim dashboards/rules; verify Grafana renders.
6. **Mainnet validation:** fresh VM, quick start verbatim, confirm inbound p2p connections appear (check `getpeerinfo` via router after port-forward), confirm dashboards populate, confirm `docker compose pull` upgrade path.
7. **Testnet/regtest env files** + `regtest-mine.sh`; apply the §6 cut rule honestly.
8. **README** per §8 — written last, against the implemented reality, every command copy-paste-verified.
9. **smoke.yaml.**

## 11. Acceptance criteria

- [ ] Fresh Linux amd64 host: §1 quick start works with zero edits; mainnet syncing; Grafana shows Zebra metrics.
- [ ] Fresh macOS arm64 host: same, no `DOCKER_PLATFORM` gymnastics for Zebra/router (Zallet per §5.2 finding).
- [ ] `ss -tlnp` shows only 8233 on non-loopback interfaces by default.
- [ ] Inbound p2p connections observed on mainnet with the port forwarded.
- [ ] `rm -rf` of the clone (not `Z3_DATA_DIR`) + re-clone + `setup.sh` + `up -d` resumes from existing chain state — proves state durability is real.
- [ ] Killing Zebra mid-run: router returns 503 on `/health`, recovers without restart when Zebra is healthy again; alert fires.
- [ ] Regtest smoke test green in CI on a fresh clone.
- [ ] Exactly one compose file, one setup script (+ regtest-mine), one README. `wc -l docker-compose.yml` ≤ ~250.
- [ ] No `logging:` keys, no `cookie` strings, no submodules in the repo.

## 12. Open questions for the implementing agent to resolve (and report back)

1. Exact ZODL Zallet image name/tags and arch support (§5.2).
2. Zallet's simplest internal auth mode and whether the binary offers a healthcheck-friendly subcommand (§4.3, §5.2).
3. Zebra config-file + env-var merge semantics for the regtest mount trick (§6), and the full regtest activation-height list through current protocol (NU6.1/NU6.2 question from PR #43 review).
4. Current Zebra metrics env var path and the metric names the existing Grafana dashboards expect (`metrics` feature flags in the published image).
5. Whether `zfnd/zebra:latest` tracks stable releases only (if it tracks main/nightly, switch the default to the rolling stable tag, e.g. major-version tag, and note it).