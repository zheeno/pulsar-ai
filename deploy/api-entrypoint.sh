#!/bin/sh
set -e

echo "Waiting for Postgres..."
until node -e "
const { Client } = require('pg');
const c = new Client({ connectionString: process.env.DATABASE_URL });
c.connect().then(() => c.end()).then(() => process.exit(0)).catch(() => process.exit(1));
" 2>/dev/null; do
  sleep 2
done

echo "Running migrations..."
node /app/scripts/migrate.js

if [ "${RUN_SEED:-false}" = "true" ]; then
  echo "Seeding database..."
  node /app/scripts/seed.js
fi

echo "Starting API..."
exec node /app/dist/main.js
