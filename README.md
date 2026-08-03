# NGX AI Trading Assistant

AI-powered NGX trading assistant with mock execution sandbox.

## Quick Start

```bash
# Start infrastructure
docker compose up -d

# Install dependencies
npm install

# Setup database
npm run db:migrate
npm run db:seed

# Copy env
cp .env.example .env

# Run development
npm run dev
```

- API: http://localhost:3001
- Dashboard: http://localhost:3000
- Login: admin@ngx.local / admin123

## Architecture

NestJS modular monolith (API) + Next.js dashboard + Postgres + Redis + BullMQ.

## Production Deployment (Docker + nginx + SSL)

### Prerequisites
- Ubuntu server with ports 80 and 443 open
- DNS A records pointing to server IP:
  - `pulsar.antimony.com.ng`
  - `pulsar-api.antimony.com.ng`
- If using Cloudflare proxy: temporarily set both records to **DNS only** (grey cloud) during initial cert issuance, then re-enable proxy with SSL mode **Full**

### Deploy

```bash
git pull origin cursor/docker-deploy-9b36   # or main after merge
cp .env.production.example .env
# Edit .env: set CERTBOT_EMAIL, optional API keys
chmod +x deploy/deploy.sh
./deploy/deploy.sh
```

The deploy script will:
1. Install Docker if needed
2. Build and start Postgres, Redis, API, Web
3. Start nginx (HTTP) for ACME challenge
4. Obtain Let's Encrypt SSL certs via Certbot
5. Switch nginx to HTTPS config

### URLs
- Dashboard: https://pulsar.antimony.com.ng
- API: https://pulsar-api.antimony.com.ng/api/health
- Login: `admin@ngx.local` / `admin123`

### Management

```bash
docker compose -p pulsar -f docker-compose.prod.yml logs -f
docker compose -p pulsar -f docker-compose.prod.yml restart api
docker compose -p pulsar -f docker-compose.prod.yml -f docker-compose.ssl.override.yml up -d
```

