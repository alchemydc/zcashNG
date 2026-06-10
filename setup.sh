#!/usr/bin/env sh
# zcashNG setup. Idempotent; safe to re-run.
#   ./setup.sh                  → mainnet
#   ./setup.sh --network testnet
#   ./setup.sh --network regtest
#
# Creates data directories with the right ownership, copies tracked config
# templates into place, and generates a Zallet age identity if one isn't there
# yet. After this finishes:
#   docker compose up -d                                  # mainnet
#   docker compose --env-file env/testnet.env up -d       # testnet
#   docker compose --env-file env/regtest.env up -d       # regtest

set -eu

NETWORK=mainnet
while [ $# -gt 0 ]; do
    case "$1" in
        --network)
            shift
            NETWORK="${1:-}"
            ;;
        --network=*)
            NETWORK="${1#--network=}"
            ;;
        -h|--help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "setup.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
    shift
done

case "$NETWORK" in
    mainnet|testnet|regtest) ;;
    *)
        echo "setup.sh: --network must be mainnet|testnet|regtest (got: $NETWORK)" >&2
        exit 2
        ;;
esac

log() { printf '==> %s\n' "$*"; }

# -----------------------------------------------------------------------------
# 1. Detect Docker Compose. Prefer v2 plugin (`docker compose`); fall back to
#    legacy standalone (`docker-compose`). One of them must work.
# -----------------------------------------------------------------------------
DC=""
if docker compose version >/dev/null 2>&1; then
    DC="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
    DC="docker-compose"
else
    echo "setup.sh: neither 'docker compose' (v2 plugin) nor 'docker-compose' (legacy) is available." >&2
    echo "Install Docker Desktop / OrbStack / Rancher, or 'pip install docker-compose'." >&2
    exit 1
fi
log "using compose: $DC"

# -----------------------------------------------------------------------------
# 2. Pick the env file for this network and source ZCASHNG_DATA_DIR from it.
# -----------------------------------------------------------------------------
case "$NETWORK" in
    mainnet)
        ENV_TEMPLATE=".env.example"
        ENV_FILE=".env"
        ;;
    testnet)
        ENV_TEMPLATE="env/testnet.env"
        ENV_FILE="env/testnet.env"
        ;;
    regtest)
        ENV_TEMPLATE="env/regtest.env"
        ENV_FILE="env/regtest.env"
        ;;
esac

if [ "$NETWORK" = "mainnet" ] && [ ! -f "$ENV_FILE" ]; then
    cp "$ENV_TEMPLATE" "$ENV_FILE"
    log ".env: created from .env.example"
fi

# Source the env file to get ZCASHNG_DATA_DIR (and only that). Use a subshell
# so we don't pollute the caller's environment with everything in the file.
DATA_DIR=$(
    set -a
    # shellcheck disable=SC1090
    . "./$ENV_FILE" 2>/dev/null || true
    set +a
    printf '%s' "${ZCASHNG_DATA_DIR:-./data}"
)
case "$NETWORK" in
    testnet) DATA_DIR="${DATA_DIR:-./data-testnet}" ;;
    regtest) DATA_DIR="${DATA_DIR:-./data-regtest}" ;;
esac
log "data directory: $DATA_DIR"

# -----------------------------------------------------------------------------
# 3. Create per-service data subdirs with the right uid:gid. Each upstream
#    image runs as a known non-root user; we chown so writes work without
#    coaxing. macOS uses osxfs/virtiofs for bind mounts, which ignores chown
#    silently (still safe to call).
# -----------------------------------------------------------------------------
mkdir -p "$DATA_DIR/zebra" "$DATA_DIR/zallet"
if [ "${COMPOSE_PROFILES:-monitoring}" != "" ]; then
    mkdir -p "$DATA_DIR/prometheus" "$DATA_DIR/grafana" "$DATA_DIR/alertmanager"
fi

chown_if_needed() {
    target_uid="$1"
    target_gid="$2"
    path="$3"
    [ -d "$path" ] || return 0
    current_uid=$(stat -f '%u' "$path" 2>/dev/null || stat -c '%u' "$path" 2>/dev/null || echo "")
    if [ "$current_uid" != "$target_uid" ]; then
        if [ "$(id -u)" = "0" ]; then
            chown -R "$target_uid:$target_gid" "$path"
        else
            sudo chown -R "$target_uid:$target_gid" "$path" 2>/dev/null || {
                log "WARN: could not chown $path to $target_uid:$target_gid (sudo declined or unavailable)"
                log "      If services fail to write to $path, run: sudo chown -R $target_uid:$target_gid $path"
            }
        fi
    fi
}

