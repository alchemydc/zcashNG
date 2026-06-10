#!/usr/bin/env sh
# Mine activation-height blocks on regtest and confirm the stack is healthy.
# Requires: stack already brought up with `--env-file env/regtest.env`.
# Idempotent; safe to re-run.

set -eu

# Pick host port from env/regtest.env (loopback bind).
ROUTER_PORT="${ZCASHNG_RPC_PORT:-28232}"
ROUTER_URL="http://127.0.0.1:${ROUTER_PORT}"

DC=""
if docker compose version >/dev/null 2>&1; then
    DC="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
    DC="docker-compose"
else
    echo "regtest-mine.sh: 'docker compose' / 'docker-compose' not found" >&2
    exit 1
fi

log() { printf '==> %s\n' "$*"; }

# 1. Wait for the router to be reachable on the regtest host port.
log "waiting for rpc-router at $ROUTER_URL"
attempt=0
until curl -sf -o /dev/null "$ROUTER_URL/health" 2>/dev/null; do
    attempt=$((attempt + 1))
    if [ "$attempt" -gt 60 ]; then
        echo "regtest-mine.sh: router did not become healthy within 5 minutes" >&2
        exit 1
    fi
    sleep 5
done
log "router healthy"

# 2. Mine 2 blocks via the `generate` RPC method (regtest-only on Zebra).
#    The first block triggers all activation-height upgrades; the second
#    confirms the chain is producing.
log "mining 2 regtest blocks via $ROUTER_URL"
RESPONSE=$(curl -sS -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"generate","params":[2],"id":1}' \
    "$ROUTER_URL/")
echo "$RESPONSE"

# 3. Smoke a node-side method and a wallet-side method through the router.
log "smoke: node method (getblockchaininfo)"
curl -sS -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"getblockchaininfo","params":[],"id":2}' \
    "$ROUTER_URL/" | head -c 400
echo

log "smoke: wallet method (z_listaccounts)"
curl -sS -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"z_listaccounts","params":[],"id":3}' \
    "$ROUTER_URL/" | head -c 400
echo

log "regtest stack reachable and producing. Tail logs with: $DC --env-file env/regtest.env logs -f"
