#!/usr/bin/env node
/** Smoke test: agent worker responds to ping over stdio */
const { spawn } = require('child_process');
const path = require('path');

const workerPath = path.join(__dirname, '../packages/agent/dist/worker.js');
const child = spawn('node', [workerPath], { stdio: ['pipe', 'pipe', 'inherit'] });

let resolved = false;
child.stdout.on('data', (buf) => {
  const line = buf.toString().trim();
  if (!line) return;
  const res = JSON.parse(line);
  if (res.ok && res.data?.pong) {
    console.log('Agent ping OK:', res.data);
    resolved = true;
    child.kill();
    process.exit(0);
  }
});

child.stdin.write(JSON.stringify({ id: 'test-1', op: 'ping' }) + '\n');

setTimeout(() => {
  if (!resolved) {
    console.error('Agent ping timeout');
    child.kill();
    process.exit(1);
  }
}, 5000);
