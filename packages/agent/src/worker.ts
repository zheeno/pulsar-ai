#!/usr/bin/env node
import * as readline from 'readline';
import { AgentRequestSchema, type AgentResponse } from '@ngx/shared';
import { generatePortfolioSignals, generateSymbolSignal, testLlmConnection } from './llm';

function respond(response: AgentResponse): void {
  process.stdout.write(`${JSON.stringify(response)}\n`);
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
        const result = await generatePortfolioSignals(req.context, req.llm);
        respond({ id: req.id, ok: true, data: result });
        break;
      }
      case 'symbol_signal': {
        const result = await generateSymbolSignal(req.context, req.llm);
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
  void handleRequest(line.trim());
});

process.stderr.write('ngx-agent worker ready\n');
