/**
 * Prepare Tauri resources for a shippable desktop build:
 * - Single-file agent worker (esbuild)
 * - Public Pulse config (app.env) from repo .env / process env
 * - Bundled Node.js runtime (macOS 10.15+ / Windows) for the LLM agent worker
 */
import { build } from 'esbuild';
import { execFileSync } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import {
  chmod,
  copyFile,
  mkdir,
  readFile,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const resourcesDir = join(root, 'apps/desktop/src-tauri/resources');
const workerEntry = join(root, 'packages/agent/src/worker.ts');
const sharedSrc = join(root, 'packages/shared/src/index.ts');
const workerOut = join(resourcesDir, 'agent-worker.cjs');
const appEnvOut = join(resourcesDir, 'app.env');
const nodeDir = join(resourcesDir, 'node');

/** Node 20 LTS — macOS x64 build targets 10.15; arm64 requires macOS 11+. */
const NODE_VERSION = process.env.PULSAR_NODE_VERSION || '20.19.2';

/** Must stay in sync with AgentRequestSchema / Rust agent.rs ops. */
const REQUIRED_WORKER_OPS = [
  'ping',
  'portfolio_signals',
  'symbol_signal',
  'test_llm',
  'strategy_coach',
];

async function assertWorkerOps(bundled) {
  const missing = REQUIRED_WORKER_OPS.filter(
    (op) => !bundled.includes(`literal("${op}")`) && !bundled.includes(`literal('${op}')`),
  );
  if (missing.length) {
    throw new Error(
      `Bundled agent worker is missing AgentRequest ops: ${missing.join(', ')}. ` +
        'esbuild must resolve @ngx/shared from packages/shared/src, not a stale dist/.',
    );
  }
}

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

async function downloadFile(url, dest) {
  const res = await fetch(url);
  if (!res.ok || !res.body) {
    throw new Error(`Failed to download ${url} (${res.status})`);
  }
  await pipeline(res.body, createWriteStream(dest));
}

function nativeArch() {
  if (process.env.PULSAR_NODE_ARCH) {
    return process.env.PULSAR_NODE_ARCH === 'arm64' ? 'arm64' : 'x64';
  }
  if (process.platform === 'darwin') {
    try {
      const machine = execFileSync('uname', ['-m'], { encoding: 'utf8' }).trim();
      return machine === 'arm64' ? 'arm64' : 'x64';
    } catch {
      // fall through
    }
  }
  return process.arch === 'arm64' ? 'arm64' : 'x64';
}

function resolveNodeTarget() {
  const cargoTarget = process.env.CARGO_BUILD_TARGET?.trim();
  if (cargoTarget === 'x86_64-apple-darwin') {
    return { platform: 'darwin', arch: 'x64' };
  }
  if (cargoTarget === 'aarch64-apple-darwin') {
    return { platform: 'darwin', arch: 'arm64' };
  }

  const platform = process.env.PULSAR_NODE_PLATFORM || process.platform;
  if (platform === 'darwin') {
    return { platform, arch: nativeArch() };
  }
  if (platform === 'win32') {
    return { platform, arch: 'x64' };
  }
  return null;
}

function nodeStamp(target) {
  return `${NODE_VERSION}-${target.platform}-${target.arch}`;
}

async function bundledNodeReady(target) {
  try {
    const stamp = await readFile(join(nodeDir, '.node-version'), 'utf8');
    if (stamp.trim() !== nodeStamp(target)) {
      return false;
    }
    if (target.platform === 'darwin') {
      return (await stat(join(nodeDir, 'bin', 'node')).catch(() => null))?.isFile();
    }
    return (await stat(join(nodeDir, 'node.exe')).catch(() => null))?.isFile();
  } catch {
    return false;
  }
}

async function bundleNodeRuntime() {
  if (process.env.PULSAR_SKIP_NODE_BUNDLE === '1') {
    console.log('[prepare-desktop-resources] Skipping Node bundle (PULSAR_SKIP_NODE_BUNDLE=1)');
    return;
  }

  const target = resolveNodeTarget();
  if (!target) {
    console.log(
      '[prepare-desktop-resources] Skipping Node bundle on this OS (Linux dev uses system Node)',
    );
    return;
  }

  if (await bundledNodeReady(target)) {
    console.log(`[prepare-desktop-resources] Node ${nodeStamp(target)} already bundled`);
    return;
  }

  await rm(nodeDir, { recursive: true, force: true });
  await mkdir(nodeDir, { recursive: true });

  const tmp = join(resourcesDir, '.node-extract');
  await rm(tmp, { recursive: true, force: true });
  await mkdir(tmp, { recursive: true });

  if (target.platform === 'darwin') {
    const tarball = `node-v${NODE_VERSION}-darwin-${target.arch}.tar.gz`;
    const url = `https://nodejs.org/dist/v${NODE_VERSION}/${tarball}`;
    const archive = join(tmp, tarball);
    console.log(`[prepare-desktop-resources] Downloading ${url}`);
    await downloadFile(url, archive);
    execFileSync('tar', ['-xzf', archive, '-C', tmp], { stdio: 'inherit' });
    const extracted = join(tmp, `node-v${NODE_VERSION}-darwin-${target.arch}`);
    await mkdir(join(nodeDir, 'bin'), { recursive: true });
    await copyFile(join(extracted, 'bin', 'node'), join(nodeDir, 'bin', 'node'));
    await chmod(join(nodeDir, 'bin', 'node'), 0o755);
    await copyFile(join(extracted, 'LICENSE'), join(nodeDir, 'LICENSE'));
  } else if (target.platform === 'win32') {
    const zipName = `node-v${NODE_VERSION}-win-${target.arch}.zip`;
    const url = `https://nodejs.org/dist/v${NODE_VERSION}/${zipName}`;
    const archive = join(tmp, zipName);
    console.log(`[prepare-desktop-resources] Downloading ${url}`);
    await downloadFile(url, archive);
    execFileSync(
      'powershell',
      [
        '-NoProfile',
        '-Command',
        `Expand-Archive -LiteralPath '${archive.replace(/'/g, "''")}' -DestinationPath '${tmp.replace(/'/g, "''")}' -Force`,
      ],
      { stdio: 'inherit' },
    );
    const extracted = join(tmp, `node-v${NODE_VERSION}-win-${target.arch}`);
    await copyFile(join(extracted, 'node.exe'), join(nodeDir, 'node.exe'));
    await copyFile(join(extracted, 'LICENSE'), join(nodeDir, 'LICENSE'));
  }

  await writeFile(join(nodeDir, '.node-version'), `${nodeStamp(target)}\n`, 'utf8');
  await rm(tmp, { recursive: true, force: true });
  console.log(`[prepare-desktop-resources] Bundled Node ${NODE_VERSION} for ${target.platform}-${target.arch}`);
}

async function main() {
  await mkdir(resourcesDir, { recursive: true });

  // Bundle shared from TypeScript source. Resolving @ngx/shared via package.json
  // "main" (dist/) silently ships a stale AgentRequestSchema — e.g. rejecting
  // strategy_coach with invalid_union_discriminator while worker.ts handles it.
  await build({
    entryPoints: [workerEntry],
    bundle: true,
    platform: 'node',
    format: 'cjs',
    outfile: workerOut,
    logLevel: 'info',
    absWorkingDir: root,
    alias: {
      '@ngx/shared': sharedSrc,
    },
  });

  const bundled = await readFile(workerOut, 'utf8');
  await assertWorkerOps(bundled);

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

  await bundleNodeRuntime();

  console.log(`[prepare-desktop-resources] wrote ${workerOut}`);
  console.log(`[prepare-desktop-resources] wrote ${appEnvOut}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
