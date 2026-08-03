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

## Testing

```bash
FORCE_INGEST=true npm run test --workspace=@ngx/api
```
