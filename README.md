# zcashNG

A single-compose-file Zcash full-node stack: **Zebra** + **Zallet** + a unified **JSON-RPC router** + monitoring. Mainnet by default. Designed to be the smallest sensible setup for running a Zcash node + wallet behind one operator-facing endpoint.

```
                  ┌──── inbound peers ──── :8233 (all interfaces)
                  ▼
  ┌─────────┐    ┌──────────────┐
  │ zebra   │◄───┤ rpc-router   │◄── 127.0.0.1:8232  (operator / apps)
  └────┬────┘    └──┬───────────┘
       │            ▼
       │         ┌────────┐
       └────────►│ zallet │   (no host exposure)
                 └────────┘
       │
       ▼
  ┌────────────┐  ┌─────────┐  ┌──────────────┐
  │ prometheus │◄─┤ grafana │  │ alertmanager │
  └────────────┘  └─────────┘  └──────────────┘
       127.0.0.1:9094  3000        9093
```

## Quick start

```bash
git clone https://github.com/alchemydc/zcashNG && cd zcashNG
./setup.sh
docker compose up -d
```

Mainnet sync takes 6–24 hours depending on disk speed. While syncing, the rpc-router returns `503` on `/` and `/health`; once both Zebra and Zallet have responded, it serves traffic. Watch progress in Grafana at <http://127.0.0.1:3000> (admin / admin — change it; see below).

Requirements:
- Docker Engine 20.10+ with the `docker compose` v2 plugin (or legacy `docker-compose`).
- ~50 GB free disk for the initial mainnet snapshot; ~500 GB+ over the long run.
- An age identity tool to generate the wallet identity key (`brew install rage` on macOS, `apt install age` on Debian/Ubuntu). `setup.sh` will tell you if it's missing.

## Where your data lives & what to back up

One env var controls every stateful path: **`ZCASHNG_DATA_DIR`** (default `./data`). Edit `.env` to point it at the disk you want, e.g. `ZCASHNG_DATA_DIR=/mnt/nvme/zcashng`. `setup.sh` creates the per-service subdirectories with the right ownership.

| Path | Holds | Back up? |
|------|-------|----------|
| `config/zallet_identity.txt` | Age key encrypting wallet secrets — **lose this and the wallet is unrecoverable**. | **Yes**, immediately. Tiny. |
| `config/zallet.toml` | Wallet config; not secret but operator-edited. | Yes. Tiny. |
| `${ZCASHNG_DATA_DIR}/zallet` | Wallet database (notes, accounts, mnemonic). | **Yes**, on a schedule. |
| `${ZCASHNG_DATA_DIR}/zebra` | Chain state. | Optional — re-syncable from peers. Backing it up saves resync time after disk failure. |
| `${ZCASHNG_DATA_DIR}/{prometheus,grafana,alertmanager}` | Monitoring state (metrics history, dashboard settings). | Optional. |

The wallet identity is the only secret; the wallet database without it is useless. Keep them at the same backup cadence.

## Ports & exposure

| Port | Service | Host bind by default |
|------|---------|----------------------|
| 8233 | Zebra p2p | `0.0.0.0` — required so inbound peers can reach you |
| 8232 | rpc-router (JSON-RPC) | `127.0.0.1` |
| 8080 | Zebra `/ready` healthcheck | `127.0.0.1` |
| 9094 | Prometheus UI | `127.0.0.1` |
| 3000 | Grafana | `127.0.0.1` |
| 9093 | Alertmanager | `127.0.0.1` |

Zebra's RPC (container port 8232) and Zallet's RPC (container port 28232) are **not** exposed to the host — they live inside the `zcashng-net` Docker network and are reachable only via the rpc-router or `docker compose exec`.

To widen non-p2p ports beyond loopback, set `ZCASHNG_BIND_ADDR=0.0.0.0` (or your LAN address) in `.env`. **Set a Grafana admin password (`ZCASHNG_GRAFANA_PASSWORD`) and enable rpc-router Basic auth (`ZCASHNG_RPC_USER` + `ZCASHNG_RPC_PASSWORD`) before doing so** — the loopback bind is the only access control by default.

