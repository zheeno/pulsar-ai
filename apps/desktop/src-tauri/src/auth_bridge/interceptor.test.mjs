import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const dir = path.dirname(fileURLToPath(import.meta.url));
const template = fs.readFileSync(path.join(dir, 'interceptor.js'), 'utf8');

function install(origin, { nonce = 'test-nonce', headers = ['authorization'], wsParam = null, allowedOrigins } = {}) {
  const calls = [];
  const script = template
    .replace('__PULSAR_SESSION_NONCE__', nonce)
    .replace('__PULSAR_ALLOWED_ORIGINS__', JSON.stringify(allowedOrigins || [origin]))
    .replace('__PULSAR_HEADER_NAMES__', JSON.stringify(headers))
    .replace('__PULSAR_WS_PARAM__', wsParam ? JSON.stringify(wsParam) : 'null');

  class HeadersPolyfill {
    constructor(init) {
      this._map = new Map();
      if (init) {
        if (Array.isArray(init)) {
          for (const [k, v] of init) this._map.set(String(k).toLowerCase(), String(v));
        } else {
          for (const [k, v] of Object.entries(init)) this._map.set(k.toLowerCase(), String(v));
        }
      }
    }
    forEach(cb) {
      for (const [k, v] of this._map) cb(v, k);
    }
  }

  class XMLHttpRequest {}
  XMLHttpRequest.prototype.open = function () {};
  XMLHttpRequest.prototype.setRequestHeader = function () {};
  XMLHttpRequest.prototype.send = function () {};

  const nativeFetch = async (input, init) => ({ ok: true, input, init });
  class NativeWS {
    constructor(url) {
      this.url = url;
    }
  }
  NativeWS.CONNECTING = 0;
  NativeWS.OPEN = 1;
  NativeWS.CLOSING = 2;
  NativeWS.CLOSED = 3;

  const ctx = {
    location: { origin },
    window: {},
    Headers: HeadersPolyfill,
    XMLHttpRequest,
    WeakMap,
  };
  ctx.window = ctx;
  ctx.window.fetch = nativeFetch;
  ctx.window.XMLHttpRequest = XMLHttpRequest;
  ctx.window.WebSocket = NativeWS;
  ctx.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
    },
  };

  vm.runInNewContext(script, ctx);
  return { ctx, calls, nativeFetch };
}

{
  const { ctx, calls } = install('https://app.busha.io');
  await ctx.window.fetch('https://api.busha.co/v1/me', {
    headers: { Authorization: 'Bearer abc.def.ghi' },
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, 'auth_bridge_submit_candidate');
  assert.deepEqual(Object.keys(calls[0].args).sort(), ['headerName', 'sessionNonce', 'value']);
  assert.equal(calls[0].args.headerName, 'authorization');
  assert.equal(calls[0].args.value, 'Bearer abc.def.ghi');
  assert.equal(calls[0].args.sessionNonce, 'test-nonce');
}

{
  const { ctx, calls } = install('https://evil.example', {
    allowedOrigins: ['https://app.busha.io'],
  });
  await ctx.window.fetch('https://api.busha.co/v1/me', {
    headers: { Authorization: 'Bearer abc.def.ghi' },
  });
  assert.equal(calls.length, 0);
}

{
  const { ctx, calls } = install('https://app.busha.io');
  await ctx.window.fetch('https://api.busha.co/v1/me', {
    headers: { 'X-Other': 'nope' },
  });
  assert.equal(calls.length, 0);
}

{
  const { ctx, calls } = install('https://app.busha.io');
  const xhr = new ctx.window.XMLHttpRequest();
  xhr.open('GET', '/me');
  xhr.setRequestHeader('Authorization', 'Bearer xhr.tok.en');
  xhr.send();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].args.value, 'Bearer xhr.tok.en');
}

{
  const { ctx, calls } = install('https://app.busha.io', { wsParam: 'access_token' });
  new ctx.window.WebSocket('wss://api.busha.co/ws?access_token=ws-secret');
  assert.equal(calls.length, 1);
  assert.equal(calls[0].args.headerName, 'websocket');
  assert.equal(calls[0].args.value, 'ws-secret');
}

console.log('interceptor tests ok');
