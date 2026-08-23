#!/usr/bin/env node
import * as readline from 'readline';
import {
  AgentRequestSchema,
  AgentToolResultSchema,
  type AgentResponse,
} from '@ngx/shared';
import {
  generatePortfolioSignals,
  generateStrategyCoach,
  generateSymbolSignal,
  testLlmConnection,
} from './llm';

process.stdout.on('error', (err: NodeJS.ErrnoException) => {
  // Parent (`tauri dev` rebuild / app exit) closed the pipe — exit quietly.
  if (err.code === 'EPIPE') {
    process.exit(0);
  }
});

function respond(response: AgentResponse): void {
  try {
    process.stdout.write(`${JSON.stringify(response)}\n`);
  } catch (err) {
    const code = err && typeof err === 'object' && 'code' in err ? (err as NodeJS.ErrnoException).code : '';
    if (code === 'EPIPE') {
      process.exit(0);
    }
    throw err;
  }
}

const pendingLines: string[] = [];
const waiters: Array<(line: string) => void> = [];

function onIncoming(line: string): void {
  const next = waiters.shift();
  if (next) next(line);
  else pendingLines.push(line);
}

function takeLine(): Promise<string> {
  const queued = pendingLines.shift();
  if (queued !== undefined) return Promise.resolve(queued);
  return new Promise((resolve) => {
    waiters.push(resolve);
  });
}

async function callTool(name: string, args: Record<string, unknown>): Promise<unknown> {
  process.stdout.write(`${JSON.stringify({ type: 'tool', name, arguments: args })}\n`);
  const line = await takeLine();
  const parsed: unknown = JSON.parse(line);
  const result = AgentToolResultSchema.safeParse(parsed);
  if (!result.success) {
    return { ok: false, error: 'invalid tool_result' };
  }
  return result.data.result;
}

async function handleRequest(line: string): Promise<void> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    respond({ id: 'unknown', ok: false, error: 'Invalid JSON' });
    return;
  }

  const reqResult = AgentRequestSchema.safeParse(parsed);
  if (!reqResult.success) {
    respond({ id: 'unknown', ok: false, error: reqResult.error.message });
    return;
  }

  const req = reqResult.data;

  try {
    switch (req.op) {
      case 'ping':
        respond({ id: req.id, ok: true, data: { pong: true, version: '1.0.0' } });
        break;
      case 'test_llm': {
        const message = await testLlmConnection(req.llm);
        respond({ id: req.id, ok: true, data: { message } });
        break;
      }
      case 'portfolio_signals': {
        const result = await generatePortfolioSignals(req.context, req.llm, callTool);
        respond({ id: req.id, ok: true, data: result });
        break;
      }
      case 'symbol_signal': {
        const result = await generateSymbolSignal(req.context, req.llm);
        respond({ id: req.id, ok: true, data: result });
        break;
      }
      case 'strategy_coach': {
        const result = await generateStrategyCoach(req.context, req.llm, callTool);
        respond({ id: req.id, ok: true, data: result });
        break;
      }
    }
  } catch (err) {
    respond({
      id: req.id,
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

const rl = readline.createInterface({ input: process.stdin, terminal: false });

rl.on('line', (line) => {
  onIncoming(line.trim());
});

void (async () => {
  for (;;) {
    const line = await takeLine();
    await handleRequest(line);
  }
})();

process.stderr.write('ngx-agent worker ready\n');
