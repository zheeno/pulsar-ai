/**
 * Prepare Tauri resources for a shippable desktop build:
 * - Single-file agent worker (esbuild)
 * - Public Pulse config (app.env) from repo .env / process env
 */
import { build } from 'esbuild';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const resourcesDir = join(root, 'apps/desktop/src-tauri/resources');
const workerEntry = join(root, 'packages/agent/src/worker.ts');
const workerOut = join(resourcesDir, 'agent-worker.cjs');
const appEnvOut = join(resourcesDir, 'app.env');

function parseDotEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq <= 0) continue;
    const key = trimmed.slice(0, eq).trim();
    let value = trimmed.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    out[key] = value;
  }
  return out;
}

async function loadEnvFile() {
  try {
    const text = await readFile(join(root, '.env'), 'utf8');
    return parseDotEnv(text);
  } catch {
    return {};
  }
}

async function main() {
  await mkdir(resourcesDir, { recursive: true });

  await build({
    entryPoints: [workerEntry],
    bundle: true,
    platform: 'node',
    format: 'cjs',
    outfile: workerOut,
    logLevel: 'info',
  });

  const fileEnv = await loadEnvFile();
  const pick = (key, fallback = '') => process.env[key] || fileEnv[key] || fallback;
  const lines = [
    '# Generated for Pulsar AI macOS/Windows bundle — public Pulse client config only.',
    `NGX_PULSE_BASE_URL=${pick('NGX_PULSE_BASE_URL', 'https://ngxpulse.ng/api')}`,
    `NGX_PULSE_SUPABASE_URL=${pick('NGX_PULSE_SUPABASE_URL')}`,
    `NGX_PULSE_SUPABASE_ANON_KEY=${pick('NGX_PULSE_SUPABASE_ANON_KEY')}`,
    // Production ship: enforce market hours + Wealth production API.
    'APP_ENV=production',
    '',
  ];
  await writeFile(appEnvOut, lines.join('\n'), 'utf8');

  if (!pick('NGX_PULSE_SUPABASE_URL') || !pick('NGX_PULSE_SUPABASE_ANON_KEY')) {
    console.warn(
      '[prepare-desktop-resources] Warning: NGX_PULSE_SUPABASE_URL / ANON_KEY missing — Pulse login will fail in the shipped app.',
    );
  }

  console.log(`[prepare-desktop-resources] wrote ${workerOut}`);
  console.log(`[prepare-desktop-resources] wrote ${appEnvOut}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
