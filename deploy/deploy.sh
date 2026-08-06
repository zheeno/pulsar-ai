#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
COMPOSE_PROJECT="-p pulsar"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[deploy]${NC} $*"; }
warn() { echo -e "${YELLOW}[deploy]${NC} $*"; }
err() { echo -e "${RED}[deploy]${NC} $*" >&2; }

# ── Install Docker if missing ──────────────────────────────────────────────
if ! command -v docker &>/dev/null; then
  log "Installing Docker..."
  curl -fsSL https://get.docker.com | sudo sh
  sudo usermod -aG docker "$USER" 2>/dev/null || true
fi

DOCKER="docker"
if ! docker info &>/dev/null 2>&1; then
  DOCKER="sudo docker"
fi

COMPOSE="$DOCKER compose $COMPOSE_PROJECT"
if ! $DOCKER compose version &>/dev/null 2>&1; then
  err "Docker Compose plugin not found"
  exit 1
fi

# ── Environment file ───────────────────────────────────────────────────────
if [ ! -f .env ]; then
  if [ -f .env.production.example ]; then
    cp .env.production.example .env
    JWT_SECRET=$(openssl rand -hex 32)
    PG_PASS=$(openssl rand -hex 16)
    sed -i "s/change-me-jwt-secret-min-32-chars/${JWT_SECRET}/" .env
    sed -i "s/change-me-strong-password/${PG_PASS}/" .env
    warn "Created .env from template with generated secrets."
    warn "Review .env and set CERTBOT_EMAIL, NGX_PULSE session credentials (or API key), OPENAI_API_KEY"
  else
    err ".env not found. Copy .env.production.example to .env first."
    exit 1
  fi
fi

# shellcheck disable=SC1091
source .env

WEB_DOMAIN="${WEB_DOMAIN:-pulsar.antimony.com.ng}"
API_DOMAIN="${API_DOMAIN:-pulsar-api.antimony.com.ng}"
API_HOST_PORT="${API_HOST_PORT:-${API_PORT:-3954}}"
WEB_HOST_PORT="${WEB_HOST_PORT:-${WEB_PORT:-3955}}"

# ── Build and start app stack (no Docker nginx) ───────────────────────────
log "Building and starting Docker services (postgres, redis, api, web)..."
$COMPOSE -f docker-compose.prod.yml build
$COMPOSE -f docker-compose.prod.yml up -d

log "Waiting for services to be ready..."
sleep 10

# ── Disable seed on subsequent deploys ────────────────────────────────────
if grep -q "RUN_SEED=true" .env; then
  sed -i 's/RUN_SEED=true/RUN_SEED=false/' .env
  log "Set RUN_SEED=false for future deploys."
fi

# ── Configure host nginx + SSL ────────────────────────────────────────────
log "Configuring host nginx..."
chmod +x deploy/setup-nginx.sh
./deploy/setup-nginx.sh

# ── Health checks ─────────────────────────────────────────────────────────
log "Running health checks..."
sleep 3

API_LOCAL=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:${API_HOST_PORT}/api/health" || echo "000")
WEB_LOCAL=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:${WEB_HOST_PORT}" || echo "000")
log "API (localhost:${API_HOST_PORT}): ${API_LOCAL}"
log "Web (localhost:${WEB_HOST_PORT}): ${WEB_LOCAL}"

HTTPS_API=$(curl -sk -o /dev/null -w "%{http_code}" "https://${API_DOMAIN}/api/health" || echo "000")
log "API (https://${API_DOMAIN}): ${HTTPS_API}"

echo ""
log "Deployment complete!"
echo "  Dashboard: https://${WEB_DOMAIN}"
echo "  API:       https://${API_DOMAIN}/api/health"
echo "  Login:     admin@ngx.local / admin123"
echo ""
log "View logs: $COMPOSE -f docker-compose.prod.yml logs -f"
