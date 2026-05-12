#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
  cat <<'EOF'
Usage: scripts/prod-up.sh [options]

Build the local production Docker image and start the production container.

Options:
  --smoke            Run scripts/prod-smoke.sh after /healthz is ready
  --remove-orphans   Pass --remove-orphans to docker compose up
  -h, --help         Show this help

Environment:
  KIRO_RS_IMAGE              Image name, default kiro-rs:prod
  KIRO_RS_BIND               Host bind address, default 0.0.0.0
  KIRO_RS_PORT               Host port, default 18990
  RUST_LOG                   Container log level, default info
  DOCKER_BUILDKIT            Enable BuildKit, default 1
  DOCKER_BUILD_PROGRESS      Build progress output, default plain
  HEALTH_TIMEOUT_SECS        Health wait timeout, default 90
  RUN_SMOKE                  Same as --smoke when set to 1
  REMOVE_ORPHANS             Same as --remove-orphans when set to 1
EOF
}

SERVICE_NAME="${SERVICE_NAME:-kiro-rs}"
KIRO_RS_IMAGE="${KIRO_RS_IMAGE:-kiro-rs:prod}"
KIRO_RS_BIND="${KIRO_RS_BIND:-0.0.0.0}"
KIRO_RS_PORT="${KIRO_RS_PORT:-18990}"
RUST_LOG="${RUST_LOG:-info}"
HEALTH_TIMEOUT_SECS="${HEALTH_TIMEOUT_SECS:-90}"
HEALTH_URL="${HEALTH_URL:-http://127.0.0.1:${KIRO_RS_PORT}/healthz}"
RUN_SMOKE="${RUN_SMOKE:-0}"
REMOVE_ORPHANS="${REMOVE_ORPHANS:-0}"
DOCKER_BUILD_PROGRESS="${DOCKER_BUILD_PROGRESS:-plain}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --smoke)
      RUN_SMOKE=1
      ;;
    --remove-orphans)
      REMOVE_ORPHANS=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 2
  fi
}

require_cmd docker
require_cmd curl

if ! docker compose version >/dev/null 2>&1; then
  echo "docker compose plugin is required" >&2
  exit 2
fi

if [ ! -f config/config.json ]; then
  echo "missing config/config.json" >&2
  echo "create it before starting production container" >&2
  exit 1
fi

mkdir -p config

export KIRO_RS_IMAGE
export KIRO_RS_BIND
export KIRO_RS_PORT
export RUST_LOG
export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-1}"
export DOCKER_BUILD_PROGRESS

compose_args=(
  -f docker-compose-prod.yml
  -f docker-compose-prod.build.yml
)

build_args=(build --progress="$DOCKER_BUILD_PROGRESS" "$SERVICE_NAME")
up_args=(up -d --no-build)
if [ "$REMOVE_ORPHANS" = "1" ]; then
  up_args+=(--remove-orphans)
fi
up_args+=("$SERVICE_NAME")

echo "Building and starting production container"
echo "image=$KIRO_RS_IMAGE bind=$KIRO_RS_BIND port=$KIRO_RS_PORT"
echo "docker_buildkit=$DOCKER_BUILDKIT docker_build_progress=$DOCKER_BUILD_PROGRESS"

docker compose "${compose_args[@]}" "${build_args[@]}"
docker compose "${compose_args[@]}" "${up_args[@]}"

echo "Waiting for health check: $HEALTH_URL"
deadline=$((SECONDS + HEALTH_TIMEOUT_SECS))
while [ "$SECONDS" -lt "$deadline" ]; do
  if curl -fsS --max-time 3 "$HEALTH_URL" >/dev/null; then
    echo "Production container is healthy"
    docker compose "${compose_args[@]}" ps "$SERVICE_NAME"
    if [ "$RUN_SMOKE" = "1" ]; then
      BASE_URL="${BASE_URL:-http://127.0.0.1:${KIRO_RS_PORT}}" scripts/prod-smoke.sh
    fi
    exit 0
  fi
  sleep 2
done

echo "container did not become healthy within ${HEALTH_TIMEOUT_SECS}s" >&2
docker compose "${compose_args[@]}" ps "$SERVICE_NAME" >&2 || true
docker compose "${compose_args[@]}" logs --tail=120 "$SERVICE_NAME" >&2 || true
exit 1
