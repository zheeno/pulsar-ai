#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const NGX_BASE_URL = 'https://ngxpulse.ng/api';
const USER_AGENT =
  'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36';

const AUTH_MODES = [
  { mode: 'bearer', label: 'bearer only' },
  { mode: 'cookies', label: 'cookies only' },
  { mode: 'both', label: 'bearer + cookies' },
];

function buildPricePath(symbol = 'DANGCEM', days = 30) {
  const to = new Date();
  const from = new Date();
  from.setDate(from.getDate() - days);
  const fmt = (d) => d.toISOString().split('T')[0];
  const params = new URLSearchParams({ from: fmt(from), to: fmt(to) });
  return `/ngxdata/prices/${symbol}?${params.toString()}`;
}

const ENDPOINTS = [
  { name: 'market-status', path: '/ngxdata/market-status', authProbe: true },
  { name: 'stocks', path: '/ngxdata/stocks' },
  { name: 'market', path: '/ngxdata/market' },
  { name: 'indices', path: '/ngxdata/indices' },
  { name: 'prices/DANGCEM', path: buildPricePath('DANGCEM') },
];

function loadEnvFile() {
  const envPath = path.join(__dirname, '..', '.env');
  if (!fs.existsSync(envPath)) return;
  const lines = fs.readFileSync(envPath, 'utf8').split('\n');
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq === -1) continue;
    const key = trimmed.slice(0, eq).trim();
    let value = trimmed.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (process.env[key] === undefined) {
      process.env[key] = value;
    }
  }
}

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing required env var: ${name}`);
    console.error('Add it to .env (see .env.example) and retry.');
    process.exit(1);
  }
  return value;
}

function parseSetCookieHeaders(headers) {
  const cookies = {};
  const raw = headers.getSetCookie ? headers.getSetCookie() : [];
  if (raw.length === 0) {
    const single = headers.get('set-cookie');
    if (single) raw.push(single);
  }
  for (const header of raw) {
    const pair = header.split(';')[0];
    const eq = pair.indexOf('=');
    if (eq === -1) continue;
    cookies[pair.slice(0, eq).trim()] = pair.slice(eq + 1).trim();
  }
  return cookies;
}

function serializeCookies(cookieMap) {
  return Object.entries(cookieMap)
    .map(([key, value]) => `${key}=${value}`)
    .join('; ');
}

function supabaseHeaders(anonKey) {
  return {
    accept: '*/*',
    'accept-language': 'en-US,en;q=0.9,ha;q=0.8',
    apikey: anonKey,
    authorization: `Bearer ${anonKey}`,
    'content-type': 'application/json;charset=UTF-8',
    origin: 'https://ngxpulse.ng',
    priority: 'u=1, i',
    referer: 'https://ngxpulse.ng/',
    'sec-ch-ua': '"Not=A?Brand";v="99", "Google Chrome";v="151", "Chromium";v="151"',
    'sec-ch-ua-mobile': '?1',
    'sec-ch-ua-platform': '"Android"',
    'sec-fetch-dest': 'empty',
    'sec-fetch-mode': 'cors',
    'sec-fetch-site': 'cross-site',
    'user-agent': USER_AGENT,
    'x-client-info': 'supabase-js/2.112.1; runtime=web',
    'x-supabase-api-version': '2024-01-01',
  };
}

function ngxpulseHeaders(extra = {}) {
  return {
    Accept: '*/*',
    'Accept-Language': 'en-US,en;q=0.9,ha;q=0.8',
    Connection: 'keep-alive',
    Referer: 'https://ngxpulse.ng/',
    'Sec-Fetch-Dest': 'empty',
    'Sec-Fetch-Mode': 'cors',
    'Sec-Fetch-Site': 'same-origin',
    'User-Agent': USER_AGENT,
    'sec-ch-ua': '"Not=A?Brand";v="99", "Google Chrome";v="151", "Chromium";v="151"',
    'sec-ch-ua-mobile': '?1',
    'sec-ch-ua-platform': '"Android"',
    ...extra,
  };
}

function previewBody(text, max = 500) {
  if (text.length <= max) return text;
  return `${text.slice(0, max)}...`;
}

function formatExpiry(expiresAt) {
  if (!expiresAt) return 'unknown';
  const date = new Date(expiresAt * 1000);
  return Number.isNaN(date.getTime()) ? String(expiresAt) : date.toISOString();
}

function summarizePayload(name, json) {
  const data = json?.data ?? json;
  if (data == null) return 'no data field';

  if (name === 'market-status') {
    return `status=${data.status}, is_open=${data.is_open}`;
  }
  if (name === 'stocks') {
    const stocks = data.stocks ?? (Array.isArray(data) ? data : []);
    const sample = stocks[0];
    const price = sample?.current_price ?? sample?.price ?? 'n/a';
    return `count=${stocks.length}, first=${sample?.symbol ?? 'n/a'} @ ${price}`;
  }
  if (name === 'market' && typeof data === 'object') {
    const asi = typeof data.asi === 'object' ? data.asi?.value : data.asi;
    return `asi=${asi ?? 'n/a'}, market_cap=${data.market_cap ?? 'n/a'}`;
  }
  if (name === 'indices') {
    const indices = data.data ?? (Array.isArray(data) ? data : []);
    const sample = indices[0];
    const value = sample?.currentPrice ?? sample?.value ?? 'n/a';
    return `count=${indices.length}, first=${sample?.code ?? 'n/a'} @ ${value}`;
  }
  if (name.startsWith('prices/')) {
    const prices = data.prices ?? (Array.isArray(data) ? data : []);
    const sample = prices[0];
    const price = sample?.close_price ?? sample?.price ?? 'n/a';
    return `count=${prices.length}, first=${sample?.trade_date ?? sample?.date ?? 'n/a'} @ ${price}`;
  }
  if (Array.isArray(data)) return `count=${data.length}`;
  return typeof data === 'object' ? `keys=${Object.keys(data).join(',')}` : String(data);
}

async function login(supabaseUrl, anonKey, email, password) {
  const url = `${supabaseUrl.replace(/\/$/, '')}/auth/v1/token?grant_type=password`;
  const res = await fetch(url, {
    method: 'POST',
    headers: supabaseHeaders(anonKey),
    body: JSON.stringify({
      email,
      password,
      gotrue_meta_security: {},
    }),
  });

  const text = await res.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    body = null;
  }

  if (!res.ok) {
    const message = body?.error_description || body?.msg || body?.message || text;
    throw new Error(`Login failed (${res.status}): ${message}`);
  }

  const cookies = parseSetCookieHeaders(res.headers);
  return { body, cookies };
}

async function refreshToken(supabaseUrl, anonKey, refreshTokenValue) {
  const url = `${supabaseUrl.replace(/\/$/, '')}/auth/v1/token?grant_type=refresh_token`;
  const res = await fetch(url, {
    method: 'POST',
    headers: supabaseHeaders(anonKey),
    body: JSON.stringify({ refresh_token: refreshTokenValue }),
  });

  const text = await res.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    body = null;
  }

  if (!res.ok) {
    const message = body?.error_description || body?.msg || body?.message || text;
    throw new Error(`Refresh failed (${res.status}): ${message}`);
  }

  return body;
}

async function probeEndpoint({ endpointPath, accessToken, cookieHeader, mode }) {
  const headers = ngxpulseHeaders();
  if (mode === 'bearer' || mode === 'both') {
    headers.Authorization = `Bearer ${accessToken}`;
  }
  if (mode === 'cookies' || mode === 'both') {
    headers.Cookie = cookieHeader;
  }

  const res = await fetch(`${NGX_BASE_URL}${endpointPath}`, {
    method: 'GET',
    headers,
  });

  const text = await res.text();
  let json = null;
  let parseError = null;
  try {
    json = JSON.parse(text);
  } catch (err) {
    parseError = err instanceof Error ? err.message : String(err);
  }

  return {
    ok: res.ok && json !== null,
    status: res.status,
    statusText: res.statusText,
    bodyPreview: previewBody(text),
    json,
    parseError,
  };
}

async function main() {
  loadEnvFile();

  const supabaseUrl = requireEnv('NGX_PULSE_SUPABASE_URL');
  const anonKey = requireEnv('NGX_PULSE_SUPABASE_ANON_KEY');
  const email = requireEnv('NGX_PULSE_EMAIL');
  const password = requireEnv('NGX_PULSE_PASSWORD');

  console.log('=== NGX Pulse Session Test ===\n');

  const { body: session, cookies: loginCookies } = await login(
    supabaseUrl,
    anonKey,
    email,
    password,
  );

  const accessToken = session.access_token;
  const refreshTokenValue = session.refresh_token;
  const userEmail = session.user?.email || email;
  const expiresAt = formatExpiry(session.expires_at);

  console.log(`[login] OK — user: ${userEmail}, expires: ${expiresAt}`);

  const cookieMap = { ...loginCookies, 'ngxpulse-signedin': '1' };
  const cookieHeader = serializeCookies(cookieMap);
  const supabaseCookieCount = Object.keys(loginCookies).length;
  console.log(
    `[cookies] captured: ${supabaseCookieCount} from Supabase, added ngxpulse-signedin=1`,
  );
  console.log('');

  const authWinners = [];
  console.log('--- Auth probe (market-status) ---\n');
  for (const attempt of AUTH_MODES) {
    const result = await probeEndpoint({
      endpointPath: '/ngxdata/market-status',
      accessToken,
      cookieHeader,
      mode: attempt.mode,
    });
    const statusLabel = result.ok ? 'OK' : `${result.status} ${result.statusText}`;
    console.log(`[market-status] ${attempt.label}: ${statusLabel}`);
    console.log(`  body: ${result.bodyPreview}`);
    if (result.parseError) {
      console.log(`  parse error: ${result.parseError}`);
    }
    if (result.ok) {
      authWinners.push(attempt.mode);
    }
    console.log('');
  }

  const preferredAuth = authWinners[0] || 'bearer';
  const authLabels = { bearer: 'bearer', cookies: 'cookies', both: 'bearer+cookies' };
  console.log(`--- Data endpoints (using ${authLabels[preferredAuth]} auth) ---\n`);

  const endpointResults = [];
  for (const endpoint of ENDPOINTS) {
    if (endpoint.authProbe) continue;

    const result = await probeEndpoint({
      endpointPath: endpoint.path,
      accessToken,
      cookieHeader,
      mode: preferredAuth,
    });
    const statusLabel = result.ok ? 'OK' : `${result.status} ${result.statusText}`;
    const summary = result.ok ? summarizePayload(endpoint.name, result.json) : '';
    console.log(`[${endpoint.name}] ${statusLabel}`);
    if (summary) console.log(`  summary: ${summary}`);
    console.log(`  body: ${result.bodyPreview}`);
    if (result.parseError) {
      console.log(`  parse error: ${result.parseError}`);
    }
    console.log('');
    endpointResults.push({ name: endpoint.name, ok: result.ok });
  }

  if (refreshTokenValue) {
    try {
      const refreshed = await refreshToken(supabaseUrl, anonKey, refreshTokenValue);
      const newExpiry = formatExpiry(refreshed.expires_at);
      console.log(`[refresh] OK — new token expires: ${newExpiry}`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.log(`[refresh] FAIL — ${message}`);
    }
    console.log('');
  } else {
    console.log('[refresh] skipped — no refresh_token in login response\n');
  }

  const dataEndpointsOk = endpointResults.every((r) => r.ok);
  const authOk = authWinners.length > 0;

  if (authOk && dataEndpointsOk) {
    const modes = authWinners.map((mode) => authLabels[mode]).join(', ');
    const passed = endpointResults.map((r) => r.name).join(', ');
    console.log(`RESULT: All endpoints OK. Auth modes: ${modes}. Data: ${passed}`);
    process.exit(0);
  }

  if (authOk && !dataEndpointsOk) {
    const failed = endpointResults.filter((r) => !r.ok).map((r) => r.name).join(', ');
    console.log(`RESULT: Auth works but some endpoints failed: ${failed}`);
    process.exit(1);
  }

  console.log('RESULT: No auth strategy succeeded.');
  process.exit(1);
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