If you're fronting the stack with a reverse proxy (Caddy, nginx, Traefik), leave `ZCASHNG_BIND_ADDR=127.0.0.1` and have the proxy connect to `127.0.0.1:8232` / `127.0.0.1:3000`.

## Using the RPC

Both node and wallet methods come through `http://127.0.0.1:8232/`. The router merges Zebra's and Zallet's OpenRPC schemas and dispatches by method name.

```bash
# Node method
curl -sS -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"getblockchaininfo","params":[],"id":1}' \
  http://127.0.0.1:8232/

# Wallet method
curl -sS -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"z_listaccounts","params":[],"id":1}' \
  http://127.0.0.1:8232/

# Discover everything the router serves
curl -sS -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"rpc.discover","params":[],"id":1}' \
  http://127.0.0.1:8232/ | jq '.methods[].name'
```

Optional HTTP Basic auth on the operator-facing port: set both `ZCASHNG_RPC_USER` and `ZCASHNG_RPC_PASSWORD` in `.env`. Anonymous access is rejected with 401 when both are set; `/health` always remains anonymous so monitoring still works.

Router status:
- `GET /health` → `{"zebra":"ok|down","zallet":"ok|down"}`; 200 when both fresh, 503 otherwise.
- `POST /` with a JSON-RPC body during startup returns 503 with a JSON-RPC error body until both backends respond.

## Monitoring

Grafana at <http://127.0.0.1:3000> (default `admin` / `admin`). Pre-provisioned dashboards (`zebra_overview` is the home page):

- Block / checkpoint / transaction verification rates
- Mempool depth, network message volume, peer count
- RocksDB latencies, RPC method timings, syncer progress
- Value pool balances (transparent / Sprout / Sapling / Orchard)

Alerts (Prometheus rules → Alertmanager → your receiver):

- `ZebraDown` (critical) — metrics endpoint unreachable for 2m
- `ZebraSyncStalled` (warning) — block height not advancing for 15m
- `ZebraLowPeers` / `ZebraNoPeers`
- `RPCHighLatency`, `RPCHighErrorRate`, `RPCOverloaded`
- `RpcRouterDown` (critical) — router `/health` unreachable for 5m

Alertmanager ships with empty receivers. Edit `observability/alertmanager/alertmanager.yml` to add Slack / email / webhook destinations and restart with `docker compose restart alertmanager`.

To run the node without monitoring, comment out `COMPOSE_PROFILES=monitoring` in `.env`.

The default observability stack covers Zebra. Disk-space alerts require `node_exporter` (deliberately omitted — bring your own if you want host-level metrics).

## Upgrading

```bash
docker compose pull
docker compose up -d
```

Defaults track `latest` for the Zcash images (Zebra, Zallet, rpc-router) so operators stay current. To pin for change control, uncomment the `ZCASHNG_*_IMAGE` lines in `.env`:

```env
ZCASHNG_ZEBRA_IMAGE=zfnd/zebra:5.0.0
ZCASHNG_ZALLET_IMAGE=electriccoinco/zallet:v0.1.0-alpha.3
ZCASHNG_RPC_ROUTER_IMAGE=ghcr.io/alchemydc/zcashng-rpc-router:v0.2.0
```

Monitoring images (Prometheus, Grafana, Alertmanager) are version-pinned in `docker-compose.yml` and only move with deliberate updates.

## Testnet & regtest

The same compose file runs all three networks; pick one with an env file:

```bash
# Testnet
docker compose --env-file env/testnet.env up -d

# Regtest (single-node test chain)
./setup.sh --network regtest
docker compose --env-file env/regtest.env up -d --wait
./scripts/regtest-mine.sh    # mines activation blocks + smoke-tests both RPCs
```

