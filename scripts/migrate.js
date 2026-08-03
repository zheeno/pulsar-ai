#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const { Client } = require('pg');

async function migrate() {
  const databaseUrl = process.env.DATABASE_URL || 'postgresql://postgres:postgres@localhost:5432/ngx_trading';
  const client = new Client({ connectionString: databaseUrl });
  await client.connect();

  const migrationDir = path.join(__dirname, '..', 'supabase', 'migrations');
  const files = fs.readdirSync(migrationDir).filter((f) => f.endsWith('.sql')).sort();

  for (const file of files) {
    const sql = fs.readFileSync(path.join(migrationDir, file), 'utf8');
    console.log(`Running migration: ${file}`);
    await client.query(sql);
  }

  await client.end();
  console.log('Migrations complete.');
}

migrate().catch((err) => {
  console.error(err);
  process.exit(1);
});
