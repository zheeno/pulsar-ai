# NGX AI Trading Assistant

AI-powered NGX trading assistant with mock execution sandbox.

## Quick Start (local development)

```bash
docker compose up -d
npm install
npm run db:migrate && npm run db:seed
cp .env.example .env
npm run dev
```

- API: http://localhost:3001
- Dashboard: http://localhost:3000
- Login: admin@ngx.local / admin123

## Architecture

NestJS modular monolith (API) + Next.js dashboard + Postgres + Redis + BullMQ.

## Production Deployment (Docker + host nginx + Certbot)

Uses **host nginx** as the reverse proxy (no nginx container). Docker runs only the app stack.

### Prerequisites

- Ubuntu server with **host nginx** already installed
- Ports 80 and 443 available to host nginx
- DNS A records pointing to server IP:
  - `pulsar.antimony.com.ng`
  - `pulsar-api.antimony.com.ng`
- Certbot: `sudo apt install certbot python3-certbot-nginx`

### Deploy

```bash
git pull
cp .env.production.example .env
# Edit .env: CERTBOT_EMAIL, optional API keys
chmod +x deploy/deploy.sh deploy/setup-nginx.sh
./deploy/deploy.sh
```

The deploy script will:
1. Install Docker if needed
2. Build and start Postgres, Redis, API (127.0.0.1:3954 → container :3001), Web (127.0.0.1:3955 → container :3000)
3. Install nginx site configs from `deploy/nginx/host/`
4. Obtain SSL certificates via host Certbot (`certbot --nginx`)

### Manual nginx setup (if needed)

```bash
sudo cp deploy/nginx/host/pulsar-web.conf /etc/nginx/sites-available/pulsar.antimony.com.ng.conf
sudo cp deploy/nginx/host/pulsar-api.conf /etc/nginx/sites-available/pulsar-api.antimony.com.ng.conf
sudo ln -sf /etc/nginx/sites-available/pulsar.antimony.com.ng.conf /etc/nginx/sites-enabled/
sudo ln -sf /etc/nginx/sites-available/pulsar-api.antimony.com.ng.conf /etc/nginx/sites-enabled/
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d pulsar.antimony.com.ng -d pulsar-api.antimony.com.ng
```

### URLs

- Dashboard: https://pulsar.antimony.com.ng
- API: https://pulsar-api.antimony.com.ng/api/health
- Login: `admin@ngx.local` / `admin123`

### Management

```bash
# App logs
docker compose -p pulsar -f docker-compose.prod.yml logs -f

# Restart API
docker compose -p pulsar -f docker-compose.prod.yml restart api

# Redeploy after code changes
docker compose -p pulsar -f docker-compose.prod.yml up -d --build

# Reload nginx after config changes
sudo nginx -t && sudo systemctl reload nginx
```

### Cloudflare

If domains are proxied, use SSL mode **Full** after Certbot issues the origin certificate.

## Testing

```bash
FORCE_INGEST=true npm run test --workspace=@ngx/api
```