Coexistence: mainnet + testnet + regtest can run side-by-side on one host. Each env file uses a distinct `COMPOSE_PROJECT_NAME`, `ZCASHNG_DATA_DIR`, and loopback port offsets (+10000 for testnet, +20000 for regtest). Zebra's p2p port differs per network too (mainnet 8233, testnet 18233).

Regtest activation heights are tracked in `config/zebra.regtest.toml` and `config/zallet.regtest.toml.example`; both ship every upgrade at height 1 so the first mined block triggers them all. If you add a new network upgrade, edit both files and bump the CI smoke test.

## Stable names for integrators

These identifiers are part of zcashNG's public surface — they don't change without a release-notes call-out. Downstream containers / scripts can rely on them.

| What | Value |
|------|-------|
| Docker network | `zcashng-net` |
| Compose project name (mainnet) | `zcashng` |
| Service DNS: full node | `zebra` (container ports 8232 RPC, 8233 p2p, 8080 health, 9999 metrics) |
| Service DNS: wallet | `zallet` (container port 28232 RPC) |
| Service DNS: router | `rpc-router` (container port 8232 JSON-RPC + `/health`) |
| Container names | `zcashng_zebra`, `zcashng_zallet`, `zcashng_rpc_router`, `zcashng_{prometheus,grafana,alertmanager}` |
| Host-facing JSON-RPC | `127.0.0.1:8232` (rpc-router) by default; widened with `ZCASHNG_BIND_ADDR` |

A peer container can join the stack with:

```bash
docker run --rm --network zcashng-net curlimages/curl \
  -sS -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"getblockchaininfo","params":[],"id":1}' \
  http://rpc-router:8232/
```

## Troubleshooting

**`docker compose up` fails with permission errors writing to `data/`.** Run `./setup.sh` first; it chowns the per-service subdirectories to the right uid:gid for each image (Zebra 10001, Zallet 1000, Prometheus/Alertmanager 65534, Grafana 472). On macOS the chown is a no-op but containers still write fine via Docker Desktop's volume layer.

**Zebra `/ready` returns `not ready` for hours.** Initial sync. Check `docker compose logs -f zebra` for `Synced` lines; until then the rpc-router's `/health` also returns 503 (`{"zebra":"down","zallet":"ok"}`).

**rpc-router returns 503 on JSON-RPC calls but `/health` says both backends are ok.** The schemas haven't loaded yet — check `docker compose logs rpc-router` for `Backend schemas loaded`. The router retries with exponential backoff (5s → 60s); transient failures recover automatically.

**Inbound peers count is stuck at 0.** Your router/firewall isn't forwarding TCP 8233. Forward it (or run zcashNG on a public IP). `getpeerinfo` through the router shows current peers.

**Zallet emulation on arm64 (Apple Silicon) is slow.** `electriccoinco/zallet` is amd64-only for now; Docker uses binfmt emulation. Acceptable for development; for a production arm64 host, wait for the multi-arch release or build Zallet locally and pin via `ZCASHNG_ZALLET_IMAGE`.

**Mining `generate` RPC returns "mining address not set" on regtest.** Set `ZEBRA_MINING__MINER_ADDRESS` in `env/regtest.env` to a regtest-valid address. Generate one with `docker compose --env-file env/regtest.env exec zallet zallet generate-mnemonic` and `... z_getaddressforaccount`.

**`docker compose pull` fails for rpc-router.** The published image lives at `ghcr.io/<your-org>/zcashng-rpc-router`. If you're running zcashNG before pushing your own CI build, comment out the `image:` line in `docker-compose.yml` to force a local build (the `build:` block fires automatically).

## Logging

Container logs use your Docker daemon's default driver — zcashNG deliberately does not override it in `docker-compose.yml`. If you use the default `json-file` driver, set rotation in `/etc/docker/daemon.json`:

```json
{
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "50m",
    "max-file": "5"
  }
}
```

then `sudo systemctl restart docker`. For `journald` (most systemd hosts) or `syslog`, rotation is managed by the system; zcashNG won't fight it.

## License

MIT. See [LICENSE](LICENSE).
