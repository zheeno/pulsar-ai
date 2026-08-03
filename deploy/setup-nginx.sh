#!/usr/bin/env bash
# Configure host nginx for Pulsar and optionally obtain SSL via Certbot.
# Run with sudo for certbot/nginx file operations, or as a user with sudo access.
set -euo pipefail

cd "$(dirname "$0")/.."
PROJECT_DIR=$(pwd)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[nginx]${NC} $*"; }
warn() { echo -e "${YELLOW}[nginx]${NC} $*"; }
err() { echo -e "${RED}[nginx]${NC} $*" >&2; }

if [ -f .env ]; then
  # shellcheck disable=SC1091
  source .env
fi

WEB_DOMAIN="${WEB_DOMAIN:-pulsar.antimony.com.ng}"
API_DOMAIN="${API_DOMAIN:-pulsar-api.antimony.com.ng}"
CERTBOT_EMAIL="${CERTBOT_EMAIL:-admin@antimony.com.ng}"
API_HOST_PORT="${API_HOST_PORT:-${API_PORT:-3954}}"
WEB_HOST_PORT="${WEB_HOST_PORT:-${WEB_PORT:-3955}}"

if ! command -v nginx &>/dev/null; then
  err "nginx is not installed. Install with: sudo apt install nginx"
  exit 1
fi

log "Installing nginx site configs..."
sudo cp "${PROJECT_DIR}/deploy/nginx/host/pulsar-cors-map.conf" /etc/nginx/conf.d/pulsar-cors-map.conf
sudo cp "${PROJECT_DIR}/deploy/nginx/host/pulsar-web.conf" "/etc/nginx/sites-available/${WEB_DOMAIN}.conf"
sudo cp "${PROJECT_DIR}/deploy/nginx/host/pulsar-api.conf" "/etc/nginx/sites-available/${API_DOMAIN}.conf"

# Patch server_name if domains differ from defaults in template files
sudo sed -i "s/pulsar.antimony.com.ng/${WEB_DOMAIN}/g" "/etc/nginx/sites-available/${WEB_DOMAIN}.conf"
sudo sed -i "s/pulsar-api.antimony.com.ng/${API_DOMAIN}/g" "/etc/nginx/sites-available/${API_DOMAIN}.conf"
sudo sed -i "s/pulsar.antimony.com.ng/${WEB_DOMAIN}/g" /etc/nginx/conf.d/pulsar-cors-map.conf
sudo sed -i "s/127.0.0.1:3955/127.0.0.1:${WEB_HOST_PORT}/g" "/etc/nginx/sites-available/${WEB_DOMAIN}.conf"
sudo sed -i "s/127.0.0.1:3954/127.0.0.1:${API_HOST_PORT}/g" "/etc/nginx/sites-available/${API_DOMAIN}.conf"
sudo sed -i "s|/etc/letsencrypt/live/pulsar.antimony.com.ng/|/etc/letsencrypt/live/${WEB_DOMAIN}/|g" \
  "/etc/nginx/sites-available/${WEB_DOMAIN}.conf" \
  "/etc/nginx/sites-available/${API_DOMAIN}.conf"

sudo ln -sf "/etc/nginx/sites-available/${WEB_DOMAIN}.conf" /etc/nginx/sites-enabled/
sudo ln -sf "/etc/nginx/sites-available/${API_DOMAIN}.conf" /etc/nginx/sites-enabled/

log "Testing nginx configuration..."
sudo nginx -t

log "Reloading nginx..."
sudo systemctl reload nginx

# ── SSL via host Certbot ───────────────────────────────────────────────────
if command -v certbot &>/dev/null; then
  if [ ! -f "/etc/letsencrypt/live/${WEB_DOMAIN}/fullchain.pem" ]; then
    log "Requesting SSL certificates for ${WEB_DOMAIN} and ${API_DOMAIN}..."
    sudo certbot --nginx \
      --email "$CERTBOT_EMAIL" \
      --agree-tos --no-eff-email \
      -d "$WEB_DOMAIN" \
      -d "$API_DOMAIN"
  else
    log "SSL certificate already exists for ${WEB_DOMAIN}, skipping issuance."
    sudo certbot renew --dry-run 2>/dev/null || warn "Certbot renew dry-run failed — check certs manually."
  fi
else
  warn "certbot not installed. Install with: sudo apt install certbot python3-certbot-nginx"
  warn "Then run: sudo certbot --nginx -d ${WEB_DOMAIN} -d ${API_DOMAIN}"
fi

log "Host nginx setup complete."
echo "  Dashboard: https://${WEB_DOMAIN}"
echo "  API:       https://${API_DOMAIN}/api/health"