# Zebra:  10001:10001 (image default — entrypoint chowns this itself, but
#                      pre-setting saves a startup chown pass).
# Zallet: 1000:1000 (numeric UID baked into the distroless image).
# Prometheus, Alertmanager: 65534:65534 (nobody).
# Grafana: 472:472 (image default).
chown_if_needed 10001 10001 "$DATA_DIR/zebra"
chown_if_needed 1000  1000  "$DATA_DIR/zallet"
[ -d "$DATA_DIR/prometheus" ]   && chown_if_needed 65534 65534 "$DATA_DIR/prometheus"
[ -d "$DATA_DIR/grafana" ]      && chown_if_needed 472   472   "$DATA_DIR/grafana"
[ -d "$DATA_DIR/alertmanager" ] && chown_if_needed 65534 65534 "$DATA_DIR/alertmanager"

# -----------------------------------------------------------------------------
# 4. Copy config templates into place.
# -----------------------------------------------------------------------------
case "$NETWORK" in
    mainnet|testnet)
        if [ ! -f "config/zallet.toml" ]; then
            cp config/zallet.toml.example config/zallet.toml
            log "config/zallet.toml: created from .example"
            if [ "$NETWORK" = "testnet" ]; then
                # Flip consensus.network from "main" to "test" in the copy.
                # POSIX sed -i differs across macOS/Linux; do it portably.
                sed -e 's/^network = "main"$/network = "test"/' \
                    config/zallet.toml > config/zallet.toml.tmp \
                    && mv config/zallet.toml.tmp config/zallet.toml
            fi
        fi
        ;;
    regtest)
        if [ ! -f "config/zallet.regtest.toml" ]; then
            cp config/zallet.regtest.toml.example config/zallet.regtest.toml
            log "config/zallet.regtest.toml: created from .example"
        fi
        ;;
esac

# -----------------------------------------------------------------------------
# 5. Generate the Zallet age identity if it doesn't exist. Zallet encrypts
#    wallet secrets with this file; lose it and the wallet is unrecoverable.
# -----------------------------------------------------------------------------
if [ ! -f config/zallet_identity.txt ]; then
    if command -v rage-keygen >/dev/null 2>&1; then
        rage-keygen -o config/zallet_identity.txt 2>/dev/null
    elif command -v age-keygen >/dev/null 2>&1; then
        age-keygen -o config/zallet_identity.txt 2>/dev/null
    else
        cat >&2 <<'EOF'
setup.sh: neither rage-keygen nor age-keygen is on PATH.

Zallet encrypts wallet secrets with an age identity stored at
config/zallet_identity.txt. Install one of these tools and re-run:
  - macOS: brew install rage
  - Debian/Ubuntu: apt install age
  - Other: https://github.com/str4d/rage/releases  (or https://age-encryption.org/)

Alternatively, generate the identity inside any container that ships age:
  docker run --rm -v "$(pwd)/config:/out" str4d/rage \
      age-keygen -o /out/zallet_identity.txt
EOF
        exit 1
    fi
    chmod 600 config/zallet_identity.txt
    log "config/zallet_identity.txt: generated (mode 0600). BACK THIS UP."
fi

# -----------------------------------------------------------------------------
# 6. Done. Print the operator next steps.
# -----------------------------------------------------------------------------
cat <<EOF

zcashNG setup complete for network: $NETWORK
  - Data directory:   $DATA_DIR
  - Wallet identity:  config/zallet_identity.txt  (BACK THIS UP)
  - Wallet config:    config/zallet$( [ "$NETWORK" = "regtest" ] && printf .regtest ).toml
  - Compose env:      $ENV_FILE

Start the stack:
EOF
case "$NETWORK" in
    mainnet) printf '  %s up -d\n' "$DC" ;;
    testnet) printf '  %s --env-file env/testnet.env up -d\n' "$DC" ;;
    regtest) printf '  %s --env-file env/regtest.env up -d  && ./scripts/regtest-mine.sh\n' "$DC" ;;
esac
echo

echo "What to back up:"
echo "  - config/             (tiny; contains your wallet identity)"
echo "  - $DATA_DIR/zallet    (wallet database)"
echo "  - $DATA_DIR/zebra is re-syncable; backing it up just saves resync time"
