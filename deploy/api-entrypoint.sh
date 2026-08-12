#!/bin/sh
set -e

echo "Waiting for MongoDB..."
until node -e "
const { MongoClient } = require('mongodb');
const uri = process.env.MONGODB_URI || 'mongodb://localhost:27017/pulsar';
const c = new MongoClient(uri);
c.connect().then(() => c.close()).then(() => process.exit(0)).catch(() => process.exit(1));
" 2>/dev/null; do
  sleep 2
done

if [ "${RUN_SEED:-false}" = "true" ]; then
  echo "Seeding database..."
  node /app/scripts/seed.js
fi

echo "Starting API..."
exec node /app/dist/main.js
